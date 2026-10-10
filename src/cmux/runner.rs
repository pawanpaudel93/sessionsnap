use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs external programs; faked in tests.
pub trait Runner {
    fn run(&self, program: &str, args: &[String]) -> Result<Output>;
}

pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        let out = std::process::Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("running {program}"))?;
        Ok(Output {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}
