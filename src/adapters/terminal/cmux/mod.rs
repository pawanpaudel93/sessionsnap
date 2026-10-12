//! cmux: reads what is open (live tree or state file), opens workspaces and tabs, and installs
//! sessionsnap's command-palette entries.
//!
//! Layer rule: an adapter; implements `crate::ports::Terminal`, never depends on `app` or
//! `interfaces`.

pub mod cli;
pub mod palette;
pub mod runner;
pub mod socket;
pub mod state_file;
pub mod tree;

/// Strips status glyphs cmux and Claude put before titles ("✳ fix login" → "fix login").
pub fn clean_title(s: &str) -> String {
    let t = s.trim_start_matches(|c: char| !c.is_alphanumeric()).trim();
    if t.is_empty() { s.trim().to_string() } else { t.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_title_strips_status_glyphs() {
        assert_eq!(clean_title("✳ fix login"), "fix login");
        assert_eq!(clean_title("⠂ Cascadia"), "Cascadia");
        assert_eq!(clean_title("✳"), "✳");
    }
}
