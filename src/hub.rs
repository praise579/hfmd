use anyhow::{Context, Result};
use hf_hub::HFClient;

/// 用已解析的 endpoint 与 token 构造 HF 客户端。
///
/// 参数全部显式传入，不依赖 hf-hub 自己的环境变量回退，保证行为可预测、可测试。
pub fn client(endpoint: &str, token: Option<&str>, max_retries: u32) -> Result<HFClient> {
    let mut builder = HFClient::builder()
        .endpoint(endpoint)
        .retry_max_attempts(max_retries as usize);

    if let Some(token) = token {
        builder = builder.token(token);
    }

    builder
        .build()
        .with_context(|| format!("构造 HF 客户端失败，endpoint = {endpoint}"))
}
