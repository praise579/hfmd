use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::segment::Segment;

/// 段状态文件的格式版本。结构变化时递增，旧版本会被安全丢弃。
pub const STATE_VERSION: u32 = 1;

/// 单个分片的进度。`offset` 是已写入该分片的字节数，下一次写入位置为 `start + offset`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentProgress {
    pub start: u64,
    pub end: u64,
    pub offset: u64,
}

impl SegmentProgress {
    /// 分片总长度。
    pub fn size(&self) -> u64 {
        self.end - self.start + 1
    }

    pub fn is_done(&self) -> bool {
        self.offset >= self.size()
    }

    /// 尚未下载的区间（闭区间），已完成时为 `None`。
    pub fn remaining(&self) -> Option<(u64, u64)> {
        if self.is_done() {
            None
        } else {
            Some((self.start + self.offset, self.end))
        }
    }
}

/// 一个未完成文件的段状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentState {
    pub version: u32,
    pub blob: String,
    pub size: u64,
    pub segments: Vec<SegmentProgress>,
}

impl SegmentState {
    pub fn new(blob: &str, size: u64, segments: &[Segment]) -> Self {
        Self {
            version: STATE_VERSION,
            blob: blob.to_string(),
            size,
            segments: segments
                .iter()
                .map(|segment| SegmentProgress {
                    start: segment.start,
                    end: segment.end,
                    offset: 0,
                })
                .collect(),
        }
    }

    pub fn is_complete(&self) -> bool {
        self.segments.iter().all(SegmentProgress::is_done)
    }

    /// 仍需下载的区间列表，可直接用于发 `Range` 请求。
    pub fn resume_ranges(&self) -> Vec<(u64, u64)> {
        self.segments
            .iter()
            .filter_map(SegmentProgress::remaining)
            .collect()
    }

    /// 结构自洽性：分片首尾相接、覆盖整个文件、偏移不越界。
    fn is_consistent(&self) -> bool {
        if self.version != STATE_VERSION {
            return false;
        }
        if self.segments.is_empty() {
            return self.size == 0;
        }

        let mut expected_start = 0;
        for segment in &self.segments {
            if segment.start != expected_start || segment.end < segment.start {
                return false;
            }
            if segment.offset > segment.size() {
                return false;
            }
            expected_start = segment.end + 1;
        }
        expected_start == self.size
    }
}

/// 未完成文件与段状态的存放位置：`<repo_dir>/.hfmd/`。
///
/// 刻意避开 `snapshots/`，否则半成品会出现在快照视图里，被 `transformers` 当成完整文件。
#[derive(Debug, Clone)]
pub struct StateStore {
    dir: PathBuf,
}

impl StateStore {
    pub fn new(repo_dir: impl AsRef<Path>) -> Self {
        Self {
            dir: repo_dir.as_ref().join(".hfmd"),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn state_path(&self, blob: &str) -> PathBuf {
        self.dir.join("incomplete").join(format!("{blob}.json"))
    }

    pub fn part_path(&self, blob: &str) -> PathBuf {
        self.dir.join("incomplete").join(format!("{blob}.part"))
    }

    /// 原子写入：先写 `.tmp` 再 rename，中断时不会留下半截 JSON。
    pub fn save(&self, state: &SegmentState) -> Result<()> {
        if !state.is_consistent() {
            bail!("拒绝写入不自洽的段状态：{}", state.blob);
        }

        let path = self.state_path(&state.blob);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建状态目录失败：{}", parent.display()))?;
        }

        let temp = path.with_extension("json.tmp");
        let text = serde_json::to_string(state).context("序列化段状态失败")?;
        std::fs::write(&temp, text)
            .with_context(|| format!("写入段状态失败：{}", temp.display()))?;
        std::fs::rename(&temp, &path)
            .with_context(|| format!("提交段状态失败：{}", path.display()))?;
        Ok(())
    }

    /// 读取段状态。文件缺失、损坏、版本不匹配或结构不自洽时返回 `None`，
    /// 并删除损坏的状态文件——下次从头下载，而不是带着不可信数据继续。
    pub fn load(&self, blob: &str) -> Result<Option<SegmentState>> {
        let path = self.state_path(blob);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| format!("读取段状态失败：{}", path.display()));
            }
        };

        let state = serde_json::from_str::<SegmentState>(&text)
            .ok()
            .filter(|state| state.blob == blob && state.is_consistent());

        if state.is_none() {
            let _ = std::fs::remove_file(&path);
        }
        Ok(state)
    }

    /// 删除该文件的状态与未完成数据。
    pub fn discard(&self, blob: &str) -> Result<()> {
        for path in [self.state_path(blob), self.part_path(blob)] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(err).with_context(|| format!("删除失败：{}", path.display()));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_repo() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("hfmd-state-{}-{id}", std::process::id()))
    }

    fn store() -> (StateStore, PathBuf) {
        let repo = temp_repo();
        (StateStore::new(&repo), repo)
    }

    fn segments() -> Vec<Segment> {
        vec![
            Segment {
                index: 0,
                start: 0,
                end: 99,
            },
            Segment {
                index: 1,
                start: 100,
                end: 199,
            },
        ]
    }

    #[test]
    fn 保存后能原样读回() {
        let (store, repo) = store();
        let mut state = SegmentState::new("blob1", 200, &segments());
        state.segments[0].offset = 40;

        store.save(&state).unwrap();
        let loaded = store.load("blob1").unwrap().unwrap();

        assert_eq!(loaded, state);
        assert_eq!(loaded.resume_ranges(), vec![(40, 99), (100, 199)]);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 状态与未完成文件不在_snapshots_下() {
        let (store, repo) = store();
        assert!(store.state_path("x").starts_with(repo.join(".hfmd")));
        assert!(store.part_path("x").starts_with(repo.join(".hfmd")));
        assert!(
            !store
                .state_path("x")
                .to_string_lossy()
                .contains("snapshots")
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 缺失时返回_none() {
        let (store, repo) = store();
        assert!(store.load("nope").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 损坏的_json_被丢弃并删除文件() {
        let (store, repo) = store();
        let path = store.state_path("blob1");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();

        assert!(store.load("blob1").unwrap().is_none());
        assert!(!path.exists(), "损坏的状态文件应被删除");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 版本不匹配被丢弃() {
        let (store, repo) = store();
        let mut state = SegmentState::new("blob1", 200, &segments());
        state.version = STATE_VERSION + 1;
        let path = store.state_path("blob1");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();

        assert!(store.load("blob1").unwrap().is_none());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 分片有空洞时被丢弃() {
        let (store, repo) = store();
        let mut state = SegmentState::new("blob1", 200, &segments());
        state.segments[1].start = 150;
        let path = store.state_path("blob1");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();

        assert!(store.load("blob1").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 偏移越界时被丢弃() {
        let (store, repo) = store();
        let mut state = SegmentState::new("blob1", 200, &segments());
        state.segments[0].offset = 1000;
        let path = store.state_path("blob1");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string(&state).unwrap()).unwrap();

        assert!(store.load("blob1").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 覆盖保存不残留临时文件() {
        let (store, repo) = store();
        let state = SegmentState::new("blob1", 200, &segments());
        store.save(&state).unwrap();
        store.save(&state).unwrap();

        let leftovers: Vec<_> = std::fs::read_dir(store.state_path("blob1").parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 残留的临时文件不影响读取() {
        let (store, repo) = store();
        let state = SegmentState::new("blob1", 200, &segments());
        store.save(&state).unwrap();

        // 模拟进程在 rename 前被杀：只剩半截 .tmp，正式文件仍是上一次的完整内容
        std::fs::write(
            store.state_path("blob1").with_extension("json.tmp"),
            "{half",
        )
        .unwrap();

        assert_eq!(store.load("blob1").unwrap().unwrap(), state);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 全部完成后_is_complete_为真且没有待下区间() {
        let mut state = SegmentState::new("blob1", 200, &segments());
        state.segments[0].offset = 100;
        state.segments[1].offset = 100;

        assert!(state.is_complete());
        assert!(state.resume_ranges().is_empty());
    }

    #[test]
    fn discard_同时删除状态与未完成文件() {
        let (store, repo) = store();
        let state = SegmentState::new("blob1", 200, &segments());
        store.save(&state).unwrap();
        std::fs::write(store.part_path("blob1"), b"partial").unwrap();

        store.discard("blob1").unwrap();

        assert!(!store.state_path("blob1").exists());
        assert!(!store.part_path("blob1").exists());
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 拒绝写入不自洽的状态() {
        let (store, repo) = store();
        let mut state = SegmentState::new("blob1", 200, &segments());
        state.segments[1].start = 150;

        assert!(store.save(&state).is_err());
        let _ = std::fs::remove_dir_all(&repo);
    }
}
