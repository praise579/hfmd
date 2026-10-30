use clap::{Parser, Subcommand};

/// 从 Hugging Face 下载模型：多线程、断点续传、完整性校验。
#[derive(Debug, Parser)]
#[command(name = "hfmd", version, about)]
pub struct Cli {
    /// HF API 与下载基地址，优先于 HF_ENDPOINT
    #[arg(long, global = true, value_name = "URL")]
    pub endpoint: Option<String>,

    /// 访问令牌，优先于 HF_TOKEN
    #[arg(long, global = true, value_name = "TOKEN")]
    pub token: Option<String>,

    /// 文件间并发数（默认 4，可被配置文件覆盖）
    #[arg(long, global = true, value_name = "N")]
    pub jobs: Option<usize>,

    /// 每个文件的分片并发数（默认 8，可被配置文件覆盖）
    #[arg(long, global = true, value_name = "N")]
    pub connections: Option<usize>,

    /// 单个请求的最大重试次数（默认 3，可被配置文件覆盖）
    #[arg(long, global = true, value_name = "N")]
    pub max_retries: Option<u32>,

    /// 以 JSON 输出
    #[arg(long, global = true)]
    pub json: bool,

    /// 只输出错误
    #[arg(short, long, global = true)]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 下载仓库文件
    Download {
        /// 仓库标识，形如 org/name
        repo_id: String,

        /// 只下载指定文件（相对仓库路径）
        files: Vec<String>,

        /// 要下载的 revision
        #[arg(long, default_value = "main")]
        revision: String,

        /// 只包含匹配的文件（glob，可重复）
        #[arg(long, value_name = "GLOB")]
        include: Vec<String>,

        /// 排除匹配的文件（glob，可重复）
        #[arg(long, value_name = "GLOB")]
        exclude: Vec<String>,

        /// 跳过完整性校验
        #[arg(long)]
        skip_verify: bool,

        /// 只列出将要下载的文件与总量
        #[arg(long)]
        dry_run: bool,
    },

    /// 查看仓库文件列表
    Info {
        /// 仓库标识，形如 org/name
        repo_id: String,

        /// 要查看的 revision
        #[arg(long, default_value = "main")]
        revision: String,

        /// 只包含匹配的文件（glob，可重复）
        #[arg(long, value_name = "GLOB")]
        include: Vec<String>,

        /// 排除匹配的文件（glob，可重复）
        #[arg(long, value_name = "GLOB")]
        exclude: Vec<String>,
    },

    /// 管理本地缓存
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// 列出已缓存的仓库
    List,

    /// 删除缓存的仓库
    Remove {
        /// 仓库标识，形如 org/name
        repo_id: String,

        /// 只删除指定 revision
        #[arg(long)]
        revision: Option<String>,

        /// 跳过确认
        #[arg(short = 'y', long)]
        yes: bool,
    },
}
