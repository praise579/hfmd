use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

pub const DEFAULT_ENDPOINT: &str = "https://huggingface.co";

/// `~/.config/hfmd/config.toml` 中可配置的默认值。
///
/// 只放 endpoint 与并发相关参数；token 不进配置文件，仍走 HF 的凭据链。
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    pub endpoint: Option<String>,
    pub jobs: Option<usize>,
    pub connections: Option<usize>,
    pub max_retries: Option<u32>,
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("hfmd").join("config.toml"))
    }

    /// 读取默认位置的配置。文件不存在时静默返回默认值。
    pub fn load() -> Result<Self> {
        match Self::path() {
            Some(path) => Self::load_from(&path),
            None => Ok(Self::default()),
        }
    }

    /// 从指定路径读取配置。文件不存在时静默返回默认值，格式错误则报错。
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("解析配置文件失败：{}", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("读取配置文件失败：{}", path.display())),
        }
    }
}

/// endpoint 优先级：命令行 > `HF_ENDPOINT` > 配置文件 > 官方站。
///
/// 空白字符串视为未设置。
pub fn resolve_endpoint(
    from_cli: Option<&str>,
    from_env: Option<&str>,
    from_config: Option<&str>,
) -> String {
    [from_cli, from_env, from_config]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or(DEFAULT_ENDPOINT)
        .to_string()
}

/// token 优先级：命令行 > `HF_TOKEN` > `~/.cache/huggingface/token`。
///
/// 空白字符串视为未设置；结果会去掉首尾空白，文件末尾的换行不会再带进请求头。
pub fn resolve_token(
    from_cli: Option<&str>,
    from_env: Option<&str>,
    from_file: Option<&str>,
) -> Option<String> {
    [from_cli, from_env, from_file]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

/// `hf login` 写入的 token 文件路径。
pub fn token_file_path() -> Option<PathBuf> {
    hf_home().map(|home| home.join("token"))
}

fn hf_home() -> Option<PathBuf> {
    match std::env::var("HF_HOME") {
        Ok(value) if !value.trim().is_empty() => Some(PathBuf::from(value)),
        _ => dirs::home_dir().map(|home| home.join(".cache").join("huggingface")),
    }
}

pub fn read_token_file() -> Option<String> {
    std::fs::read_to_string(token_file_path()?).ok()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_path() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("hfmd-config-{}-{id}.toml", std::process::id()))
    }

    #[test]
    fn endpoint_命令行优先于环境变量与配置文件() {
        assert_eq!(
            resolve_endpoint(
                Some("https://cli"),
                Some("https://env"),
                Some("https://cfg")
            ),
            "https://cli"
        );
    }

    #[test]
    fn endpoint_环境变量优先于配置文件() {
        assert_eq!(
            resolve_endpoint(None, Some("https://env"), Some("https://cfg")),
            "https://env"
        );
    }

    #[test]
    fn endpoint_配置文件优先于默认值() {
        assert_eq!(
            resolve_endpoint(None, None, Some("https://cfg")),
            "https://cfg"
        );
    }

    #[test]
    fn endpoint_全部缺失时使用官方站() {
        assert_eq!(resolve_endpoint(None, None, None), DEFAULT_ENDPOINT);
    }

    #[test]
    fn endpoint_空白字符串视为未设置() {
        assert_eq!(
            resolve_endpoint(Some("  "), Some(""), None),
            DEFAULT_ENDPOINT
        );
    }

    #[test]
    fn token_命令行优先于环境变量与文件() {
        assert_eq!(
            resolve_token(Some("cli"), Some("env"), Some("file")),
            Some("cli".to_string())
        );
    }

    #[test]
    fn token_环境变量优先于文件() {
        assert_eq!(
            resolve_token(None, Some("env"), Some("file")),
            Some("env".to_string())
        );
    }

    #[test]
    fn token_回退到文件并去掉换行() {
        assert_eq!(
            resolve_token(None, None, Some("hf_abc\n")),
            Some("hf_abc".to_string())
        );
    }

    #[test]
    fn token_全部缺失时返回空() {
        assert_eq!(resolve_token(None, None, None), None);
        assert_eq!(resolve_token(Some(" "), Some(""), None), None);
    }

    #[test]
    fn 缺失的配置文件静默返回默认值() {
        let path = temp_path();
        let config = Config::load_from(&path).expect("缺失文件不应报错");
        assert!(config.endpoint.is_none());
        assert!(config.jobs.is_none());
    }

    #[test]
    fn 配置文件可解析_endpoint_与并发() {
        let path = temp_path();
        std::fs::write(
            &path,
            "endpoint = \"https://hf-mirror.com\"\njobs = 6\nconnections = 12\nmax_retries = 5\n",
        )
        .unwrap();

        let config = Config::load_from(&path).expect("应能解析");
        let _ = std::fs::remove_file(&path);

        assert_eq!(config.endpoint.as_deref(), Some("https://hf-mirror.com"));
        assert_eq!(config.jobs, Some(6));
        assert_eq!(config.connections, Some(12));
        assert_eq!(config.max_retries, Some(5));
    }

    #[test]
    fn 配置格式错误时报错并带上路径() {
        let path = temp_path();
        std::fs::write(&path, "endpoint = [").unwrap();

        let err = Config::load_from(&path).expect_err("应报错");
        let _ = std::fs::remove_file(&path);

        assert!(err.to_string().contains("解析配置文件失败"));
    }
}
