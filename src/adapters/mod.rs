//! Adapters: the outside world behind the ports — the snapshot store on disk, terminals (cmux),
//! released builds on GitHub, and coding agents (Claude Code, Codex and the others in the agent registry).
//!
//! Layer rule: adapters implement `crate::ports` traits and may use `crate::domain` types; they never
//! depend on `app` or `interfaces`.

use std::path::PathBuf;

use anyhow::Result;

use crate::error::{Exit, failure};

pub mod agents;
pub mod releases;
pub mod scheduler;
pub mod store;
pub mod terminal;

pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| failure(Exit::Error, "HOME is not set"))
}

/// Quotes `s` for a POSIX shell, without any backslash (a command typed by `cmux send` must not
/// hold one): an apostrophe becomes `'"'"'`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r#"'"'"'"#))
}

/// `s` as one shell word: as is when it has only characters that are safe unquoted, else quoted.
/// A leading `=` or `~` is quoted too: zsh expands `=cmd` to the path of `cmd`.
pub fn shell_word(s: &str) -> String {
    let safe = !s.is_empty()
        && !s.starts_with(['=', '~'])
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
    if safe { s.to_string() } else { shell_quote(s) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_for_the_shell() {
        assert_eq!(shell_quote("it's"), r#"'it'"'"'s'"#);
        assert_eq!(shell_word("--model=o3"), "--model=o3");
        assert_eq!(shell_word("a b"), "'a b'");
        assert_eq!(shell_word(""), "''");
        assert_eq!(shell_word("~/x"), "'~/x'");
        assert_eq!(shell_word("=foo"), "'=foo'");
        assert_eq!(shell_word("a=b"), "a=b");
        assert_eq!(shell_word("$(id)"), "'$(id)'");
        assert_eq!(shell_word("it's"), r#"'it'"'"'s'"#);
    }

    #[test]
    fn quoted_words_round_trip_through_sh() {
        for word in ["Don't push", "it's", "a b", "$(id)", "=x", "~/x", "'", "''", "\"q\"", "plain"] {
            let script = format!("printf %s {}", shell_word(word));
            let out = std::process::Command::new("sh").arg("-c").arg(&script).output().unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), word, "{script}");
            assert!(!shell_word(word).contains('\\') || word.contains('\\'), "{word}");
        }
    }
}
