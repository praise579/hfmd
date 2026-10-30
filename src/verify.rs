use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::repo::FileEntry;

/// 校验算法。
///
/// LFS 文件的 `lfs.oid` 是内容 sha256；普通文件的 `oid` 是 **git blob SHA-1**，
/// 即 `sha1("blob <len>\0" + 内容)`，而不是内容本身的 SHA-1。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Sha256,
    GitBlobSha1,
}

impl Algorithm {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::GitBlobSha1 => "git-blob-sha1",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    pub algorithm: Algorithm,
    pub expected: String,
    pub actual: String,
}

impl Verification {
    /// 哈希按十六进制比较，忽略大小写。
    pub fn matched(&self) -> bool {
        self.expected.eq_ignore_ascii_case(&self.actual)
    }
}

pub fn algorithm_for(file: &FileEntry) -> Result<Algorithm> {
    if file.sha256.is_some() {
        Ok(Algorithm::Sha256)
    } else if !file.oid.is_empty() {
        Ok(Algorithm::GitBlobSha1)
    } else {
        bail!("文件缺少内容哈希，无法校验：{}", file.path)
    }
}

pub fn expected_hash(file: &FileEntry) -> Result<String> {
    match algorithm_for(file)? {
        Algorithm::Sha256 => file.sha256.clone().context("LFS 文件缺少 sha256"),
        Algorithm::GitBlobSha1 => Ok(file.oid.clone()),
    }
}

/// 先比大小，再流式计算内容哈希。整个文件只读一遍。
pub fn verify_file(path: &Path, file: &FileEntry) -> Result<Verification> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("读取文件信息失败：{}", path.display()))?;
    if metadata.len() != file.size {
        bail!(
            "大小不符：{} 期望 {} 字节，实际 {} 字节",
            file.path,
            file.size,
            metadata.len()
        );
    }

    let algorithm = algorithm_for(file)?;
    let actual = hash_file(path, algorithm, metadata.len())?;

    Ok(Verification {
        algorithm,
        expected: expected_hash(file)?,
        actual,
    })
}

/// `size` 只对 git blob SHA-1 有意义：它要拼进 `blob <len>\0` 前缀。
pub fn hash_file(path: &Path, algorithm: Algorithm, size: u64) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("打开文件失败：{}", path.display()))?;

    match algorithm {
        Algorithm::Sha256 => {
            let mut hasher = Sha256::new();
            read_into(&mut file, path, |chunk| hasher.update(chunk))?;
            Ok(to_hex(&hasher.finalize()))
        }
        Algorithm::GitBlobSha1 => {
            let mut hasher = Sha1::new();
            hasher.update(format!("blob {size}\0").as_bytes());
            read_into(&mut file, path, |chunk| hasher.update(chunk))?;
            Ok(to_hex(&hasher.finalize()))
        }
    }
}

fn read_into(file: &mut File, path: &Path, mut consume: impl FnMut(&[u8])) -> Result<()> {
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("读取文件失败：{}", path.display()))?;
        if read == 0 {
            return Ok(());
        }
        consume(&buffer[..read]);
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_file(content: &[u8]) -> std::path::PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("hfmd-verify-{}-{id}", std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }

    fn entry(size: u64, sha256: Option<&str>, oid: &str) -> FileEntry {
        FileEntry {
            path: "x".to_string(),
            size,
            oid: oid.to_string(),
            sha256: sha256.map(str::to_string),
            xet_hash: None,
        }
    }

    #[test]
    fn 内容_sha256_正确() {
        let path = temp_file(b"hello");
        assert_eq!(
            hash_file(&path, Algorithm::Sha256, 5).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn git_blob_sha1_带_blob_头而不是内容摘要() {
        let path = temp_file(b"hello");
        assert_eq!(
            hash_file(&path, Algorithm::GitBlobSha1, 5).unwrap(),
            "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0"
        );
        // 内容本身的 sha1 不应匹配
        assert_ne!(
            hash_file(&path, Algorithm::GitBlobSha1, 5).unwrap(),
            "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn lfs_文件用_sha256_普通文件用_git_blob_sha1() {
        assert_eq!(
            algorithm_for(&entry(5, Some("sha"), "oid")).unwrap(),
            Algorithm::Sha256
        );
        assert_eq!(
            algorithm_for(&entry(5, None, "oid")).unwrap(),
            Algorithm::GitBlobSha1
        );
        assert!(algorithm_for(&entry(5, None, "")).is_err());
    }

    #[test]
    fn 校验通过时_matched_为真() {
        let path = temp_file(b"hello");
        let file = entry(
            5,
            Some("2CF24DBA5FB0A30E26E83B2AC5B9E29E1B161E5C1FA7425E73043362938B9824"),
            "ignored",
        );

        let verification = verify_file(&path, &file).unwrap();
        assert!(verification.matched());
        assert_eq!(verification.algorithm, Algorithm::Sha256);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 大小不符直接报错() {
        let path = temp_file(b"hello!");
        let file = entry(
            5,
            Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"),
            "",
        );
        assert!(verify_file(&path, &file).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 内容被篡改时_matched_为假() {
        let path = temp_file(b"hellp");
        let file = entry(
            5,
            Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"),
            "",
        );
        assert!(!verify_file(&path, &file).unwrap().matched());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 普通文件用_git_blob_sha1_校验() {
        let path = temp_file(b"hello");
        let file = entry(5, None, "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0");

        let verification = verify_file(&path, &file).unwrap();
        assert!(verification.matched());
        assert_eq!(verification.algorithm, Algorithm::GitBlobSha1);
        let _ = std::fs::remove_file(&path);
    }
}
