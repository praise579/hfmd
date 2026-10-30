mod cli;

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Parser;
use cli::{CacheCommand, Cli, Command};
use hfmd::api::Api;
use hfmd::cache::RepoCache;
use hfmd::config::{self, resolve_endpoint, resolve_token};
use hfmd::download::{Downloader, Outcome};
use hfmd::error::RetryPolicy;
use hfmd::state::StateStore;
use tokio::task::JoinSet;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    hfmd::logging::init(cli.quiet);
    let config = config::Config::load()?;

    let env_endpoint = std::env::var("HF_ENDPOINT").ok();
    let env_token = std::env::var("HF_TOKEN").ok();
    let file_token = config::read_token_file();

    let endpoint = resolve_endpoint(
        cli.endpoint.as_deref(),
        env_endpoint.as_deref(),
        config.endpoint.as_deref(),
    );
    let token = resolve_token(
        cli.token.as_deref(),
        env_token.as_deref(),
        file_token.as_deref(),
    );

    match cli.command {
        Command::Info {
            repo_id,
            revision,
            include,
            exclude,
        } => {
            let api = Api::new(&endpoint, token.as_deref())?;
            let files = api.list_files(&repo_id, &revision).await?;
            let files = hfmd::repo::filter_files(files, &include, &exclude)?;
            let report = hfmd::repo::report(&repo_id, &revision, files);
            print!("{}", hfmd::repo::render(&report, cli.json, 50));
            Ok(())
        }

        Command::Download {
            repo_id,
            files: wanted,
            revision,
            include,
            exclude,
            skip_verify,
            dry_run,
        } => {
            let api = Api::new(&endpoint, token.as_deref())?;
            let files = api.list_files(&repo_id, &revision).await?;
            let mut files = hfmd::repo::filter_files(files, &include, &exclude)?;
            if !wanted.is_empty() {
                files.retain(|file| wanted.iter().any(|path| path == &file.path));
            }

            let selection = hfmd::repo::report(&repo_id, &revision, files);
            if selection.total_files == 0 {
                bail!("没有匹配的文件");
            }

            if dry_run {
                print!("{}", hfmd::repo::render(&selection, cli.json, usize::MAX));
                return Ok(());
            }

            let total_files = selection.total_files;
            let total_bytes = selection.total_size;

            let root = hfmd::cache::default_root()
                .context("无法确定缓存目录，请设置 HF_HOME 或 HF_HUB_CACHE")?;
            let cache = RepoCache::new(&root, &repo_id)?;
            let store = StateStore::new(cache.repo_dir());
            // 整个下载期间持有；第二个进程会立即失败而不是抢写段状态。
            let _lock = hfmd::lock::RepoLock::acquire(cache.repo_dir())?;

            let jobs = cli.jobs.or(config.jobs).unwrap_or(4).max(1);
            let connections = cli.connections.or(config.connections).unwrap_or(8);
            let max_retries = cli.max_retries.or(config.max_retries).unwrap_or(3);
            let downloader = Downloader::new(
                &endpoint,
                token.as_deref(),
                connections,
                cli.quiet,
                !skip_verify,
                RetryPolicy::new(max_retries),
            )?;

            let semaphore = Arc::new(tokio::sync::Semaphore::new(jobs));
            let mut tasks = JoinSet::new();
            let commit = api.resolve_commit(&repo_id, &revision).await?;

            for file in selection.files {
                let permit = Arc::clone(&semaphore)
                    .acquire_owned()
                    .await
                    .context("获取并发许可失败")?;

                let downloader = downloader.clone();
                let cache = cache.clone();
                let store = store.clone();
                let repo_id = repo_id.clone();
                let revision = revision.clone();
                let commit = commit.clone();

                tasks.spawn(async move {
                    let _permit = permit;
                    let outcome = downloader
                        .download_file(&repo_id, &revision, &commit, &file, &cache, &store)
                        .await;
                    (file.path, file.size, outcome)
                });
            }

            let mut result = hfmd::download::DownloadReport {
                repo_id: repo_id.clone(),
                revision: revision.clone(),
                commit: commit.clone(),
                total_files,
                total_bytes,
                ..Default::default()
            };

            while let Some(joined) = tasks.join_next().await {
                let (path, size, outcome) = joined.context("下载任务异常终止")?;
                match outcome {
                    Ok(Outcome::Skipped) => {
                        result.skipped += 1;
                        result.files.push(hfmd::download::FileOutcome {
                            path,
                            size,
                            status: "skipped",
                            error: None,
                        });
                    }
                    Ok(Outcome::Downloaded) => {
                        result.downloaded += 1;
                        if !cli.quiet && !cli.json {
                            println!("完成：{path}");
                        }
                        result.files.push(hfmd::download::FileOutcome {
                            path,
                            size,
                            status: "downloaded",
                            error: None,
                        });
                    }
                    Err(err) => {
                        result.failed += 1;
                        let kind = hfmd::error::kind_of(&err).unwrap_or("unknown");
                        tracing::error!(kind = %kind, path = %path, "文件下载失败：{err:#}");
                        result.files.push(hfmd::download::FileOutcome {
                            path,
                            size,
                            status: "failed",
                            error: Some(format!("{err:#}")),
                        });
                    }
                }
            }

            if cli.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            }

            if result.failed > 0 {
                bail!(
                    "{} 个文件下载失败，断点已保留，可重新运行继续",
                    result.failed
                );
            }

            // 整批文件都已落地后才更新引用，避免 refs 指向不完整的 snapshot。
            cache.write_ref(&revision, &commit)?;

            Ok(())
        }

        Command::Cache { command } => match command {
            CacheCommand::List => {
                let root = cache_root()?;
                let repos = hfmd::cache_admin::list(&root)?;
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&repos)?);
                } else {
                    print!("{}", hfmd::cache_admin::render_list(&repos));
                }
                Ok(())
            }

            CacheCommand::Remove {
                repo_id,
                revision,
                yes,
            } => {
                let root = cache_root()?;
                if !yes {
                    confirm(&repo_id, revision.as_deref())?;
                }
                let report = hfmd::cache_admin::remove(&root, &repo_id, revision.as_deref())?;
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!(
                        "已删除 {}：{} 个 revision，{} 个 blob，释放 {}",
                        report.repo_id,
                        report.removed_revisions.len(),
                        report.removed_blobs,
                        hfmd::repo::format_bytes(report.freed_bytes)
                    );
                }
                Ok(())
            }
        },
    }
}

fn cache_root() -> Result<std::path::PathBuf> {
    hfmd::cache::default_root().context("无法确定缓存目录，请设置 HF_HOME 或 HF_HUB_CACHE")
}

/// 删除前的确认。stdin 不是终端（例如管道）时读不到内容，按取消处理。
fn confirm(repo_id: &str, revision: Option<&str>) -> Result<()> {
    use std::io::{BufRead, Write};

    let target = match revision {
        Some(revision) => format!("{repo_id}@{revision}"),
        None => format!("{repo_id}（整个仓库）"),
    };

    eprint!("将删除 {target}，确认？[y/N] ");
    std::io::stderr().flush().ok();

    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;

    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        bail!("已取消");
    }
    Ok(())
}
