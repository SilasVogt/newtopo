//! `newtopo-cli`: headless driver for the core (SPEC §10). One module per
//! subcommand; `run`, `validate`, `export` and `render` come later.

use clap::{Parser, Subcommand};
use std::process::ExitCode;

mod info;

#[derive(Parser, Debug)]
#[command(name = "newtopo-cli", version, about = "newtopo headless CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print bodies, faces, surface types, edges, layers, colours and the heal report.
    Info(info::InfoArgs),
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    let res = match cli.command {
        Command::Info(a) => info::run(&a),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
