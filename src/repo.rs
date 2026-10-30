use anyhow::{Context, Result, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Serialize;

/// 文件树中的一个文件。`sha256` 只对 LFS 文件存在，普通文件只有 `oid`（git blob SHA-1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub oid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xet_hash: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InfoReport {
    pub repo_id: String,
    pub revision: String,
    pub total_files: usize,
    pub total_size: u64,
    pub files: Vec<FileEntry>,
}

/// 按 include / exclude 过滤。include 为空表示全部；exclude 优先。
///
/// 模式同时匹配完整路径与文件名，因此 `*.safetensors` 能命中任意子目录下的文件，
/// 与 `hf download` 的直觉一致。
pub fn filter_files(
    files: Vec<FileEntry>,
    include: &[String],
    exclude: &[String],
) -> Result<Vec<FileEntry>> {
    let include = compile_globs(include)?;
    let exclude = compile_globs(exclude)?;

    Ok(files
        .into_iter()
        .filter(|file| {
            let included = include.is_empty() || matches(&include, file);
            included && !matches(&exclude, file)
        })
        .collect())
}

pub fn report(repo_id: &str, revision: &str, files: Vec<FileEntry>) -> InfoReport {
    InfoReport {
        repo_id: repo_id.to_string(),
        revision: revision.to_string(),
        total_files: files.len(),
        total_size: files.iter().map(|file| file.size).sum(),
        files,
    }
}

/// 渲染人类可读的输出；超过 `max_lines` 时只列前若干条并提示使用 `--json`。
pub fn render(report: &InfoReport, json: bool, max_lines: usize) -> String {
    if json {
        return serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_string()) + "\n";
    }

    let mut out = format!(
        "{} @ {}\n{} 个文件，共 {}\n\n",
        report.repo_id,
        report.revision,
        report.total_files,
        format_bytes(report.total_size)
    );

    for file in report.files.iter().take(max_lines) {
        let hash = file
            .sha256
            .as_deref()
            .or(Some(file.oid.as_str()))
            .unwrap_or("");
        let short: String = hash.chars().take(12).collect();
        out.push_str(&format!(
            "{:>12}  {:<12}  {}\n",
            format_bytes(file.size),
            short,
            file.path
        ));
    }

    if report.files.len() > max_lines {
        out.push_str(&format!(
            "\n（另有 {} 个文件未显示，使用 --json 查看全部）\n",
            report.files.len() - max_lines
        ));
    }

    out
}

/// 人类可读的字节数。
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 把 `org/name` 拆成两段，用于构造仓库句柄。
pub fn split_repo_id(repo_id: &str) -> Result<(&str, &str)> {
    match repo_id.split_once('/') {
        Some((owner, name)) if !owner.is_empty() && !name.is_empty() && !name.contains('/') => {
            Ok((owner, name))
        }
        _ => bail!("仓库标识应形如 org/name，收到：{repo_id}"),
    }
}

fn compile_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).with_context(|| format!("无效的 glob：{pattern}"))?;
        builder.add(glob);
    }
    builder.build().context("编译 glob 失败")
}

fn matches(set: &GlobSet, file: &FileEntry) -> bool {
    if set.is_empty() {
        return false;
    }
    if set.is_match(&file.path) {
        return true;
    }
    std::path::Path::new(&file.path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| set.is_match(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: u64, lfs: bool) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            size,
            oid: format!("oid-{path}"),
            sha256: lfs.then(|| format!("sha-{path}")),
            xet_hash: None,
        }
    }

    fn files() -> Vec<FileEntry> {
        vec![
            entry("config.json", 700, false),
            entry("model-00001-of-00002.safetensors", 2048, true),
            entry("sub/model-00002-of-00002.safetensors", 4096, true),
            entry("tokenizer.json", 1024, false),
        ]
    }

    #[test]
    fn include_为空表示全部() {
        let out = filter_files(files(), &[], &[]).unwrap();
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn include_按文件名匹配任意子目录() {
        let out = filter_files(files(), &["*.safetensors".to_string()], &[]).unwrap();
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|file| file.path.ends_with(".safetensors")));
    }

    #[test]
    fn exclude_优先于_include() {
        let out = filter_files(
            files(),
            &["*.json".to_string()],
            &["tokenizer.json".to_string()],
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "config.json");
    }

    #[test]
    fn 无效_glob_报错并带上模式() {
        let err = filter_files(files(), &["[".to_string()], &[]).unwrap_err();
        assert!(err.to_string().contains("无效的 glob"));
    }

    #[test]
    fn 仓库标识必须是_org_斜杠_name() {
        assert_eq!(
            split_repo_id("Qwen/Qwen2.5-7B").unwrap(),
            ("Qwen", "Qwen2.5-7B")
        );
        for bad in ["Qwen", "Qwen/", "/7B", "a/b/c", ""] {
            assert!(split_repo_id(bad).is_err(), "{bad} 应当报错");
        }
    }

    #[test]
    fn 字节数格式化() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn 报告汇总文件数与总大小() {
        let report = report("org/name", "main", files());
        assert_eq!(report.total_files, 4);
        assert_eq!(report.total_size, 700 + 2048 + 4096 + 1024);
    }

    #[test]
    fn 超过上限时截断并提示_json() {
        let text = render(&report("org/name", "main", files()), false, 2);
        assert!(text.contains("另有 2 个文件未显示"));
        assert!(text.contains("--json"));
    }

    #[test]
    fn json_输出包含全部文件与哈希() {
        let text = render(&report("org/name", "main", files()), true, 2);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["total_files"], 4);
        assert!(value["files"][0]["sha256"].is_null());
        assert_eq!(
            value["files"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|file| !file["sha256"].is_null())
                .count(),
            2
        );
    }
}
