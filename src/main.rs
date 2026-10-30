mod cli;

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Parser;
use cli::{CacheCommand, Cli, Command};
use hfmd::api::Api;
use hfmd::cache::RepoCache;
use hfmd::config::{self, resolve_endpoint, resolve_token};
use hfmd::download::{Downloader, Outcome};
use hfmd::state::StateStore;
use tokio::task::JoinSet;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
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
            skip_verify: _,
            dry_run,
        } => {
            let api = Api::new(&endpoint, token.as_deref())?;
            let files = api.list_files(&repo_id, &revision).await?;
            let mut files = hfmd::repo::filter_files(files, &include, &exclude)?;
            if !wanted.is_empty() {
                files.retain(|file| wanted.iter().any(|path| path == &file.path));
            }

            let report = hfmd::repo::report(&repo_id, &revision, files);
            if report.total_files == 0 {
                bail!("没有匹配的文件");
            }

            if dry_run {
                print!("{}", hfmd::repo::render(&report, cli.json, usize::MAX));
                return Ok(());
            }

            let root = hfmd::cache::default_root()
                .context("无法确定缓存目录，请设置 HF_HOME 或 HF_HUB_CACHE")?;
            let cache = RepoCache::new(&root, &repo_id)?;
            let store = StateStore::new(cache.repo_dir());

            let jobs = cli.jobs.or(config.jobs).unwrap_or(4).max(1);
            let connections = cli.connections.or(config.connections).unwrap_or(8);
            let downloader = Downloader::new(&endpoint, token.as_deref(), connections, cli.quiet)?;

            let semaphore = Arc::new(tokio::sync::Semaphore::new(jobs));
            let mut tasks = JoinSet::new();
            let commit = api.resolve_commit(&repo_id, &revision).await?;

            for file in report.files {
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
                    (file.path, outcome)
                });
            }

            let mut failures = 0;
            while let Some(joined) = tasks.join_next().await {
                let (path, outcome) = joined.context("下载任务异常终止")?;
                match outcome {
                    Ok(Outcome::Skipped) => {}
                    Ok(Outcome::Downloaded) => {
                        if !cli.quiet {
                            println!("完成：{path}");
                        }
                    }
                    Err(err) => {
                        failures += 1;
                        eprintln!("失败：{path}：{err:#}");
                    }
                }
            }

            if failures > 0 {
                bail!("{failures} 个文件下载失败，断点已保留，可重新运行继续");
            }

            // 整批文件都已落地后才更新引用，避免 refs 指向不完整的 snapshot。
            cache.write_ref(&revision, &commit)?;

            Ok(())
        }

        Command::Cache { command } => match command {
            CacheCommand::List => bail!("cache list 尚未实现（见 #11）"),
            CacheCommand::Remove { .. } => bail!("cache remove 尚未实现（见 #11）"),
        },
    }
}
