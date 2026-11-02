//! `pseudofy run`: find the files, apply the rules, write the copies.

use std::path::Path;

/// Counts for the summary line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Summary {
    pub files: usize,
    pub replaced: usize,
    pub collisions: usize,
}

/// Write a pseudonymized copy of each CSV file under `project` to `out`.
pub fn run(_project: &Path, _out: &Path) -> anyhow::Result<Summary> {
    todo!("task io")
}
