//! Panic hook without the panic message, and a silent guard for caught panics.
//!
//! A panic message can contain a real value. The hook prints only the code
//! location. A panic inside [`catch`] prints nothing: the caller reports it.

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};

thread_local! {
    static CATCHING: Cell<bool> = const { Cell::new(false) };
}

/// Install the panic hook. Call it once at the start of `main`.
pub fn install_hook() {
    panic::set_hook(Box::new(|info| {
        if CATCHING.with(Cell::get) {
            return;
        }
        match info.location() {
            Some(location) => eprintln!(
                "error: internal error at {}:{}",
                location.file(),
                location.line()
            ),
            None => eprintln!("error: internal error"),
        }
    }));
}

/// Run `f`. Return `None` if it panics. The hook stays silent for that panic.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "spec 0003 preparation; xlsx_file calls it, the task removes this"
    )
)]
pub fn catch<R>(f: impl FnOnce() -> R) -> Option<R> {
    let before = CATCHING.with(|flag| flag.replace(true));
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    CATCHING.with(|flag| flag.set(before));
    result.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_returns_the_value_or_none() {
        assert_eq!(catch(|| 7), Some(7));
        let caught: Option<()> = catch(|| panic!("invented"));
        assert_eq!(caught, None);
        assert!(!CATCHING.with(Cell::get));
    }
}
