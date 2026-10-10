use std::fmt;

/// Process exit codes, documented in the spec and in skill/SKILL.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Ok = 0,
    Error = 1,
    CmuxUnreachable = 2,
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
            Exit::CmuxUnreachable => "cmux-unreachable",
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
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

pub fn failure(exit: Exit, message: impl Into<String>) -> anyhow::Error {
    Failure { exit, message: message.into() }.into()
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
        assert_eq!(Exit::CmuxUnreachable.name(), "cmux-unreachable");
    }
}
