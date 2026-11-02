//! Known names: the real names from the name columns of all project files
//! (spec 0004). They stay in memory only and are never printed.
#![expect(
    dead_code,
    reason = "spec 0004 skeleton; removed when the module is implemented and wired in"
)]

use crate::config::FreeText;
use crate::text::Part;

/// The known names of one project. Pass 1 of `run` fills it.
#[derive(Default)]
pub struct KnownNames {
    _todo: (),
}

impl KnownNames {
    /// Add one person from one row: the first-name and last-name cell values
    /// (each may hold several tokens, e.g. "Anna Lena"). Either may be empty.
    pub fn add_person(&mut self, _first: &str, _last: &str) {
        todo!("task known_names")
    }

    /// Add the value of one `full_name` cell ("First Middle Last" or "Last, First").
    pub fn add_full(&mut self, _value: &str) {
        todo!("task known_names")
    }

    /// Build the whole-word matcher for `mode`.
    pub fn matcher(&self, _mode: FreeText) -> anyhow::Result<Matcher> {
        todo!("task known_names")
    }
}

/// Whole-word, case-insensitive matcher for the known names.
pub struct Matcher {
    _todo: (),
}

impl Matcher {
    /// Replace each known name in `text`. `replace(part, token)` gives the
    /// pseudonym of one real token; the result keeps the case style of the
    /// matched text. Returns the new text and the number of replaced names.
    pub fn replace(
        &self,
        _text: &str,
        _replace: &mut dyn FnMut(Part, &str) -> String,
    ) -> (String, usize) {
        todo!("task known_names")
    }
}
