use anyhow::Result;

mod cli;
mod commands;
mod pipeline;

fn main() -> Result<()> {
    let cmd = cli::build_cli();
    let matches = cmd.get_matches();
    commands::dispatch(matches)
}
