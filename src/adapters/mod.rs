//! Adapters: the outside world behind the ports — the snapshot store on disk, terminals (cmux)
//! and coding agents (Claude Code).
//!
//! Layer rule: adapters implement `crate::ports` traits and may use `crate::core` types; they never
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
