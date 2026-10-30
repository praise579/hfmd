use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::repo::{FileEntry, split_repo_id};

/// 链接的实际落地方式。调用方据此判断是否发生了降级并记录日志。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Symlink,
    HardLink,
    Copy,
}

/// 一个仓库在本地缓存中的目录结构：
///
/// ```text
/// <root>/models--<org>--<name>/
/// ├── blobs/<内容哈希>
/// ├── snapshots/<commit>/<仓库内相对路径>  → 相对符号链接
/// └── refs/<分支>                          → commit hash
/// ```
#[derive(Debug, Clone)]
pub struct RepoCache {
    repo_dir: PathBuf,
}

impl RepoCache {
    pub fn new(root: impl AsRef<Path>, repo_id: &str) -> Result<Self> {
        let (owner, name) = split_repo_id(repo_id)?;
        Ok(Self {
            repo_dir: root.as_ref().join(format!("models--{owner}--{name}")),
        })
    }

    pub fn repo_dir(&self) -> &Path {
        &self.repo_dir
    }

    pub fn blobs_dir(&self) -> PathBuf {
        self.repo_dir.join("blobs")
    }

    pub fn refs_dir(&self) -> PathBuf {
        self.repo_dir.join("refs")
    }

    pub fn snapshot_dir(&self, commit: &str) -> PathBuf {
        self.repo_dir.join("snapshots").join(commit)
    }

    pub fn blob_path(&self, blob_name: &str) -> PathBuf {
        self.blobs_dir().join(blob_name)
    }

    pub fn ref_path(&self, branch: &str) -> PathBuf {
        self.refs_dir().join(branch)
    }

    /// 在 snapshot 里为已下载的 blob 建立链接。
    ///
    /// 优先相对符号链接；Windows 无权限时退化为硬链接，再不行才复制。
    pub fn link_into_snapshot(
        &self,
        commit: &str,
        path_in_repo: &str,
        blob_name: &str,
    ) -> Result<LinkKind> {
        let relative = safe_relative_path(path_in_repo)?;
        let blob = self.blob_path(blob_name);
        if !blob.is_file() {
            bail!("blob 尚未下载：{}", blob.display());
        }

        let target = self.snapshot_dir(commit).join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建目录失败：{}", parent.display()))?;
        }
        if target.exists() || target.symlink_metadata().is_ok() {
            std::fs::remove_file(&target)
                .with_context(|| format!("移除旧链接失败：{}", target.display()))?;
        }

        create_link(&blob, &target, &relative)
    }

    /// 写入 `refs/<branch>`。内容为 commit hash 原文，不含换行。
    ///
    /// 先写临时文件再 rename，避免中断时留下半截引用。
    pub fn write_ref(&self, branch: &str, commit: &str) -> Result<()> {
        let relative = safe_relative_path(branch)?;
        std::fs::create_dir_all(self.refs_dir()).context("创建 refs 目录失败")?;

        let path = self.refs_dir().join(&relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let temp = path.with_extension("tmp");
        std::fs::write(&temp, commit)
            .with_context(|| format!("写入引用失败：{}", temp.display()))?;
        std::fs::rename(&temp, &path)
            .with_context(|| format!("更新引用失败：{}", path.display()))?;
        Ok(())
    }

    pub fn read_ref(&self, branch: &str) -> Result<Option<String>> {
        let path = self.ref_path(branch);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let commit = text.trim();
                Ok((!commit.is_empty()).then(|| commit.to_string()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("读取引用失败：{}", path.display())),
        }
    }
}

/// blob 名：LFS 文件用内容 sha256，普通文件用 git blob SHA-1。
pub fn blob_name(file: &FileEntry) -> Result<String> {
    if let Some(sha256) = &file.sha256 {
        return Ok(sha256.clone());
    }
    if !file.oid.is_empty() {
        return Ok(file.oid.clone());
    }
    bail!("文件缺少内容哈希，无法确定 blob 名：{}", file.path)
}

/// 默认缓存根目录：`HF_HUB_CACHE` → `HF_HOME/hub` → `~/.cache/huggingface/hub`。
pub fn default_root() -> Option<PathBuf> {
    if let Ok(value) = std::env::var("HF_HUB_CACHE")
        && !value.trim().is_empty()
    {
        return Some(PathBuf::from(value));
    }
    crate::config::hf_home().map(|home| home.join("hub"))
}

fn safe_relative_path(path: &str) -> Result<PathBuf> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("非法仓库路径：{path}");
    }
    Ok(relative.to_path_buf())
}

fn create_link(blob: &Path, target: &Path, relative: &Path) -> Result<LinkKind> {
    let depth = relative.components().count().saturating_sub(1);
    let blob_name = blob
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let link_target = format!("{}blobs/{blob_name}", "../".repeat(depth + 2));

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&link_target, target)
            .with_context(|| format!("创建符号链接失败：{}", target.display()))?;
        Ok(LinkKind::Symlink)
    }

    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_file(&link_target, target).is_ok() {
            return Ok(LinkKind::Symlink);
        }
        if std::fs::hard_link(blob, target).is_ok() {
            return Ok(LinkKind::HardLink);
        }
        std::fs::copy(blob, target)
            .with_context(|| format!("复制 blob 失败：{}", target.display()))?;
        Ok(LinkKind::Copy)
    }

    #[cfg(not(any(unix, windows)))]
    {
        std::fs::copy(blob, target)
            .with_context(|| format!("复制 blob 失败：{}", target.display()))?;
        Ok(LinkKind::Copy)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_root() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("hfmd-cache-{}-{id}", std::process::id()))
    }

    fn entry(path: &str, sha256: Option<&str>, oid: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            size: 0,
            oid: oid.to_string(),
            sha256: sha256.map(str::to_string),
            xet_hash: None,
        }
    }

    fn seed_blob(cache: &RepoCache, name: &str) -> PathBuf {
        let blob = cache.blob_path(name);
        std::fs::create_dir_all(cache.blobs_dir()).unwrap();
        std::fs::write(&blob, b"content").unwrap();
        blob
    }

    #[test]
    fn 仓库目录名遵循官方布局() {
        let cache = RepoCache::new("/cache", "Qwen/Qwen2.5-7B").unwrap();
        assert_eq!(
            cache.repo_dir(),
            Path::new("/cache/models--Qwen--Qwen2.5-7B")
        );
        assert_eq!(
            cache.blob_path("abc"),
            Path::new("/cache/models--Qwen--Qwen2.5-7B/blobs/abc")
        );
        assert_eq!(
            cache.snapshot_dir("deadbeef"),
            Path::new("/cache/models--Qwen--Qwen2.5-7B/snapshots/deadbeef")
        );
        assert_eq!(
            cache.ref_path("main"),
            Path::new("/cache/models--Qwen--Qwen2.5-7B/refs/main")
        );
    }

    #[test]
    fn 非法仓库标识被拒绝() {
        assert!(RepoCache::new("/cache", "Qwen").is_err());
    }

    #[test]
    fn blob_名优先用_sha256_再退到_oid() {
        assert_eq!(
            blob_name(&entry("a.safetensors", Some("sha"), "oid")).unwrap(),
            "sha"
        );
        assert_eq!(
            blob_name(&entry("config.json", None, "oid")).unwrap(),
            "oid"
        );
        assert!(blob_name(&entry("broken", None, "")).is_err());
    }

    #[test]
    fn 符号链接使用相对路径且层级正确() {
        let root = temp_root();
        let cache = RepoCache::new(&root, "org/name").unwrap();
        seed_blob(&cache, "blob1");

        let flat = cache
            .link_into_snapshot("c1", "config.json", "blob1")
            .unwrap();
        assert_eq!(flat, LinkKind::Symlink);
        let nested = cache
            .link_into_snapshot("c1", "sub/dir/weight.bin", "blob1")
            .unwrap();
        assert_eq!(nested, LinkKind::Symlink);

        #[cfg(unix)]
        {
            assert_eq!(
                std::fs::read_link(cache.snapshot_dir("c1").join("config.json")).unwrap(),
                Path::new("../../blobs/blob1")
            );
            assert_eq!(
                std::fs::read_link(cache.snapshot_dir("c1").join("sub/dir/weight.bin")).unwrap(),
                Path::new("../../../../blobs/blob1")
            );
            assert_eq!(
                std::fs::read(cache.snapshot_dir("c1").join("sub/dir/weight.bin")).unwrap(),
                b"content"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn blob_不存在时报错() {
        let root = temp_root();
        let cache = RepoCache::new(&root, "org/name").unwrap();
        assert!(cache.link_into_snapshot("c1", "a", "missing").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 路径穿越被拒绝() {
        let root = temp_root();
        let cache = RepoCache::new(&root, "org/name").unwrap();
        seed_blob(&cache, "blob1");

        assert!(
            cache
                .link_into_snapshot("c1", "../escape", "blob1")
                .is_err()
        );
        assert!(cache.write_ref("../escape", "c1").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 引用写入后内容为_commit_且无换行() {
        let root = temp_root();
        let cache = RepoCache::new(&root, "org/name").unwrap();

        assert_eq!(cache.read_ref("main").unwrap(), None);
        cache.write_ref("main", "deadbeef").unwrap();
        assert_eq!(
            cache.read_ref("main").unwrap(),
            Some("deadbeef".to_string())
        );
        assert_eq!(
            std::fs::read_to_string(cache.ref_path("main")).unwrap(),
            "deadbeef"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 覆盖写入引用不会残留临时文件() {
        let root = temp_root();
        let cache = RepoCache::new(&root, "org/name").unwrap();

        cache.write_ref("main", "aaa").unwrap();
        cache.write_ref("main", "bbb").unwrap();

        assert_eq!(cache.read_ref("main").unwrap().as_deref(), Some("bbb"));
        let leftovers: Vec<_> = std::fs::read_dir(cache.refs_dir())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不应留下 .tmp 文件");

        let _ = std::fs::remove_dir_all(&root);
    }
}
