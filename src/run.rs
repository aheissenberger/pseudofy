//! `pseudofy run`: find the files, apply the rules, write the copies.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use globset::GlobBuilder;
use walkdir::WalkDir;

use crate::config::{self, Rule};
use crate::csv_file;
use crate::key;
use crate::pseudonym::Pseudonymizer;
use crate::text;

/// Counts for the summary line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    pub files: usize,
    pub replaced: usize,
    pub collisions: usize,
    /// Files that could not be read or written. `run` reports each one and
    /// continues with the other files.
    pub failed: usize,
}

/// Write a pseudonymized copy of each CSV file under `project` to `out`.
pub fn run(project: &Path, out: &Path) -> anyhow::Result<Summary> {
    let key = key::load(project)?;
    let config = config::load(project)?;

    let (project, out) = resolve_dirs(project, out)?;

    let matchers = config
        .rules
        .iter()
        .map(|rule| {
            let glob = GlobBuilder::new(&rule.files)
                .case_insensitive(true)
                .literal_separator(true)
                .build()
                .with_context(|| format!("invalid glob \"{}\" in the config", rule.files))?;
            Ok((glob.compile_matcher(), rule))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let email_regex = text::email_regex()?;
    let mut pseudonymizer = Pseudonymizer::new(&key, config.language, config.email_subdomains);
    let mut summary = Summary::default();

    let secrets = project.join("secrets");
    let config_dir = project.join("config");
    let walker = WalkDir::new(&project)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            let path = entry.path();
            path != secrets && path != config_dir && path != out
        });
    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                let rel = error
                    .path()
                    .and_then(|path| path.strip_prefix(&project).ok())
                    .map_or_else(|| ".".to_string(), relative_text);
                eprintln!("error: cannot read {rel} in the project directory");
                summary.failed += 1;
                continue;
            }
        };
        let relative = entry
            .path()
            .strip_prefix(&project)
            .context("file outside the project")?;
        let rel = relative_text(relative);
        if entry.path_is_symlink() {
            eprintln!("warning: {rel}: symlink skipped");
            continue;
        }
        if !entry.file_type().is_file() || !is_csv(entry.path()) {
            continue;
        }
        let rules: Vec<&Rule> = matchers
            .iter()
            .filter(|(matcher, _)| matcher.is_match(&rel))
            .map(|(_, rule)| *rule)
            .collect();
        if rules.is_empty() {
            eprintln!("warning: {rel}: no rule matches; only emails are replaced");
        }
        let target = out.join(relative);
        match csv_file::process(
            entry.path(),
            &out,
            &target,
            &rel,
            &rules,
            &mut pseudonymizer,
            &email_regex,
        ) {
            Ok(replaced) => {
                summary.replaced += replaced;
                summary.files += 1;
            }
            Err(error) => {
                eprintln!("error: {error:#}");
                summary.failed += 1;
            }
        }
    }
    summary.collisions = pseudonymizer.collisions();
    Ok(summary)
}

/// Canonical project and output directories. Fail if the output could
/// overwrite or contain the input, or lies in `secrets/` or `config/`.
fn resolve_dirs(project: &Path, out: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let project = project
        .canonicalize()
        .context("cannot open the project directory")?;
    let future = resolve_future(out)?;
    if future.starts_with(project.join("secrets")) || future.starts_with(project.join("config")) {
        bail!("--out must not be inside secrets/ or config/ of the project");
    }
    fs::create_dir_all(out).context("cannot create the output directory")?;
    let out = out
        .canonicalize()
        .context("cannot open the output directory")?;
    if out == project {
        bail!("--out must not be the project directory");
    }
    if project.starts_with(&out) {
        bail!("the project directory must not be inside --out");
    }
    Ok((project, out))
}

/// Absolute form of a path that may not exist yet: the nearest existing
/// ancestor is canonical, the missing rest is appended.
fn resolve_future(path: &Path) -> anyhow::Result<PathBuf> {
    let absolute = std::path::absolute(path).context("cannot resolve the output directory")?;
    let mut rest = Vec::new();
    let mut existing = absolute.as_path();
    loop {
        if existing.exists() {
            break;
        }
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => bail!("cannot resolve the output directory"),
        }
    }
    let mut resolved = existing
        .canonicalize()
        .context("cannot resolve the output directory")?;
    for name in rest.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn is_csv(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("csv"))
}

/// Relative path with `/` separators.
fn relative_text(relative: &Path) -> String {
    relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_equal_to_project_fails() {
        let dir = tempfile::tempdir().unwrap();
        assert!(resolve_dirs(dir.path(), dir.path()).is_err());
    }

    #[test]
    fn project_inside_out_fails() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        assert!(resolve_dirs(&project, dir.path()).is_err());
    }

    #[test]
    fn out_inside_project_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let (project, out) = resolve_dirs(dir.path(), &out).unwrap();
        assert!(out.starts_with(&project));
    }

    #[test]
    fn sibling_out_is_allowed() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        assert!(resolve_dirs(&project, &dir.path().join("out")).is_ok());
    }

    #[test]
    fn out_inside_secrets_or_config_fails_without_creating_it() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["secrets", "config"] {
            let out = dir.path().join(name).join("deep").join("out");
            assert!(resolve_dirs(dir.path(), &out).is_err());
            assert!(!dir.path().join(name).exists());
        }
        let out = dir.path().join("secrets2");
        assert!(resolve_dirs(dir.path(), &out).is_ok());
    }

    #[test]
    fn globs_are_case_insensitive_with_literal_separator() {
        let matcher = |pattern: &str| {
            GlobBuilder::new(pattern)
                .case_insensitive(true)
                .literal_separator(true)
                .build()
                .unwrap()
                .compile_matcher()
        };
        assert!(matcher("*.csv").is_match("People.CSV"));
        assert!(!matcher("*.csv").is_match("a/people.csv"));
        assert!(matcher("**/*.csv").is_match("a/People.Csv"));
    }

    #[test]
    fn csv_extension_is_case_insensitive() {
        assert!(is_csv(Path::new("a/b.CSV")));
        assert!(!is_csv(Path::new("a/b.xlsx")));
        assert_eq!(relative_text(Path::new("a/b.csv")), "a/b.csv");
    }
}
