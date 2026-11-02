use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Stable pseudonymization of names and email addresses in CSV and XLSX files.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the project key and the config.
    Init,
    /// Write pseudonymized copies of the project files.
    Run,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Init | Command::Run => {
            eprintln!("not implemented");
            ExitCode::from(2)
        }
    }
}
