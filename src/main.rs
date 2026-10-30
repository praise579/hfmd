mod cli;

use clap::Parser;
use cli::{CacheCommand, Cli, Command};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Download { .. } => anyhow::bail!("download 尚未实现（见 #7）"),
        Command::Info { .. } => anyhow::bail!("info 尚未实现（见 #3）"),
        Command::Cache { command } => match command {
            CacheCommand::List => anyhow::bail!("cache list 尚未实现（见 #11）"),
            CacheCommand::Remove { .. } => anyhow::bail!("cache remove 尚未实现（见 #11）"),
        },
    }
}
