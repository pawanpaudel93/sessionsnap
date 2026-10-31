//! The terminals sessionsnap works with. Adding one: ARCHITECTURE.md's "Add a terminal".

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalKind {
    /// cmux (macOS), the one used when nothing says otherwise.
    #[default]
    Cmux,
    Tmux,
}

impl TerminalKind {
    /// Every terminal, in the order `--terminal` lists them.
    pub const ALL: [TerminalKind; 2] = [TerminalKind::Cmux, TerminalKind::Tmux];

    /// The name `--terminal`, versions and schedules use.
    pub fn name(self) -> &'static str {
        match self {
            TerminalKind::Cmux => "cmux",
            TerminalKind::Tmux => "tmux",
        }
    }

    /// The terminal named `name`, if sessionsnap knows it.
    pub fn parse(name: &str) -> Option<TerminalKind> {
        TerminalKind::ALL.into_iter().find(|t| t.name() == name)
    }
}

impl fmt::Display for TerminalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_serialize_as_names() {
        for t in TerminalKind::ALL {
            assert_eq!(TerminalKind::parse(t.name()), Some(t));
            assert_eq!(serde_json::to_string(&t).unwrap(), format!("\"{t}\""));
        }
        assert_eq!(TerminalKind::parse("wezterm"), None);
        assert_eq!(TerminalKind::default(), TerminalKind::Cmux);
    }
}
