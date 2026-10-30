use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use reqwest::header::RANGE;
use reqwest::{Client, StatusCode, Url};
use tokio::task::{JoinHandle, JoinSet};

use crate::cache::{RepoCache, blob_name};
use crate::error::{HfmdError, Kind, RetryPolicy, from_status, is_retryable};
use crate::repo::{FileEntry, format_bytes};
use crate::segment;
use crate::state::{STATE_VERSION, SegmentProgress, SegmentState, StateStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// blob 已存在，未发起任何请求。
    Skipped,
    Downloaded,
}

#[derive(Clone)]
pub struct Downloader {
    http: Client,
    endpoint: String,
    token: Option<String>,
    connections: usize,
    quiet: bool,
    verify: bool,
    retry: RetryPolicy,
}

impl Downloader {
    pub fn new(
        endpoint: &str,
        token: Option<&str>,
        connections: usize,
        quiet: bool,
        verify: bool,
        retry: RetryPolicy,
    ) -> Result<Self> {
        let http = Client::builder()
            .user_agent(format!("hfmd/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .context("构造 HTTP 客户端失败")?;

        Ok(Self {
            http,
            endpoint: endpoint.trim_end_matches('/').to_string(),
            token: token.map(str::to_string),
            connections: connections.max(1),
            quiet,
            verify,
            retry,
        })
    }

    /// 下载单个文件到 blob，可跨进程续传。
    ///
    /// 失败时保留段状态与 `.part`，下次运行从断点继续；
    /// 校验不匹配时丢弃并自动重下一次。
    pub async fn download_file(
        &self,
        repo_id: &str,
        revision: &str,
        commit: &str,
        file: &FileEntry,
        cache: &RepoCache,
        store: &StateStore,
    ) -> Result<Outcome> {
        self.download_once(repo_id, revision, commit, file, cache, store, true)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_once(
        &self,
        repo_id: &str,
        revision: &str,
        commit: &str,
        file: &FileEntry,
        cache: &RepoCache,
        store: &StateStore,
        retry_on_mismatch: bool,
    ) -> Result<Outcome> {
        let blob = blob_name(file)?;
        let blob_path = cache.blob_path(&blob);
        if blob_path.is_file() {
            // blob 可能来自上一次中断的运行，但快照链接尚未建立。
            cache.link_into_snapshot(commit, &file.path, &blob)?;
            return Ok(Outcome::Skipped);
        }

        std::fs::create_dir_all(cache.blobs_dir()).context("创建 blobs 目录失败")?;

        if file.size == 0 {
            std::fs::write(&blob_path, [])
                .with_context(|| format!("创建空文件失败：{}", blob_path.display()))?;
            cache.link_into_snapshot(commit, &file.path, &blob)?;
            return Ok(Outcome::Downloaded);
        }

        let state = match store.load(&blob)? {
            Some(existing) if existing.size == file.size => existing,
            _ => SegmentState::new(
                &blob,
                file.size,
                &segment::plan(file.size, self.connections),
            ),
        };
        store.save(&state)?;

        let part = store.part_path(&blob);
        if let Some(parent) = part.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建状态目录失败：{}", parent.display()))?;
        }
        {
            let handle = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .read(true)
                .truncate(false)
                .open(&part)
                .with_context(|| format!("打开未完成文件失败：{}", part.display()))?;
            handle
                .set_len(file.size)
                .with_context(|| format!("预分配文件大小失败：{}", part.display()))?;
        }

        let url = resolve_url(&self.endpoint, repo_id, revision, &file.path)?;
        let bounds: Vec<(u64, u64)> = state
            .segments
            .iter()
            .map(|segment| (segment.start, segment.end))
            .collect();
        let offsets: Arc<Vec<AtomicU64>> = Arc::new(
            state
                .segments
                .iter()
                .map(|segment| AtomicU64::new(segment.offset))
                .collect(),
        );
        let written = Arc::new(AtomicU64::new(
            state.segments.iter().map(|segment| segment.offset).sum(),
        ));

        let stop = Arc::new(AtomicBool::new(false));
        let reporter = self.spawn_reporter(
            file,
            &state,
            &bounds,
            &offsets,
            &written,
            &stop,
            store.clone(),
        );

        let mut tasks = JoinSet::new();
        let file_size = file.size;
        for (index, segment) in state.segments.iter().enumerate() {
            let Some((resume_start, end)) = segment.remaining() else {
                continue;
            };

            let client = self.http.clone();
            let token = self.token.clone();
            let url = url.clone();
            let part = part.clone();
            let offsets = Arc::clone(&offsets);
            let written = Arc::clone(&written);
            let base = segment.start;
            let policy = self.retry.clone();

            tasks.spawn(async move {
                fetch_segment(
                    client,
                    token,
                    url,
                    part,
                    base,
                    resume_start,
                    end,
                    file_size,
                    offsets,
                    index,
                    written,
                    policy,
                )
                .await
            });
        }

        let mut failure = None;
        while let Some(joined) = tasks.join_next().await {
            // 只保留第一个真实错误：abort_all 之后其余任务会以 cancelled 结束，
            // 若让它们覆盖，真正的失败原因就丢了。
            match joined {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    failure.get_or_insert(err);
                    tasks.abort_all();
                }
                Err(err) if err.is_cancelled() => {}
                Err(err) => {
                    failure.get_or_insert_with(|| anyhow::Error::from(err));
                    tasks.abort_all();
                }
            }
        }

        stop.store(true, Ordering::Relaxed);
        if let Some(handle) = reporter {
            handle.abort();
        }

        if let Some(err) = failure {
            return Err(err);
        }

        let finished = snapshot(&blob, file.size, &bounds, &offsets);
        store.save(&finished)?;

        if self.verify {
            let verification = crate::verify::verify_file(&part, file)?;
            if !verification.matched() {
                // 内容不可信：连同段状态一起丢弃，避免下次从坏数据续传。
                store.discard(&blob)?;
                if retry_on_mismatch {
                    if !self.quiet {
                        eprintln!("校验失败，丢弃后重下：{}", file.path);
                    }
                    return Box::pin(
                        self.download_once(repo_id, revision, commit, file, cache, store, false),
                    )
                    .await;
                }
                bail!(
                    "校验失败：{}（{} 期望 {}，实际 {}）",
                    file.path,
                    verification.algorithm.as_str(),
                    verification.expected,
                    verification.actual
                );
            }
            if !self.quiet {
                eprintln!(
                    "校验通过：{}（{}）",
                    file.path,
                    verification.algorithm.as_str()
                );
            }
        } else if !self.quiet {
            eprintln!("未校验：{}（--skip-verify）", file.path);
        }

        std::fs::rename(&part, &blob_path)
            .with_context(|| format!("提交 blob 失败：{}", blob_path.display()))?;
        store.discard(&blob)?;
        cache.link_into_snapshot(commit, &file.path, &blob)?;

        Ok(Outcome::Downloaded)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_reporter(
        &self,
        file: &FileEntry,
        template: &SegmentState,
        bounds: &[(u64, u64)],
        offsets: &Arc<Vec<AtomicU64>>,
        written: &Arc<AtomicU64>,
        stop: &Arc<AtomicBool>,
        store: StateStore,
    ) -> Option<JoinHandle<()>> {
        let quiet = self.quiet;
        let label = file.path.clone();
        let total = file.size;
        let blob = template.blob.clone();
        let bounds = bounds.to_vec();
        let offsets = Arc::clone(offsets);
        let written = Arc::clone(written);
        let stop = Arc::clone(stop);

        Some(tokio::spawn(async move {
            use std::io::IsTerminal;

            // 保存固定每 0.8 秒一次（崩溃恢复依赖它）；打印在非终端下降频到 5 秒。
            let interactive = std::io::stderr().is_terminal();
            let print_period = if interactive {
                Duration::from_millis(800)
            } else {
                Duration::from_secs(5)
            };

            let started = Instant::now();
            let mut last_print = Instant::now();
            let mut ticker = tokio::time::interval(Duration::from_millis(800));
            ticker.tick().await;

            loop {
                ticker.tick().await;
                if stop.load(Ordering::Relaxed) {
                    break;
                }

                let _ = store.save(&snapshot(&blob, total, &bounds, &offsets));

                if !quiet && last_print.elapsed() >= print_period {
                    last_print = Instant::now();
                    let done = written.load(Ordering::Relaxed);
                    let elapsed = started.elapsed().as_secs_f64().max(0.001);
                    let line = format!(
                        "{}  {} / {}  {}/s",
                        label,
                        format_bytes(done),
                        format_bytes(total),
                        format_bytes((done as f64 / elapsed) as u64)
                    );
                    if interactive {
                        eprint!("\r{line}");
                    } else {
                        eprintln!("{line}");
                    }
                }
            }

            if !quiet && interactive {
                eprintln!();
            }
        }))
    }
}

fn snapshot(blob: &str, size: u64, bounds: &[(u64, u64)], offsets: &[AtomicU64]) -> SegmentState {
    SegmentState {
        version: STATE_VERSION,
        blob: blob.to_string(),
        size,
        segments: bounds
            .iter()
            .zip(offsets)
            .map(|((start, end), offset)| SegmentProgress {
                start: *start,
                end: *end,
                offset: offset.load(Ordering::Relaxed),
            })
            .collect(),
    }
}

/// 在 `start..=end` 区间内下载，失败按策略重试，并从已写入的位置继续。
#[allow(clippy::too_many_arguments)]
async fn fetch_segment(
    client: Client,
    token: Option<String>,
    url: Url,
    part: PathBuf,
    base: u64,
    start: u64,
    end: u64,
    file_size: u64,
    offsets: Arc<Vec<AtomicU64>>,
    index: usize,
    written: Arc<AtomicU64>,
    policy: RetryPolicy,
) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&part)
        .with_context(|| format!("打开未完成文件失败：{}", part.display()))?;

    let mut position = start;
    let mut attempt = 0;

    loop {
        match stream_once(
            &client,
            token.as_deref(),
            &url,
            &file,
            position,
            end,
            file_size,
            base,
            &offsets,
            index,
            &written,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(err) => {
                if !is_retryable(&err) || attempt >= policy.max_retries {
                    return Err(err);
                }
                tokio::time::sleep(policy.delay(attempt)).await;
                attempt += 1;
                // 重试从已落盘的偏移继续，不重传已经写好的字节。
                position = base + offsets[index].load(Ordering::Relaxed);
            }
        }
    }
}

/// 单次请求：从 `start` 读到 `end`，边读边定位写入。
#[allow(clippy::too_many_arguments)]
async fn stream_once(
    client: &Client,
    token: Option<&str>,
    url: &Url,
    file: &std::fs::File,
    start: u64,
    end: u64,
    file_size: u64,
    base: u64,
    offsets: &[AtomicU64],
    index: usize,
    written: &AtomicU64,
) -> Result<()> {
    // 整文件单流时不要发 Range：不少服务端对完整文件的 Range 请求直接回 200，
    // 而对分段请求回 206。发 Range 反而会让单流路径失败。
    let whole_file = start == 0 && end + 1 == file_size;

    let mut request = client.get(url.clone());
    if !whole_file {
        request = request.header(RANGE, format!("bytes={start}-{end}"));
    }
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }

    let mut response = request
        .send()
        .await
        .with_context(|| format!("请求失败：{url}"))?;

    let status = response.status();
    let accepted = if whole_file {
        status.is_success()
    } else {
        status == StatusCode::PARTIAL_CONTENT
    };
    if !accepted {
        let typed = if status == StatusCode::OK {
            HfmdError::new(
                Kind::Range,
                "服务端未按 Range 返回，可能不支持分段下载",
                false,
            )
        } else {
            from_status(status)
        };
        // 必须保留类型，否则重试策略无法识别「可重试」。
        return Err(anyhow::Error::new(typed).context(format!("下载失败：{url}")));
    }

    let mut position = start;
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("读取响应失败：{url}"))?
    {
        let len = chunk.len() as u64;
        if position + len > end + 1 {
            return Err(anyhow::Error::new(HfmdError::new(
                Kind::Range,
                "服务端返回的字节超出请求区间",
                false,
            ))
            .context(format!("{url}")));
        }
        write_at(file, &chunk, position).with_context(|| format!("写入未完成文件失败：{url}"))?;
        position += len;
        offsets[index].store(position - base, Ordering::Relaxed);
        written.fetch_add(len, Ordering::Relaxed);
    }

    if position != end + 1 {
        // 截断通常是网络抖动，值得重试；重试会从已写入的位置继续。
        return Err(anyhow::Error::new(HfmdError::new(
            Kind::Network,
            format!(
                "响应提前结束：期望到 {end}，实际到 {}",
                position.saturating_sub(1)
            ),
            true,
        ))
        .context(format!("{url}")));
    }

    Ok(())
}

/// `{endpoint}/{repo}/resolve/{revision}/{path}`。模型仓库没有 `models/` 前缀。
pub fn resolve_url(endpoint: &str, repo_id: &str, revision: &str, path: &str) -> Result<Url> {
    let mut url =
        Url::parse(endpoint).with_context(|| format!("endpoint 不是合法 URL：{endpoint}"))?;

    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("endpoint 不能作为基地址：{endpoint}"))?;
        segments.pop_if_empty();
        for part in repo_id.split('/').filter(|part| !part.is_empty()) {
            segments.push(part);
        }
        segments.push("resolve");
        for part in revision.split('/').filter(|part| !part.is_empty()) {
            segments.push(part);
        }
        for part in path.split('/').filter(|part| !part.is_empty()) {
            segments.push(part);
        }
    }

    Ok(url)
}

#[cfg(unix)]
fn write_at(file: &std::fs::File, buf: &[u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

#[cfg(windows)]
fn write_at(file: &std::fs::File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;

    let mut written = 0;
    while written < buf.len() {
        let n = file.seek_write(&buf[written..], offset + written as u64)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "写入 0 字节"));
        }
        written += n;
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn write_at(file: &std::fs::File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};

    let mut clone = file.try_clone()?;
    clone.seek(SeekFrom::Start(offset))?;
    clone.write_all(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_url_模型仓库无前缀() {
        let url = resolve_url(
            "https://hf-mirror.com/",
            "Qwen/Qwen2.5-7B",
            "main",
            "model.safetensors",
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://hf-mirror.com/Qwen/Qwen2.5-7B/resolve/main/model.safetensors"
        );
    }

    #[test]
    fn resolve_url_支持带斜杠的_revision_与嵌套路径() {
        let url = resolve_url(
            "https://huggingface.co",
            "org/name",
            "refs/pr/1",
            "sub/dir/w.bin",
        )
        .unwrap();
        assert_eq!(url.path(), "/org/name/resolve/refs/pr/1/sub/dir/w.bin");
    }

    #[test]
    fn resolve_url_对特殊字符做编码() {
        let url = resolve_url("https://huggingface.co", "org/name", "main", "a b/c#1.bin").unwrap();
        assert_eq!(url.path(), "/org/name/resolve/main/a%20b/c%231.bin");
    }

    #[test]
    fn 快照能反映各片偏移() {
        let bounds = vec![(0, 99), (100, 199)];
        let offsets = vec![AtomicU64::new(40), AtomicU64::new(100)];
        let state = snapshot("blob", 200, &bounds, &offsets);

        assert_eq!(state.segments[0].offset, 40);
        assert_eq!(state.segments[1].offset, 100);
        assert!(!state.is_complete());
        assert_eq!(state.resume_ranges(), vec![(40, 99)]);
    }
}
