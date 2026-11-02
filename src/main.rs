#![expect(
    dead_code,
    reason = "spec 0002 skeleton; the orchestrator removes this after the last task merge"
)]

mod config;
mod csv_file;
mod key;
mod locale;
mod names;
mod pseudonym;
mod run;
mod text;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::bail;
use clap::{Parser, Subcommand};

use crate::config::CONFIG_FILE;
use crate::key::KEY_FILE;
use crate::locale::Language;

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
    Init {
        /// Project directory.
        #[arg(long)]
        project: PathBuf,
        /// Language of the name lists. The default is the system language.
        #[arg(long, value_enum)]
        language: Option<Language>,
    },
    /// Write pseudonymized copies of the project files.
    Run {
        /// Project directory.
        #[arg(long)]
        project: PathBuf,
        /// Output directory.
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Init { project, language } => init(&project, language),
        Command::Run { project, out } => run(&project, &out),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn init(project: &Path, language: Option<Language>) -> anyhow::Result<()> {
    for file in [KEY_FILE, CONFIG_FILE] {
        if project.join(file).exists() {
            bail!("{file} exists; init does not overwrite it");
        }
    }
    let language = language.unwrap_or_else(|| {
        let detected = locale::system_language();
        if detected.fallback {
            eprintln!("warning: no name list for the system language; using \"en\"");
        }
        detected.language
    });
    key::create(project)?;
    config::create(project, language)?;
    println!(
        "created {KEY_FILE} and {CONFIG_FILE} (language = \"{}\")",
        language.code()
    );
    Ok(())
}

fn run(project: &Path, out: &Path) -> anyhow::Result<()> {
    let summary = run::run(project, out)?;
    println!(
        "files: {}, replaced values: {}, collisions: {}",
        summary.files, summary.replaced, summary.collisions
    );
    if summary.collisions > 0 {
        eprintln!("warning: two or more real values share one pseudonym");
    }
    Ok(())
}
