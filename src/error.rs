use std::fmt;

/// Process exit codes, documented in the spec and in skill/SKILL.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Ok = 0,
    Error = 1,
    TerminalUnreachable = 2,
    NothingToDo = 3,
    Refused = 4,
    UnknownFormat = 5,
    PlanStale = 6,
    NotFound = 7,
}

impl Exit {
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn name(self) -> &'static str {
        match self {
            Exit::Ok => "ok",
            Exit::Error => "error",
            Exit::TerminalUnreachable => "terminal-unreachable",
            Exit::NothingToDo => "nothing-to-do",
            Exit::Refused => "refused",
            Exit::UnknownFormat => "unknown-format",
            Exit::PlanStale => "plan-stale",
            Exit::NotFound => "not-found",
        }
    }
}

/// An error that carries the exit code the process should end with.
#[derive(Debug)]
pub struct Failure {
    pub exit: Exit,
    pub message: String,
    /// A more precise code than the exit's name, for `{"error": {"code"}}`.
    pub code: Option<&'static str>,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

pub fn failure(exit: Exit, message: impl Into<String>) -> anyhow::Error {
    Failure { exit, message: message.into(), code: None }.into()
}

/// An action that needs a person's yes (exit 4, code `needs-confirmation`). The message says to
/// ask, not to add `--yes`: an agent must not answer for the user.
pub fn needs_confirmation(question: &str) -> anyhow::Error {
    let message = format!("{question} Not confirmed: ask the user, and run it again with --yes only if they agree.");
    Failure { exit: Exit::Refused, message, code: Some("needs-confirmation") }.into()
}

/// The error's code: its `Failure`'s own, else its exit's name.
pub fn code_of(err: &anyhow::Error) -> &'static str {
    err.downcast_ref::<Failure>().and_then(|f| f.code).unwrap_or_else(|| exit_of(err).name())
}

/// The exit code for an error: its `Failure` code, or `Error` for anything else.
pub fn exit_of(err: &anyhow::Error) -> Exit {
    err.downcast_ref::<Failure>().map(|f| f.exit).unwrap_or(Exit::Error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn exit_code_survives_context() {
        let err: anyhow::Result<()> = Err(failure(Exit::NotFound, "gone"));
        let err = err.context("loading").unwrap_err();
        assert_eq!(exit_of(&err), Exit::NotFound);
        assert_eq!(exit_of(&anyhow::anyhow!("plain")), Exit::Error);
        assert_eq!(Exit::PlanStale.code(), 6);
        assert_eq!(Exit::TerminalUnreachable.name(), "terminal-unreachable");
        // A refusal that needs a person says so, and never tells the caller to add --yes on its own.
        let refused = needs_confirmation("Delete snapshot work?");
        assert_eq!((exit_of(&refused), code_of(&refused)), (Exit::Refused, "needs-confirmation"));
        assert!(refused.to_string().contains("ask the user"));
        assert_eq!(code_of(&failure(Exit::Refused, "exists")), "refused");
    }
}
