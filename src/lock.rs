use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// 仓库级排他锁。
///
/// 锁文件放在 `<repo_dir>/.hfmd/lock`，不进 `snapshots/`。
/// 进程崩溃或文件句柄关闭时由操作系统释放，不会留下「死锁」。
#[derive(Debug)]
pub struct RepoLock {
    _file: File,
    path: PathBuf,
}

impl RepoLock {
    /// 尝试获取排他锁；已被占用时立即返回错误，不排队。
    pub fn acquire(repo_dir: &Path) -> Result<Self> {
        let dir = repo_dir.join(".hfmd");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("创建锁目录失败：{}", dir.display()))?;

        let path = dir.join("lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("打开锁文件失败：{}", path.display()))?;

        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file, path }),
            Err(TryLockError::WouldBlock) => bail!(
                "已有 hfmd 在下载该仓库（锁：{}），请等待其结束",
                path.display()
            ),
            Err(TryLockError::Error(err)) => {
                Err(err).with_context(|| format!("获取仓库锁失败：{}", path.display()))
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_repo() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("hfmd-lock-{}-{id}", std::process::id()))
    }

    #[test]
    fn 锁文件位于_hfmd_目录() {
        let repo = temp_repo();
        let lock = RepoLock::acquire(&repo).unwrap();
        assert!(lock.path().ends_with(".hfmd/lock"));
        assert!(!lock.path().to_string_lossy().contains("snapshots"));
        drop(lock);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 第二个进程式句柄拿不到锁() {
        let repo = temp_repo();
        let first = RepoLock::acquire(&repo).unwrap();

        let err = RepoLock::acquire(&repo).expect_err("第二次应失败");
        assert!(
            err.to_string().contains("已有 hfmd"),
            "错误应可操作，实际：{err}"
        );

        drop(first);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 释放后可以再次获取() {
        let repo = temp_repo();
        let first = RepoLock::acquire(&repo).unwrap();
        drop(first);

        let second = RepoLock::acquire(&repo).expect("释放后应能重新获取");
        drop(second);
        let _ = std::fs::remove_dir_all(&repo);
    }
}
