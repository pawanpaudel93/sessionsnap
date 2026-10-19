use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
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

    /// Like `run`, but a program still running after `timeout` (or whose output is still held
    /// open then) is stopped, with whatever it started, and this fails. Fakes answer at once, so
    /// by default this is `run`.
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
        // Its own process group, so a timeout stops whatever it started too.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn().with_context(|| format!("running {program}"))?;
        let group = child.id();
        // Both pipes are read on threads that hand over what they read, so a full pipe never
        // blocks the program, and a pipe a child of it keeps open never blocks this.
        let reader = |pipe: Option<Box<dyn Read + Send>>| {
            let (send, receive) = mpsc::channel();
            std::thread::spawn(move || {
                let mut text = String::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_string(&mut text);
                }
                let _ = send.send(text);
            });
            receive
        };
        let stdout = reader(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let stderr = reader(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let deadline = Instant::now() + timeout;
        let first = args.first().map_or("", String::as_str);
        let late = |child: &mut std::process::Child| {
            stop_group(group);
            let _ = child.kill();
            let _ = child.wait();
            anyhow::anyhow!("`{program} {first}` did not answer within {}s and was stopped", timeout.as_secs_f32())
        };
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                return Err(late(&mut child));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let read =
            |pipe: &mpsc::Receiver<String>| pipe.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        match (read(&stdout), read(&stderr)) {
            (Ok(stdout), Ok(stderr)) => Ok(Output { success: status.success(), stdout, stderr }),
            // Exited, but something it started still holds its output open.
            _ => Err(late(&mut child)),
        }
    }
}

/// Stops every process of the group `group` (the program and whatever it started).
fn stop_group(group: u32) {
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-s", "KILL", "--", &format!("-{group}")]).stderr(Stdio::null()).status();
    #[cfg(not(unix))]
    let _ = group;
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A program that exits but leaves a child holding its output open: the wait for that output
    /// is bounded too, and the child is stopped.
    #[test]
    fn output_held_open_is_bounded() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake");
        std::fs::write(&fake, "#!/bin/sh\nsleep 20 &\necho $! >> \"$(dirname \"$0\")/pids\"\necho ok\nexit 0\n")
            .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = Instant::now();
        let err = ProcessRunner.run_timeout(fake.to_str().unwrap(), &[], Duration::from_secs(2)).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10), "took {:?}", started.elapsed());
        assert!(err.to_string().contains("did not answer"), "{err}");
        let pids = std::fs::read_to_string(dir.path().join("pids")).unwrap();
        for pid in pids.split_whitespace() {
            let alive = Command::new("kill").args(["-0", pid]).stderr(Stdio::null()).status().unwrap();
            assert!(!alive.success(), "{pid} is still running");
        }
    }

    #[test]
    fn a_quick_program_answers() {
        let out = ProcessRunner
            .run_timeout("/bin/sh", &["-c".into(), "echo hi; echo err >&2".into()], Duration::from_secs(5))
            .unwrap();
        assert_eq!((out.success, out.stdout.as_str(), out.stderr.as_str()), (true, "hi\n", "err\n"));
    }
}
