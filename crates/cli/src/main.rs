//! `newtopo-cli`: headless driver for the core (SPEC §10). One module per
//! subcommand.

use clap::{Parser, Subcommand};
use std::process::ExitCode;

mod export;
mod info;
mod render;
mod run;
mod scene_io;
mod validate;

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
    /// Apply a JSON op script and optionally save the scene.
    Run(run::RunArgs),
    /// Check output-mesh invariants; non-zero exit on failure.
    Validate(validate::ValidateArgs),
    /// Export the output mesh as FBX, OBJ or glTF (.glb).
    Export(export::ExportArgs),
    /// Render a PNG headless.
    Render(render::RenderArgs),
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    let res = match cli.command {
        Command::Info(a) => info::run(&a),
        Command::Run(a) => run::run(&a),
        Command::Validate(a) => validate::run(&a),
        Command::Export(a) => export::run(&a),
        Command::Render(a) => render::run(&a),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
