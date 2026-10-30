use anyhow::{Context, Result, anyhow, bail};
use reqwest::header::{self, HeaderMap};
use reqwest::{Client, Response, Url};
use serde::Deserialize;

use crate::repo::FileEntry;

/// Hugging Face Hub 的 HTTP 客户端。
///
/// 只覆盖 v0.1 需要的端点：仓库文件树。鉴权与错误提示集中在这里，
/// 避免每个调用点重复判断 401/403/404。
pub struct Api {
    http: Client,
    endpoint: String,
    token: Option<String>,
}

impl Api {
    pub fn new(endpoint: &str, token: Option<&str>) -> Result<Self> {
        let http = Client::builder()
            .user_agent(format!("hfmd/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .context("构造 HTTP 客户端失败")?;

        Ok(Self {
            http,
            endpoint: endpoint.trim_end_matches('/').to_string(),
            token: token.map(str::to_string),
        })
    }

    /// 列出仓库在某个 revision 下的全部文件，按路径排序。
    ///
    /// 自己解析 `lfs.oid` 而不是复用 `hf-hub`：后者把该字段映射成 `sha256` 却没有 serde rename，
    /// 导致 LFS 文件的 sha256 恒为 `None`。
    pub async fn list_files(&self, repo_id: &str, revision: &str) -> Result<Vec<FileEntry>> {
        // 先校验形状，避免把 "Qwen" 这种输入变成一个费解的 404。
        crate::repo::split_repo_id(repo_id)?;

        let mut url = tree_url(&self.endpoint, repo_id, revision)?;
        let mut files = Vec::new();

        loop {
            let response = self.get(url.clone()).await?;
            let next = next_link(response.headers());
            let batch: Vec<TreeEntry> = response
                .json()
                .await
                .with_context(|| format!("解析文件树响应失败：{url}"))?;

            files.extend(batch.into_iter().filter_map(TreeEntry::into_file));

            match next {
                Some(next) => url = next,
                None => break,
            }
        }

        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    async fn get(&self, url: Url) -> Result<Response> {
        let mut request = self.http.get(url.clone());
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("请求失败：{url}"))?;

        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        let hint = match status.as_u16() {
            401 | 403 => "需要 token 或权限不足（gated 仓库需先在网页上同意条款）",
            404 => "仓库或 revision 不存在",
            _ => "服务端返回错误",
        };
        bail!("请求 {url} 失败：HTTP {status}，{hint}")
    }
}

/// 构造 `{endpoint}/api/models/{repo}/tree/{revision}?recursive=true&expand=true`。
fn tree_url(endpoint: &str, repo_id: &str, revision: &str) -> Result<Url> {
    let mut url =
        Url::parse(endpoint).with_context(|| format!("endpoint 不是合法 URL：{endpoint}"))?;

    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow!("endpoint 不能作为基地址：{endpoint}"))?;
        segments.pop_if_empty();
        segments.push("api").push("models");
        for part in repo_id.split('/').filter(|part| !part.is_empty()) {
            segments.push(part);
        }
        segments.push("tree");
        for part in revision.split('/').filter(|part| !part.is_empty()) {
            segments.push(part);
        }
    }

    url.query_pairs_mut()
        .append_pair("recursive", "true")
        .append_pair("expand", "true");

    Ok(url)
}

/// 从 `Link` 头里取 `rel="next"` 的地址；没有下一页时返回 `None`。
fn next_link(headers: &HeaderMap) -> Option<Url> {
    let value = headers.get(header::LINK)?.to_str().ok()?;

    for entry in value.split(',') {
        let mut parts = entry.split(';');
        let target = parts.next()?.trim();
        let is_next = parts.any(|param| {
            let param: String = param.chars().filter(|c| !c.is_whitespace()).collect();
            param.eq_ignore_ascii_case("rel=\"next\"")
        });
        if is_next {
            let target = target.trim_start_matches('<').trim_end_matches('>');
            return Url::parse(target).ok();
        }
    }

    None
}

#[derive(Debug, Deserialize)]
struct TreeEntry {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    oid: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    path: String,
    #[serde(default)]
    lfs: Option<LfsInfo>,
    #[serde(default, rename = "xetHash")]
    xet_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LfsInfo {
    #[serde(default)]
    oid: String,
    #[serde(default)]
    size: Option<u64>,
}

impl TreeEntry {
    fn into_file(self) -> Option<FileEntry> {
        if self.kind != "file" {
            return None;
        }

        let sha256 = self.lfs.as_ref().and_then(|lfs| {
            if lfs.oid.is_empty() {
                None
            } else {
                Some(lfs.oid.clone())
            }
        });
        let size = self
            .lfs
            .as_ref()
            .and_then(|lfs| lfs.size)
            .unwrap_or(self.size);

        Some(FileEntry {
            path: self.path,
            size,
            oid: self.oid,
            sha256,
            xet_hash: self.xet_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_url_处理结尾斜杠与多级路径() {
        let url = tree_url("https://hf-mirror.com/", "Qwen/Qwen2.5-7B", "main").unwrap();
        assert_eq!(
            url.as_str(),
            "https://hf-mirror.com/api/models/Qwen/Qwen2.5-7B/tree/main?recursive=true&expand=true"
        );
    }

    #[test]
    fn tree_url_支持带斜杠的_revision() {
        let url = tree_url("https://huggingface.co", "org/name", "refs/pr/1").unwrap();
        assert_eq!(url.path(), "/api/models/org/name/tree/refs/pr/1");
    }

    #[test]
    fn tree_url_拒绝非法_endpoint() {
        assert!(tree_url("not a url", "org/name", "main").is_err());
    }

    #[test]
    fn 解析_link_头的下一页() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::LINK,
            "<https://huggingface.co/api/models/o/n/tree/main?cursor=abc>; rel=\"next\", <https://x>; rel=\"prev\""
                .parse()
                .unwrap(),
        );

        let next = next_link(&headers).unwrap();
        assert_eq!(
            next.as_str(),
            "https://huggingface.co/api/models/o/n/tree/main?cursor=abc"
        );
    }

    #[test]
    fn 没有_link_头时返回_none() {
        assert!(next_link(&HeaderMap::new()).is_none());
    }

    #[test]
    fn lfs_文件取_lfs_oid_作为_sha256() {
        let json = r#"{
            "type": "file",
            "oid": "5e68dcea92e3696ddd4e1d44d9b420a31fe5622c",
            "size": 3945441440,
            "path": "model.safetensors",
            "lfs": {"oid": "a1333e6293854747c481288ea83b348226af178dd565c49b6f9495ba1966aba7", "size": 3945441440, "pointerSize": 134}
        }"#;

        let file = serde_json::from_str::<TreeEntry>(json)
            .unwrap()
            .into_file()
            .unwrap();
        assert_eq!(file.path, "model.safetensors");
        assert_eq!(file.size, 3945441440);
        assert_eq!(
            file.sha256.as_deref(),
            Some("a1333e6293854747c481288ea83b348226af178dd565c49b6f9495ba1966aba7")
        );
        assert_eq!(file.oid, "5e68dcea92e3696ddd4e1d44d9b420a31fe5622c");
    }

    #[test]
    fn 普通文件没有_sha256() {
        let json = r#"{"type": "file", "oid": "abc123", "size": 663, "path": "config.json"}"#;
        let file = serde_json::from_str::<TreeEntry>(json)
            .unwrap()
            .into_file()
            .unwrap();
        assert_eq!(file.sha256, None);
        assert_eq!(file.oid, "abc123");
    }

    #[test]
    fn 目录条目被跳过() {
        let json = r#"{"type": "directory", "oid": "dir", "path": "sub", "size": 0}"#;
        assert!(
            serde_json::from_str::<TreeEntry>(json)
                .unwrap()
                .into_file()
                .is_none()
        );
    }
}
