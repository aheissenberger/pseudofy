//! `pseudofy run`: find the CSV and XLSX files, apply the rules, write the copies.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use globset::{GlobBuilder, GlobMatcher};
use walkdir::WalkDir;

use crate::cells::CellFns;
use crate::config::{self, Rule};
use crate::csv_file;
use crate::key;
use crate::known_names::KnownNames;
use crate::pseudonym::Pseudonymizer;
use crate::text;
use crate::xlsx_file;

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

/// A CSV or XLSX file of the project that both passes handle.
struct Item<'a> {
    path: PathBuf,
    relative: PathBuf,
    rel: String,
    csv: bool,
    rules: Vec<&'a Rule>,
}

/// Write a pseudonymized copy of each CSV and XLSX file under `project` to
/// `out`. A failed file is reported and counted in `failed`; the run goes on.
/// Excel lock files (`~$*`) are skipped.
pub fn run(project: &Path, out: &Path) -> anyhow::Result<Summary> {
    let key = key::load(project)?;
    let config = config::load(project)?;

    let (project, out) = resolve_dirs(project, out)?;

    let matchers = rule_matchers(&config.rules)?;

    let email_regex = text::email_regex()?;
    let mut pseudonymizer = Pseudonymizer::new(&key, config.language, config.email_subdomains);
    let mut summary = Summary::default();
    let mut items: Vec<Item> = Vec::new();

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
        if !entry.file_type().is_file() {
            continue;
        }
        let csv = is_csv(entry.path());
        if !csv && !is_workbook(entry.path()) {
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
        items.push(Item {
            path: entry.path().to_path_buf(),
            relative: relative.to_path_buf(),
            rel,
            csv,
            rules,
        });
    }

    // Pass 1: collect the known names. A file that fails here is skipped; pass 2
    // reports it.
    let mut known = KnownNames::default();
    let pass1 = CellFns {
        email_regex: &email_regex,
        names: text::replace_name,
        emails: text::replace_emails,
        matcher: None,
    };
    collect_known(&items, &pass1, &mut known);
    let matcher = known.matcher(config.free_text)?;
    let pass2 = CellFns {
        matcher: Some(&matcher),
        ..pass1
    };

    // Pass 2: write the copies.
    for item in &items {
        let result = write_item(item, &out, &mut pseudonymizer, &pass2);
        match result {
            Ok((replaced, free_text)) => {
                if free_text > 0 {
                    println!("{}: {free_text} free-text replacements", item.rel);
                }
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

/// The compiled file glob of each rule.
fn rule_matchers(rules: &[Rule]) -> anyhow::Result<Vec<(GlobMatcher, &Rule)>> {
    rules
        .iter()
        .map(|rule| {
            let glob = GlobBuilder::new(&rule.files)
                .case_insensitive(true)
                .literal_separator(true)
                .build()
                .with_context(|| format!("invalid glob \"{}\" in the config", rule.files))?;
            Ok((glob.compile_matcher(), rule))
        })
        .collect()
}

/// Pass 2 for one file. Returns the replaced values and the free-text replacements.
fn write_item(
    item: &Item,
    out: &Path,
    pseudonymizer: &mut Pseudonymizer,
    cells: &CellFns,
) -> anyhow::Result<(usize, usize)> {
    let target = out.join(&item.relative);
    let (path, rel, rules) = (&item.path, &item.rel, &item.rules);
    if item.csv {
        let stats = csv_file::process(path, out, &target, rel, rules, pseudonymizer, cells)?;
        Ok((stats.replaced, stats.free_text))
    } else {
        process_xlsx(path, out, &target, rel, rules, pseudonymizer, cells)
    }
}

/// Pass 1 over all files. A file that fails adds no names and is skipped.
fn collect_known(items: &[Item], cells: &CellFns, known: &mut KnownNames) {
    for item in items.iter().filter(|item| !item.rules.is_empty()) {
        let collected = if item.csv {
            csv_file::collect(&item.path, &item.rel, &item.rules, cells, known)
        } else {
            xlsx_file::collect(&item.path, &item.rel, &item.rules, cells, known)
        };
        drop(collected);
    }
}

/// Process one XLSX file and report its formula and rich-text counts.
/// Returns the number of replaced values and of free-text replacements.
fn process_xlsx(
    input: &Path,
    out: &Path,
    target: &Path,
    rel: &str,
    rules: &[&Rule],
    pseudonymizer: &mut Pseudonymizer,
    cells: &CellFns,
) -> anyhow::Result<(usize, usize)> {
    let stats = xlsx_file::process(input, out, target, rel, rules, pseudonymizer, cells)?;
    if stats.formulas > 0 {
        println!("{rel}: {} formula cells unchanged", stats.formulas);
    }
    if stats.rich_text > 0 {
        eprintln!(
            "warning: {rel}: {} rich-text cells written as plain text",
            stats.rich_text
        );
    }
    Ok((stats.replaced, stats.free_text))
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

/// An XLSX file that is not an Excel lock file.
fn is_workbook(path: &Path) -> bool {
    is_xlsx(path) && !is_lock_file(path)
}

fn is_xlsx(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("xlsx"))
}

/// Excel lock file (`~$name.xlsx`): not a workbook.
fn is_lock_file(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("~$"))
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

    #[test]
    fn xlsx_extension_and_lock_files() {
        assert!(is_xlsx(Path::new("a/b.XLSX")));
        assert!(!is_xlsx(Path::new("a/b.xls")));
        assert!(is_lock_file(Path::new("a/~$b.xlsx")));
        assert!(!is_lock_file(Path::new("a/b.xlsx")));
    }
}
