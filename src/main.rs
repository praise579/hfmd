mod cli;

use anyhow::{Result, bail};
use clap::Parser;
use cli::{CacheCommand, Cli, Command};
use hfmd::api::Api;
use hfmd::config::{self, resolve_endpoint, resolve_token};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = config::Config::load()?;

    let env_endpoint = std::env::var("HF_ENDPOINT").ok();
    let env_token = std::env::var("HF_TOKEN").ok();
    let file_token = config::read_token_file();

    let endpoint = resolve_endpoint(
        cli.endpoint.as_deref(),
        env_endpoint.as_deref(),
        config.endpoint.as_deref(),
    );
    let token = resolve_token(
        cli.token.as_deref(),
        env_token.as_deref(),
        file_token.as_deref(),
    );
    match cli.command {
        Command::Info {
            repo_id,
            revision,
            include,
            exclude,
        } => {
            let api = Api::new(&endpoint, token.as_deref())?;
            let files = api.list_files(&repo_id, &revision).await?;
            let files = hfmd::repo::filter_files(files, &include, &exclude)?;
            let report = hfmd::repo::report(&repo_id, &revision, files);
            print!("{}", hfmd::repo::render(&report, cli.json, 50));
            Ok(())
        }
        Command::Download { .. } => bail!("download 尚未实现（见 #7）"),
        Command::Cache { command } => match command {
            CacheCommand::List => bail!("cache list 尚未实现（见 #11）"),
            CacheCommand::Remove { .. } => bail!("cache remove 尚未实现（见 #11）"),
        },
    }
}
