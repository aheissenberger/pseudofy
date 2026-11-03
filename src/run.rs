//! `pseudofy run`: find the CSV and XLSX files, apply the rules, write the copies.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use globset::{GlobBuilder, GlobMatcher};
use walkdir::WalkDir;

use crate::cells::{CellFns, is_not_found};
use crate::config::{self, FreeText, Rule};
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
    /// Other files copied unchanged. `None` if `copy_other_files` is off (spec 0014).
    pub copied: Option<usize>,
}

/// Name of the marker file in an output directory inside the project.
const OUT_MARKER: &str = ".pseudofy-out";
/// The exact content of the marker file. Another entry with the marker name
/// (a directory, a symlink, an empty file) does not mark the directory.
const OUT_MARKER_TEXT: &[u8] = b"pseudofy output directory\n";

/// Identity of a file: device and inode on unix, the canonical path elsewhere.
#[cfg(unix)]
type FileId = (u64, u64);
#[cfg(not(unix))]
type FileId = PathBuf;

/// Identity of an existing file or directory. A symlink is followed.
#[cfg(unix)]
fn file_id(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).ok().map(|meta| (meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(path: &Path) -> Option<FileId> {
    path.canonicalize().ok()
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
    let mut others: Vec<(PathBuf, PathBuf, String)> = Vec::new();
    summary.copied = config.copy_other_files.then_some(0);

    // `secrets/` and `config/` are compared without case: on a case-insensitive
    // file system (the macOS default), `Secrets/` is the same directory.
    let walker = WalkDir::new(&project)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            let top_dir = entry.depth() == 1
                && ["secrets", "config"]
                    .iter()
                    .any(|dir| entry.file_name().eq_ignore_ascii_case(dir));
            // A path with a component that starts with `.` is hidden: skipped.
            let hidden = entry.depth() > 0 && entry.file_name().to_string_lossy().starts_with('.');
            !top_dir && !hidden && entry.path() != out
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
            if config.copy_other_files && is_copyable_other(entry.path(), relative) {
                others.push((entry.path().to_path_buf(), relative.to_path_buf(), rel));
            }
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
    // With `free_text = "off"`, pass 1 does not run.
    let mut known = KnownNames::default();
    let pass1 = CellFns {
        email_regex: &email_regex,
        names: text::replace_name,
        emails: text::replace_emails,
        matcher: None,
    };
    let free_text = config.free_text != FreeText::Off;
    if free_text {
        collect_known(&items, &pass1, &mut known);
    }
    let matcher = known.matcher(config.free_text);
    let pass2 = CellFns {
        matcher: free_text.then_some(&matcher),
        ..pass1
    };

    let inputs = input_ids(&items, &others);

    // Pass 2: write the copies.
    write_items(
        &items,
        &out,
        &inputs,
        &mut pseudonymizer,
        &pass2,
        &mut summary,
    );
    summary.collisions = pseudonymizer.collisions();

    copy_others(&others, &out, &inputs, &mut summary);
    Ok(summary)
}

/// Identities of the walked input files: an output path must not be one of them.
fn input_ids(items: &[Item], others: &[(PathBuf, PathBuf, String)]) -> HashSet<FileId> {
    items
        .iter()
        .map(|item| &item.path)
        .chain(others.iter().map(|(path, _, _)| path))
        .filter_map(|path| file_id(path))
        .collect()
}

/// Pass 2 over all files. A failed file is reported and counted.
fn write_items(
    items: &[Item],
    out: &Path,
    inputs: &HashSet<FileId>,
    pseudonymizer: &mut Pseudonymizer,
    cells: &CellFns,
    summary: &mut Summary,
) {
    for item in items {
        let result = write_item(item, out, inputs, pseudonymizer, cells);
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
    inputs: &HashSet<FileId>,
    pseudonymizer: &mut Pseudonymizer,
    cells: &CellFns,
) -> anyhow::Result<(usize, usize)> {
    let target = out.join(&item.relative);
    let (path, rel, rules) = (&item.path, &item.rel, &item.rules);
    if is_input(&target, inputs) {
        bail!("{rel}: the output path is an input file");
    }
    if item.csv {
        csv_file::process(path, out, &target, rel, rules, pseudonymizer, cells)
            .map(|stats| (stats.replaced, stats.free_text))
    } else {
        process_xlsx(path, out, &target, rel, rules, pseudonymizer, cells)
    }
}

/// Whether `target` exists and is one of the walked input files.
fn is_input(target: &Path, inputs: &HashSet<FileId>) -> bool {
    file_id(target).is_some_and(|id| inputs.contains(&id))
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
/// An output directory inside the project must be new, empty, or marked.
fn resolve_dirs(project: &Path, out: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let project = project
        .canonicalize()
        .context("cannot open the project directory")?;
    let protected: Vec<PathBuf> = ["secrets", "config"]
        .iter()
        .filter_map(|name| project.join(name).canonicalize().ok())
        .collect();
    let future = resolve_future(out)?;
    check_not_protected(&project, &protected, &future)?;
    let needs_marker = future.starts_with(&project) && future != project && marker_needed(&future)?;
    let existed = out.exists();
    create_private_dir(out)?;
    let out = out
        .canonicalize()
        .context("cannot open the output directory")?;
    check_not_protected(&project, &protected, &out)?;
    if out == project {
        bail!("--out must not be the project directory");
    }
    if project.starts_with(&out) {
        bail!("the project directory must not be inside --out");
    }
    if needs_marker {
        fs::write(out.join(OUT_MARKER), OUT_MARKER_TEXT)
            .context("cannot write the marker file in the output directory")?;
    }
    // A directory that pseudofy made is private (0700); warn only for an old one.
    if existed {
        warn_if_writable_by_others(&out);
    }
    Ok((project, out))
}

/// Create `out` and its missing parents. A new directory gets mode 0700 (unix).
fn create_private_dir(out: &Path) -> anyhow::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(out)
        .context("cannot create the output directory")
}

/// Fail if `path` is, or lies in, `secrets/` or `config/` of the project.
/// The check uses the name (any letter case), the canonical path, and the
/// device and inode of each existing ancestor, so a symlink does not get past.
fn check_not_protected(project: &Path, protected: &[PathBuf], path: &Path) -> anyhow::Result<()> {
    let by_name = path.strip_prefix(project).is_ok_and(|rest| {
        rest.components().next().is_some_and(|first| {
            ["secrets", "config"]
                .iter()
                .any(|dir| first.as_os_str().eq_ignore_ascii_case(dir))
        })
    });
    let by_path = protected.iter().any(|dir| path.starts_with(dir));
    let ids: Vec<FileId> = protected.iter().filter_map(|dir| file_id(dir)).collect();
    let by_id = path
        .ancestors()
        .filter_map(file_id)
        .any(|id| ids.contains(&id));
    if by_name || by_path || by_id {
        bail!("--out must not be inside secrets/ or config/ of the project");
    }
    Ok(())
}

/// For an output directory inside the project: does the run write the marker?
/// Yes if the directory is new or empty. No if it has the marker. Fail if it
/// has other content (it may hold input files).
fn marker_needed(dir: &Path) -> anyhow::Result<bool> {
    match fs::metadata(dir) {
        Err(error) if is_not_found(&error) => Ok(true),
        Ok(meta) if meta.is_dir() => {
            if is_marker(&dir.join(OUT_MARKER)) {
                return Ok(false);
            }
            let mut entries = fs::read_dir(dir).context("cannot read the output directory")?;
            if entries.next().is_none() {
                Ok(true)
            } else {
                bail!(
                    "--out is inside the project and not empty; it needs the marker file \
                     {OUT_MARKER}, or use an empty or new directory"
                )
            }
        }
        Ok(_) => bail!("--out is not a directory"),
        Err(_) => bail!("cannot check the output directory"),
    }
}

/// True if `path` is a regular file (not a symlink) with the marker text.
fn is_marker(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
        && fs::read(path).is_ok_and(|bytes| bytes == OUT_MARKER_TEXT)
}

/// Warn if other users can write to the output directory (unix).
fn warn_if_writable_by_others(out: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(out).is_ok_and(|meta| meta.permissions().mode() & 0o022 != 0) {
            eprintln!("warning: --out is writable by other users");
        }
    }
    #[cfg(not(unix))]
    let _ = out;
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

/// Copy each other file unchanged and count it. A failure is reported and
/// counted in `failed`.
fn copy_others(
    others: &[(PathBuf, PathBuf, String)],
    out: &Path,
    inputs: &HashSet<FileId>,
    summary: &mut Summary,
) {
    for (path, relative, rel) in others {
        let target = out.join(relative);
        if !is_input(&target, inputs) && copy_file(path, out, &target).is_ok() {
            println!("{rel}: copied unchanged");
            summary.copied = summary.copied.map(|n| n + 1);
        } else {
            eprintln!("error: cannot copy {rel}");
            summary.failed += 1;
        }
    }
}

/// A file that `copy_other_files` copies: not CSV, not XLSX, not an Excel lock
/// file (`~$*`), and no path component starts with `.`.
fn is_copyable_other(path: &Path, relative: &Path) -> bool {
    !is_csv(path)
        && !is_xlsx(path)
        && !is_lock_file(path)
        && !relative
            .components()
            .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
}

/// Stream `input` to `output` without following a symlink in the output
/// directory: check each directory below `out_root`, refuse a symlink target,
/// write a temp file, rename it. The error carries no detail.
fn copy_file(input: &Path, out_root: &Path, output: &Path) -> Result<(), ()> {
    let parent = output.parent().unwrap_or(out_root);
    let below = parent
        .strip_prefix(out_root)
        .unwrap_or_else(|_| Path::new(""));
    let mut current = out_root.to_path_buf();
    for part in below.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Err(error) if is_not_found(&error) => fs::create_dir(&current).map_err(drop)?,
            Ok(_) | Err(_) => return Err(()),
        }
    }
    match fs::symlink_metadata(output) {
        Ok(meta) if meta.file_type().is_symlink() || meta.is_dir() => return Err(()),
        Ok(_) => {}
        Err(error) if is_not_found(&error) => {}
        Err(_) => return Err(()),
    }
    // A short fixed name: a long but legal file name still fits.
    let temp = parent.join(".pseudofy-copy.tmp");
    match fs::remove_file(&temp) {
        Ok(()) => {}
        Err(error) if is_not_found(&error) => {}
        Err(_) => return Err(()),
    }
    let mut source = fs::File::open(input).map_err(drop)?;
    let permissions = source.metadata().map_err(drop)?.permissions();
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .and_then(|mut file| {
            // Keep the permission bits: a private file stays private.
            file.set_permissions(permissions)?;
            io::copy(&mut source, &mut file)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temp, output));
    if written.is_err() {
        drop(fs::remove_file(&temp));
    }
    written.map_err(drop)
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

    #[cfg(unix)]
    #[test]
    fn new_out_is_private_and_gets_no_warning() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("p");
        fs::create_dir(&project).unwrap();
        let out = dir.path().join("o/new");
        resolve_dirs(&project, &out).unwrap();
        let mode = fs::metadata(&out).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn out_inside_project_needs_new_empty_or_marked_dir() {
        let dir = tempfile::tempdir().unwrap();
        let used = dir.path().join("data");
        fs::create_dir(&used).unwrap();
        fs::write(used.join("a.csv"), "x").unwrap();
        assert!(resolve_dirs(dir.path(), &used).is_err());
        assert!(!used.join(OUT_MARKER).exists());
        // Only a regular file with the marker text marks the directory.
        fs::write(used.join(OUT_MARKER), "").unwrap();
        assert!(resolve_dirs(dir.path(), &used).is_err());
        fs::remove_file(used.join(OUT_MARKER)).unwrap();
        fs::create_dir(used.join(OUT_MARKER)).unwrap();
        assert!(resolve_dirs(dir.path(), &used).is_err());
        fs::remove_dir(used.join(OUT_MARKER)).unwrap();
        fs::write(used.join(OUT_MARKER), OUT_MARKER_TEXT).unwrap();
        assert!(resolve_dirs(dir.path(), &used).is_ok());
        let empty = dir.path().join("empty");
        fs::create_dir(&empty).unwrap();
        assert!(resolve_dirs(dir.path(), &empty).is_ok());
        assert!(empty.join(OUT_MARKER).exists());
        // Outside the project: no marker, any directory.
        let other = tempfile::tempdir().unwrap();
        fs::write(other.path().join("a.csv"), "x").unwrap();
        let project = dir.path().join("p");
        fs::create_dir(&project).unwrap();
        assert!(resolve_dirs(&project, other.path()).is_ok());
        assert!(!other.path().join(OUT_MARKER).exists());
    }

    #[test]
    fn out_in_secrets_in_another_case_fails() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("secrets")).unwrap();
        assert!(resolve_dirs(dir.path(), &dir.path().join("Secrets")).is_err());
        assert!(resolve_dirs(dir.path(), &dir.path().join("CONFIG/x")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn out_through_a_symlink_to_secrets_fails() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("secrets")).unwrap();
        std::os::unix::fs::symlink("secrets", dir.path().join("link")).unwrap();
        assert!(resolve_dirs(dir.path(), &dir.path().join("link")).is_err());
        assert!(resolve_dirs(dir.path(), &dir.path().join("link/new")).is_err());
        assert!(!dir.path().join("secrets/new").exists());
    }

    #[test]
    fn an_output_that_is_an_input_file_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("a.csv");
        fs::write(&input, "x").unwrap();
        let other = dir.path().join("b.csv");
        fs::write(&other, "y").unwrap();
        let inputs: HashSet<FileId> = file_id(&input).into_iter().collect();
        assert!(is_input(&input, &inputs));
        assert!(!is_input(&other, &inputs));
        assert!(!is_input(&dir.path().join("missing.csv"), &inputs));
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
    fn other_files_skip_hidden_lock_and_tabular() {
        let ok = |p: &str| is_copyable_other(Path::new(p), Path::new(p));
        assert!(ok("a/notes.txt"));
        assert!(ok("a/noext"));
        assert!(!ok("a/b.CSV"));
        assert!(!ok("~$b.xlsx"));
        assert!(!ok("~$b.docx"));
        assert!(!ok(".private_x"));
        assert!(!ok(".git/config"));
        assert!(!ok("sub/.hidden/x.txt"));
        assert!(!ok("sub/.DS_Store"));
    }

    #[test]
    fn copy_streams_and_refuses_a_symlinked_target() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in.txt");
        fs::write(&input, "abc").unwrap();
        let out = dir.path().join("out");
        fs::create_dir(&out).unwrap();
        copy_file(&input, &out, &out.join("sub").join("in.txt")).unwrap();
        assert_eq!(fs::read_to_string(out.join("sub/in.txt")).unwrap(), "abc");
        #[cfg(unix)]
        {
            let elsewhere = dir.path().join("elsewhere.txt");
            fs::write(&elsewhere, "keep").unwrap();
            std::os::unix::fs::symlink(&elsewhere, out.join("link.txt")).unwrap();
            assert!(copy_file(&input, &out, &out.join("link.txt")).is_err());
            assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "keep");
        }
    }

    #[test]
    fn xlsx_extension_and_lock_files() {
        assert!(is_xlsx(Path::new("a/b.XLSX")));
        assert!(!is_xlsx(Path::new("a/b.xls")));
        assert!(is_lock_file(Path::new("a/~$b.xlsx")));
        assert!(!is_lock_file(Path::new("a/b.xlsx")));
    }
}
