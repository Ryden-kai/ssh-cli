use clap::Parser;
use ssh_cli::cli::{run, Cli};
use ssh_cli::error::Result;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let code = run(cli)?;
    std::process::exit(code);
}
