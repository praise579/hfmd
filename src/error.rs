use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Error;
use reqwest::StatusCode;

/// 错误类别。用于日志分类与决定是否重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 传输层失败：超时、连接中断、DNS。
    Network,
    /// 鉴权失败：401/403。
    Auth,
    /// 资源不存在：404。
    NotFound,
    /// 服务端错误：5xx / 429。
    Server,
    /// 服务端拒绝了字节区间：416。
    Range,
    /// 内存与磁盘不一致。
    Verify,
    /// 本地 I/O。
    Disk,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Auth => "auth",
            Self::NotFound => "not-found",
            Self::Server => "server",
            Self::Range => "range",
            Self::Verify => "verify",
            Self::Disk => "disk",
        }
    }
}

#[derive(Debug)]
pub struct HfmdError {
    pub kind: Kind,
    pub message: String,
    pub retryable: bool,
}

impl HfmdError {
    pub fn new(kind: Kind, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            kind,
            message: message.into(),
            retryable,
        }
    }
}

impl std::fmt::Display for HfmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.kind.as_str(), self.message)
    }
}

impl std::error::Error for HfmdError {}

/// 把 HTTP 状态码映射成带可操作提示的错误。
pub fn from_status(status: StatusCode) -> HfmdError {
    match status.as_u16() {
        401 | 403 => HfmdError::new(
            Kind::Auth,
            "需要 token 或权限不足（gated 仓库需先在网页上同意条款）；仓库名拼写有误时 HF 也可能返回 401，请一并确认",
            false,
        ),
        404 => HfmdError::new(Kind::NotFound, "仓库、revision 或文件不存在", false),
        416 => HfmdError::new(
            Kind::Range,
            "服务端拒绝了请求的字节区间，文件可能在下载期间被改动",
            false,
        ),
        429 => HfmdError::new(Kind::Server, "请求过于频繁，服务端要求限速", true),
        500..=599 => HfmdError::new(Kind::Server, "服务端暂时不可用", true),
        _ => HfmdError::new(Kind::Server, format!("服务端返回 HTTP {status}"), false),
    }
}

/// 从错误链里取出类别标签，用于结构化日志。
pub fn kind_of(err: &Error) -> Option<&'static str> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<HfmdError>())
        .map(|typed| typed.kind.as_str())
}

/// 错误链里只要有传输层错误就重试；类型化错误按自身标记决定。
///
/// 沿整条链查找：错误常被 `context` 包一层，只看最外层会漏掉类型化错误。
pub fn is_retryable(err: &Error) -> bool {
    if let Some(typed) = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<HfmdError>())
    {
        return typed.retryable;
    }
    err.chain()
        .any(|cause| cause.downcast_ref::<reqwest::Error>().is_some())
}

/// 指数退避 + 抖动。抖动只加不减，避免与同批任务共振。
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            ..Self::default()
        }
    }

    /// 第 `attempt` 次重试前的等待时间（attempt 从 0 开始）。
    pub fn delay(&self, attempt: u32) -> Duration {
        let factor = 1u32 << attempt.min(20);
        let exponential = self.base_delay.saturating_mul(factor).min(self.max_delay);
        exponential.saturating_add(jitter(exponential))
    }
}

/// 0 到基值 1/4 之间的抖动，用系统时间做熵源，避免引入随机数依赖。
fn jitter(delay: Duration) -> Duration {
    let window = (delay / 4).max(Duration::from_millis(1)).as_nanos() as u64;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos() as u64)
        .unwrap_or(0);
    Duration::from_nanos(nanos % window.max(1))
}

/// 按策略重试 `op`。总尝试次数为 `max_retries + 1`。
pub async fn retry<T, F, Fut>(policy: &RetryPolicy, mut op: F) -> anyhow::Result<T>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let mut attempt = 0;
    loop {
        match op(attempt).await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if !is_retryable(&err) || attempt >= policy.max_retries {
                    return Err(err);
                }
                tokio::time::sleep(policy.delay(attempt)).await;
                attempt += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn instant_policy(max_retries: u32) -> RetryPolicy {
        RetryPolicy {
            max_retries,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
        }
    }

    #[test]
    fn 状态码分类正确() {
        assert!(from_status(StatusCode::TOO_MANY_REQUESTS).retryable);
        assert!(from_status(StatusCode::BAD_GATEWAY).retryable);
        assert!(from_status(StatusCode::INTERNAL_SERVER_ERROR).retryable);
        assert!(!from_status(StatusCode::UNAUTHORIZED).retryable);
        assert!(!from_status(StatusCode::FORBIDDEN).retryable);
        assert!(!from_status(StatusCode::NOT_FOUND).retryable);
        assert_eq!(from_status(StatusCode::NOT_FOUND).kind, Kind::NotFound);
        assert_eq!(
            from_status(StatusCode::TOO_MANY_REQUESTS).kind,
            Kind::Server
        );
    }

    #[test]
    fn 只有传输层与标记为可重试的错误才重试() {
        let retryable = anyhow::Error::new(HfmdError::new(Kind::Server, "boom", true));
        let fatal = anyhow::Error::new(HfmdError::new(Kind::Auth, "denied", false));
        let plain = anyhow::anyhow!("普通错误");

        assert!(is_retryable(&retryable));
        assert!(!is_retryable(&fatal));
        assert!(!is_retryable(&plain));
    }

    #[test]
    fn 退避随次数增长并封顶() {
        let policy = RetryPolicy {
            max_retries: 8,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
        };

        assert!(policy.delay(0) >= Duration::from_millis(100));
        assert!(policy.delay(3) > policy.delay(1));
        assert_eq!(
            policy.delay(20),
            policy.max_delay + jitter(policy.max_delay)
        );
    }

    #[tokio::test]
    async fn 前两次失败第三次成功时共尝试三次() {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);

        let result = retry(&instant_policy(3), |_| {
            let counter = Arc::clone(&counter);
            async move {
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                if n < 3 {
                    Err(anyhow::Error::new(HfmdError::new(
                        Kind::Server,
                        "retry",
                        true,
                    )))
                } else {
                    Ok(n)
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(result, 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn 不可重试的错误立即返回() {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);

        let result: anyhow::Result<()> = retry(&instant_policy(5), |_| {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err(anyhow::Error::new(HfmdError::new(
                    Kind::Auth,
                    "denied",
                    false,
                )))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn 重试耗尽的尝试次数等于上限加一() {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&calls);

        let result: anyhow::Result<()> = retry(&instant_policy(2), |_| {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err(anyhow::Error::new(HfmdError::new(
                    Kind::Server,
                    "still down",
                    true,
                )))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
}
