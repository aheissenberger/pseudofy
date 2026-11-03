mod cells;
mod config;
mod csv_file;
mod key;
mod known_names;
mod locale;
mod names;
mod panic_guard;
mod pseudonym;
mod run;
mod text;
mod xlsx_file;
mod xlsx_scrub;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::bail;
use clap::{Parser, Subcommand};

use crate::config::CONFIG_FILE;
use crate::key::KEY_FILE;
use crate::locale::Language;
use crate::pseudonym::Collisions;

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
    panic_guard::install_hook();
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
    if let Err(error) = config::create(project, language) {
        // Remove the new key so that a later `init` can start again.
        let _ = std::fs::remove_file(project.join(KEY_FILE));
        return Err(error);
    }
    println!(
        "created {KEY_FILE} and {CONFIG_FILE} (language = \"{}\")",
        language.code()
    );
    Ok(())
}

/// The warning for the collisions of a run, by kind (spec 0022).
fn collision_warning(c: Collisions) -> Option<String> {
    (c.total() > 0).then(|| {
        format!(
            "warning: {} collisions (persons: {}, emails: {}, email domains: {}): \
             different real values got one pseudonym by chance",
            c.total(),
            c.persons,
            c.emails,
            c.domains
        )
    })
}

fn run(project: &Path, out: &Path) -> anyhow::Result<()> {
    let summary = run::run(project, out)?;
    let copied = summary
        .copied
        .map(|n| format!(", copied unchanged: {n}"))
        .unwrap_or_default();
    let excluded = if summary.excluded > 0 {
        format!(", excluded: {}", summary.excluded)
    } else {
        String::new()
    };
    println!(
        "files: {}, replaced values: {}, collisions: {}{copied}{excluded}",
        summary.files,
        summary.replaced,
        summary.collisions.total()
    );
    if let Some(warning) = collision_warning(summary.collisions) {
        eprintln!("{warning}");
    }
    if summary.failed > 0 {
        bail!("{} file(s) failed; see the messages above", summary.failed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collision_warning_gives_each_kind_and_the_cause() {
        assert_eq!(collision_warning(Collisions::default()), None);
        let c = Collisions {
            persons: 16,
            emails: 1,
            domains: 0,
        };
        assert_eq!(
            collision_warning(c).unwrap(),
            "warning: 17 collisions (persons: 16, emails: 1, email domains: 0): \
             different real values got one pseudonym by chance"
        );
    }
}
