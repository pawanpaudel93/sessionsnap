use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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

    /// Like `run`, but a program still running after `timeout` is killed and this fails. Fakes
    /// answer at once, so by default this is `run`.
    fn run_timeout(&self, program: &str, args: &[String], timeout: Duration) -> Result<Output> {
        let _ = timeout;
        self.run(program, args)
    }
}

pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<Output> {
        let out = Command::new(program).args(args).output().with_context(|| format!("running {program}"))?;
        Ok(Output {
            success: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn run_timeout(&self, program: &str, args: &[String], timeout: Duration) -> Result<Output> {
        let mut command = Command::new(program);
        command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Its own process group, so a timeout kills whatever it started too.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn().with_context(|| format!("running {program}"))?;
        // Read both pipes on threads, so a full pipe never blocks the child.
        let reader = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut text = String::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_string(&mut text);
                }
                text
            })
        };
        let stdout = reader(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let stderr = reader(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                #[cfg(unix)]
                let _ = Command::new("kill").args(["-s", "KILL", "--", &format!("-{}", child.id())]).status();
                let _ = child.kill();
                let _ = child.wait();
                // The readers end once nothing holds the pipes; they are not waited for.
                let first = args.first().map_or("", String::as_str);
                anyhow::bail!("`{program} {first}` did not answer within {}s and was stopped", timeout.as_secs_f32());
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        Ok(Output {
            success: status.success(),
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        })
    }
}
