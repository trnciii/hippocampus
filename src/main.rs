mod cli;
mod config;
mod linker;
mod vcs;

use anyhow::Result;
use cli::Cli;

fn main() -> Result<()> {
    let cli = Cli::parse_args();
    cli.run()
}
