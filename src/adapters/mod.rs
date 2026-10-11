//! Adapters: the outside world behind the ports — the snapshot store on disk, terminals (cmux)
//! and coding agents (Claude Code).
//!
//! Layer rule: adapters implement `crate::ports` traits and may use `crate::domain` types; they never
//! depend on `app` or `interfaces`.

use std::path::PathBuf;

use anyhow::Result;

use crate::error::{Exit, failure};

pub mod agents;
pub mod store;
pub mod terminal;

pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| failure(Exit::Error, "HOME is not set"))
}

/// Quotes `s` for a POSIX shell.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `s` as one shell word: as is when it has only characters that are safe unquoted, else quoted.
pub fn shell_word(s: &str) -> String {
    let safe = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
    if safe { s.to_string() } else { shell_quote(s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_word("--model=o3"), "--model=o3");
        assert_eq!(shell_word("a b"), "'a b'");
        assert_eq!(shell_word(""), "''");
        assert_eq!(shell_word("~/x"), "'~/x'");
        assert_eq!(shell_word("$(id)"), "'$(id)'");
        assert_eq!(shell_word("it's"), r"'it'\''s'");
    }
}
