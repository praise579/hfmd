use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::cache::RepoCache;
use crate::lock::RepoLock;

#[derive(Debug, Clone, Serialize)]
pub struct CachedRepo {
    pub repo_id: String,
    pub size: u64,
    pub revisions: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct RemoveReport {
    pub repo_id: String,
    pub removed_revisions: Vec<String>,
    pub removed_blobs: usize,
    pub freed_bytes: u64,
}

/// 列出缓存根目录下的全部仓库，按 repo_id 排序。
pub fn list(root: &Path) -> Result<Vec<CachedRepo>> {
    let mut repos = Vec::new();

    for entry in read_dir_optional(root)? {
        if !entry.file_type().context("读取目录项失败")?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(repo_id) = repo_id_from_dir(&name) else {
            continue;
        };
        let dir = entry.path();
        repos.push(CachedRepo {
            repo_id,
            size: dir_size(&dir)?,
            revisions: revisions(&dir)?,
        });
    }

    repos.sort_by(|a, b| a.repo_id.cmp(&b.repo_id));
    Ok(repos)
}

/// 删除整个仓库，或只删除某个 revision。
///
/// 只删 revision 时，被其他 snapshot 引用的 blob 会保留。
pub fn remove(root: &Path, repo_id: &str, revision: Option<&str>) -> Result<RemoveReport> {
    let cache = RepoCache::new(root, repo_id)?;
    if !cache.repo_dir().is_dir() {
        bail!("本地没有该仓库的缓存：{repo_id}");
    }

    let _lock = RepoLock::acquire(cache.repo_dir())?;

    match revision {
        None => {
            let size = dir_size(cache.repo_dir())?;
            let revisions = revisions(cache.repo_dir())?;
            let blobs = read_dir_optional(&cache.blobs_dir())?.len();
            std::fs::remove_dir_all(cache.repo_dir())
                .with_context(|| format!("删除失败：{}", cache.repo_dir().display()))?;
            Ok(RemoveReport {
                repo_id: repo_id.to_string(),
                removed_revisions: revisions,
                removed_blobs: blobs,
                freed_bytes: size,
            })
        }
        Some(revision) => remove_revision(&cache, repo_id, revision),
    }
}

fn remove_revision(cache: &RepoCache, repo_id: &str, revision: &str) -> Result<RemoveReport> {
    // revision 可以是分支名（经 refs 解析）或直接是 commit。
    let commit = cache
        .read_ref(revision)?
        .unwrap_or_else(|| revision.to_string());
    let target = cache.snapshot_dir(&commit);
    if !target.is_dir() {
        bail!("本地没有该 revision 的快照：{repo_id}@{revision}");
    }
    ensure_symlink_snapshot(&target)?;

    let own = blob_names(&target)?;
    let mut referenced_elsewhere = HashSet::new();
    for entry in read_dir_optional(&cache.repo_dir().join("snapshots"))? {
        if entry.file_name().to_string_lossy() == commit {
            continue;
        }
        referenced_elsewhere.extend(blob_names(&entry.path())?);
    }

    let exclusive: Vec<String> = own.difference(&referenced_elsewhere).cloned().collect();

    std::fs::remove_dir_all(&target)
        .with_context(|| format!("删除快照失败：{}", target.display()))?;
    remove_refs_pointing_to(cache, &commit)?;

    let mut freed_bytes = 0;
    for blob in &exclusive {
        let path = cache.blob_path(blob);
        if let Ok(metadata) = path.symlink_metadata() {
            freed_bytes += metadata.len();
            let _ = std::fs::remove_file(&path);
        }
        // 段状态与未完成文件随 blob 一起清理，避免留下孤儿。
        for suffix in ["json", "part"] {
            let _ = std::fs::remove_file(
                cache
                    .repo_dir()
                    .join(".hfmd")
                    .join("incomplete")
                    .join(format!("{blob}.{suffix}")),
            );
        }
    }

    Ok(RemoveReport {
        repo_id: repo_id.to_string(),
        removed_revisions: vec![commit],
        removed_blobs: exclusive.len(),
        freed_bytes,
    })
}

/// `models--<org>--<name>` → `<org>/<name>`。
fn repo_id_from_dir(name: &str) -> Option<String> {
    let rest = name.strip_prefix("models--")?;
    let (org, repo) = rest.split_once("--")?;
    if org.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{org}/{repo}"))
}

fn revisions(repo_dir: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in read_dir_optional(&repo_dir.join("snapshots"))? {
        if entry.file_type()?.is_dir() {
            out.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    out.sort();
    Ok(out)
}

/// 统计目录占用。符号链接指向 blobs，跳过以免重复计数。
fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in read_dir_optional(&dir)? {
            let metadata = entry.metadata()?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                total += metadata.len();
            }
        }
    }

    Ok(total)
}

/// 只有符号链接才能从快照反查 blob 名。
///
/// Windows 无 symlink 权限时会退化为硬链接或复制，此时无法判断共享关系，
/// 按 revision 删除可能误删其他 revision 仍在用的 blob，因此直接拒绝。
fn ensure_symlink_snapshot(snapshot: &Path) -> Result<()> {
    let mut stack = vec![snapshot.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in read_dir_optional(&dir)? {
            let metadata = entry.metadata()?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                bail!(
                    "该快照使用硬链接或复制而非符号链接，无法安全地按 revision 删除；请改用 `cache remove <repo>` 删除整个仓库"
                );
            }
        }
    }

    Ok(())
}

/// 收集快照里所有符号链接指向的 blob 名。
fn blob_names(snapshot: &Path) -> Result<HashSet<String>> {
    let mut names = HashSet::new();
    let mut stack = vec![snapshot.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in read_dir_optional(&dir)? {
            let metadata = entry.metadata()?;
            if metadata.file_type().is_symlink() {
                if let Ok(target) = std::fs::read_link(entry.path())
                    && let Some(name) = target.file_name()
                {
                    names.insert(name.to_string_lossy().into_owned());
                }
            } else if metadata.is_dir() {
                stack.push(entry.path());
            }
        }
    }

    Ok(names)
}

fn remove_refs_pointing_to(cache: &RepoCache, commit: &str) -> Result<()> {
    let mut stack = vec![cache.refs_dir()];
    let mut stale = Vec::new();

    while let Some(dir) = stack.pop() {
        for entry in read_dir_optional(&dir)? {
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                stack.push(entry.path());
            } else if std::fs::read_to_string(entry.path())
                .map(|text| text.trim() == commit)
                .unwrap_or(false)
            {
                stale.push(entry.path());
            }
        }
    }

    for path in stale {
        std::fs::remove_file(&path).with_context(|| format!("删除引用失败：{}", path.display()))?;
    }
    Ok(())
}

fn read_dir_optional(path: &Path) -> Result<Vec<std::fs::DirEntry>> {
    match std::fs::read_dir(path) {
        Ok(entries) => entries
            .collect::<std::io::Result<Vec<_>>>()
            .with_context(|| format!("读取目录失败：{}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(err).with_context(|| format!("读取目录失败：{}", path.display())),
    }
}

/// 人类可读的缓存清单。
pub fn render_list(repos: &[CachedRepo]) -> String {
    if repos.is_empty() {
        return "缓存为空\n".to_string();
    }

    let mut out = String::new();
    let mut total = 0;
    for repo in repos {
        total += repo.size;
        out.push_str(&format!(
            "{:>10}  {:>3} 个 revision  {}\n",
            crate::repo::format_bytes(repo.size),
            repo.revisions.len(),
            repo.repo_id
        ));
    }
    out.push_str(&format!(
        "\n共 {} 个仓库，占用 {}\n",
        repos.len(),
        crate::repo::format_bytes(total)
    ));
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::repo::FileEntry;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_root() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("hfmd-cacheadmin-{}-{id}", std::process::id()))
    }

    fn seed(root: &Path, repo_id: &str, commit: &str, files: &[(&str, &str)]) -> RepoCache {
        let cache = RepoCache::new(root, repo_id).unwrap();
        for (path, blob) in files {
            std::fs::create_dir_all(cache.blobs_dir()).unwrap();
            std::fs::write(cache.blob_path(blob), vec![0u8; 100]).unwrap();
            cache.link_into_snapshot(commit, path, blob).unwrap();
        }
        cache
    }

    #[test]
    fn 列出仓库与_revision() {
        let root = temp_root();
        let cache = seed(&root, "org/name", "c1", &[("a.bin", "blob-a")]);
        cache.write_ref("main", "c1").unwrap();

        let repos = list(&root).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].repo_id, "org/name");
        assert_eq!(repos[0].revisions, vec!["c1".to_string()]);
        assert!(repos[0].size >= 100);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 删除整个仓库() {
        let root = temp_root();
        let cache = seed(&root, "org/name", "c1", &[("a.bin", "blob-a")]);

        let report = remove(&root, "org/name", None).unwrap();
        assert_eq!(report.removed_revisions, vec!["c1".to_string()]);
        assert!(!cache.repo_dir().exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn 删除单个_revision_保留共享_blob() {
        let root = temp_root();
        let cache = seed(&root, "org/name", "c1", &[("shared.bin", "blob-shared")]);
        seed(&root, "org/name", "c2", &[("shared.bin", "blob-shared")]);
        cache.write_ref("main", "c1").unwrap();
        cache.write_ref("v2", "c2").unwrap();

        let report = remove(&root, "org/name", Some("main")).unwrap();

        assert_eq!(report.removed_blobs, 0);
        assert!(!cache.snapshot_dir("c1").exists());
        assert!(cache.snapshot_dir("c2").exists());
        assert!(cache.blob_path("blob-shared").exists(), "共享 blob 应保留");
        assert!(cache.read_ref("main").unwrap().is_none());
        assert_eq!(cache.read_ref("v2").unwrap().as_deref(), Some("c2"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn 删除_revision_会清理独占_blob_与段状态() {
        let root = temp_root();
        let cache = seed(
            &root,
            "org/name",
            "c1",
            &[("only.bin", "blob-only"), ("shared.bin", "blob-shared")],
        );
        seed(&root, "org/name", "c2", &[("shared.bin", "blob-shared")]);

        let state_dir = cache.repo_dir().join(".hfmd").join("incomplete");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(state_dir.join("blob-only.json"), "{}").unwrap();
        std::fs::write(state_dir.join("blob-only.part"), "x").unwrap();

        let report = remove(&root, "org/name", Some("c1")).unwrap();

        assert_eq!(report.removed_blobs, 1);
        assert!(!cache.blob_path("blob-only").exists());
        assert!(cache.blob_path("blob-shared").exists());
        assert!(
            !state_dir.join("blob-only.json").exists(),
            "段状态应一并清理"
        );
        assert!(!state_dir.join("blob-only.part").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 非符号链接快照拒绝按_revision_删除() {
        let root = temp_root();
        let cache = seed(&root, "org/name", "c1", &[("a.bin", "blob-a")]);

        // 用普通文件替换符号链接，模拟 Windows 上退化为硬链接或复制的情况
        let target = cache.snapshot_dir("c1").join("a.bin");
        std::fs::remove_file(&target).unwrap();
        std::fs::write(&target, b"copy").unwrap();

        let err = remove(&root, "org/name", Some("c1")).expect_err("应拒绝");
        assert!(err.to_string().contains("硬链接或复制"), "实际：{err}");
        assert!(cache.repo_dir().exists(), "拒绝时不应删除任何内容");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 删除不存在的仓库报错() {
        let root = temp_root();
        assert!(remove(&root, "org/nope", None).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 目录名解析回_repo_id() {
        assert_eq!(
            repo_id_from_dir("models--org--name").as_deref(),
            Some("org/name")
        );
        assert_eq!(repo_id_from_dir("datasets--org--name"), None);
        assert_eq!(repo_id_from_dir("models--org"), None);
    }

    #[test]
    fn 文件条目类型用于构造缓存() {
        // 保证 FileEntry 的字段语义与缓存模块一致（防止重构时静默漂移）
        let entry = FileEntry {
            path: "a.bin".into(),
            size: 100,
            oid: "oid".into(),
            sha256: Some("sha".into()),
            xet_hash: None,
        };
        assert_eq!(crate::cache::blob_name(&entry).unwrap(), "sha");
    }
}
