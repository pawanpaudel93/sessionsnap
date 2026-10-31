//! Scheduled saves as launchd agents: one `~/Library/LaunchAgents/sessionsnap.schedule.<name>.plist`
//! per automatic snapshot, running `sessionsnap --terminal <t> schedule run -- <name>` every interval.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::adapters::home_dir;
use crate::domain::schedule::Schedule;
use crate::error::{Exit, failure};
use crate::ports::Scheduler;

/// The start of every label (and file name) sessionsnap gives its agents.
pub const LABEL_PREFIX: &str = "sessionsnap.schedule.";

pub struct Launchd {
    /// Where the agents' plists go.
    pub agents_dir: PathBuf,
    /// The sessionsnap binary the agents run.
    pub program: PathBuf,
    /// `launchctl`; tests pass a fake.
    pub launchctl: String,
    /// Variables the runs need (launchd starts them with an almost empty environment).
    pub env: Vec<(String, String)>,
}

impl Launchd {
    pub fn open_default() -> Result<Self> {
        let env = super::run_env();
        Ok(Launchd {
            agents_dir: home_dir()?.join("Library/LaunchAgents"),
            program: std::env::current_exe()?,
            launchctl: "launchctl".into(),
            env,
        })
    }

    fn label(name: &str) -> String {
        format!("{LABEL_PREFIX}{}", name.to_ascii_lowercase())
    }

    fn plist_path(&self, name: &str) -> PathBuf {
        self.agents_dir.join(format!("{}.plist", Self::label(name)))
    }

    fn launchctl(&self, args: &[&str]) -> Result<std::process::Output> {
        Command::new(&self.launchctl).args(args).output().with_context(|| format!("running {}", self.launchctl))
    }

    /// `gui/<uid>`: the logged-in user's launchd domain.
    fn domain() -> Result<String> {
        let out = Command::new("id").arg("-u").output().context("running id -u")?;
        Ok(format!("gui/{}", String::from_utf8_lossy(&out.stdout).trim()))
    }

    /// Unloads the agent `label` (fine when it isn't loaded).
    fn bootout(&self, label: &str) -> Result<()> {
        self.launchctl(&["bootout", &format!("{}/{label}", Self::domain()?)])?;
        Ok(())
    }

    /// Unloads and deletes every sessionsnap agent in `agents_dir`; returns their labels.
    pub fn remove_all(&self) -> Result<Vec<String>> {
        let mut removed = vec![];
        let Ok(entries) = std::fs::read_dir(&self.agents_dir) else { return Ok(removed) };
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            let Some(label) = file.strip_suffix(".plist").filter(|l| l.starts_with(LABEL_PREFIX)) else { continue };
            self.bootout(label)?;
            std::fs::remove_file(entry.path()).with_context(|| format!("removing {}", entry.path().display()))?;
            removed.push(label.to_string());
        }
        removed.sort();
        Ok(removed)
    }
}

impl Scheduler for Launchd {
    fn install(&self, name: &str, schedule: &Schedule) -> Result<()> {
        let label = Self::label(name);
        let path = self.plist_path(name);
        std::fs::create_dir_all(&self.agents_dir).with_context(|| format!("creating {}", self.agents_dir.display()))?;
        std::fs::write(&path, plist(&label, &self.program, name, schedule, &self.env))
            .with_context(|| format!("writing {}", path.display()))?;
        self.bootout(&label)?;
        let out = self.launchctl(&["bootstrap", &Self::domain()?, &path.to_string_lossy()])?;
        if !out.status.success() {
            let _ = std::fs::remove_file(&path);
            return Err(failure(
                Exit::Error,
                format!("launchctl could not load {label}: {}", String::from_utf8_lossy(&out.stderr).trim()),
            ));
        }
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<()> {
        self.bootout(&Self::label(name))?;
        match std::fs::remove_file(self.plist_path(name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(anyhow::Error::new(e).context(format!("removing {}", self.plist_path(name).display())))
            }
            _ => Ok(()),
        }
    }
}

/// The agent's plist: runs `<program> --terminal <t> schedule run -- <name>` every interval, not at
/// load (`schedule add` has just saved). The terminal is always named: launchd runs outside both.
pub fn plist(label: &str, program: &Path, name: &str, schedule: &Schedule, env: &[(String, String)]) -> String {
    let s = |v: &str| format!("<string>{}</string>", xml(v));
    let program = program.to_string_lossy();
    let terminal = schedule.terminal.as_deref().unwrap_or("cmux");
    let args: Vec<String> = std::iter::once(program.as_ref())
        .chain(["--terminal", terminal])
        .chain(["schedule", "run", "--", name])
        .map(s)
        .collect();
    let interval = schedule.interval;
    let vars: String = env.iter().map(|(k, v)| format!("\n    <key>{}</key>{}", xml(k), s(v))).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>{}
  <key>ProgramArguments</key>
  <array>{}</array>
  <key>StartInterval</key><integer>{interval}</integer>
  <key>RunAtLoad</key><false/>
  <key>ProcessType</key><string>Background</string>
  <key>EnvironmentVariables</key>
  <dict>{vars}
  </dict>
</dict>
</plist>
"#,
        s(label),
        args.join("")
    )
}

fn xml(v: &str) -> String {
    v.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A launchd on `dir/agents` whose launchctl only logs its arguments to `dir/log`.
    fn fake(dir: &Path, fail_bootstrap: bool) -> Launchd {
        let script = dir.join("launchctl");
        let fail = if fail_bootstrap { "[ \"$1\" = bootstrap ] && { echo nope >&2; exit 5; }\n" } else { "" };
        std::fs::write(&script, format!("#!/bin/sh\necho \"$@\" >> '{}'\n{fail}exit 0\n", dir.join("log").display()))
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Launchd {
            agents_dir: dir.join("agents"),
            program: "/opt/s s/sessionsnap".into(),
            launchctl: script.to_string_lossy().into_owned(),
            env: vec![("PATH".into(), "/bin:/a&b".into())],
        }
    }

    fn every(interval: u64) -> Schedule {
        Schedule { interval, created_at: "t".into(), stopped: false, last_run: None, terminal: None }
    }

    fn log(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("log")).unwrap_or_default()
    }

    #[test]
    fn plist_runs_the_named_save_every_interval() {
        let p = plist(
            "sessionsnap.schedule.work",
            Path::new("/bin/s<x>"),
            "-work",
            &every(600),
            &[("PATH".into(), "/a&b".into())],
        );
        assert!(p.contains("<string>/bin/s&lt;x&gt;</string><string>--terminal</string><string>cmux</string><string>schedule</string><string>run</string><string>--</string><string>-work</string>"), "{p}");
        assert!(p.contains("<key>StartInterval</key><integer>600</integer>"));
        assert!(p.contains("<key>PATH</key><string>/a&amp;b</string>"));
        assert!(p.contains("<key>RunAtLoad</key><false/>"));
        // Where Apple's checker is there (macOS), launchd's parser accepts it.
        if Path::new("/usr/bin/plutil").exists() {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("a.plist");
            std::fs::write(&file, &p).unwrap();
            let out = Command::new("/usr/bin/plutil").arg("-lint").arg(&file).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
        }
    }

    #[test]
    fn plist_names_a_tmux_schedules_terminal() {
        let tmux = Schedule { terminal: Some("tmux".into()), ..every(60) };
        let p = plist("l", Path::new("/bin/s"), "w", &tmux, &[]);
        assert!(
            p.contains(
                "<string>/bin/s</string><string>--terminal</string><string>tmux</string><string>schedule</string>"
            ),
            "{p}"
        );
    }

    #[test]
    fn install_replaces_the_agent_and_remove_deletes_it() {
        let dir = tempfile::tempdir().unwrap();
        let l = fake(dir.path(), false);
        l.install("Work", &every(600)).unwrap();
        let plist = dir.path().join("agents/sessionsnap.schedule.work.plist");
        assert!(std::fs::read_to_string(&plist).unwrap().contains("<string>Work</string>"));
        let calls = log(dir.path());
        assert!(calls.contains("bootout gui/") && calls.contains("/sessionsnap.schedule.work"), "{calls}");
        assert!(calls.contains("bootstrap gui/") && calls.contains(&plist.display().to_string()), "{calls}");
        l.remove("work").unwrap();
        assert!(!plist.exists());
        l.remove("work").unwrap();
    }

    #[test]
    fn a_failed_load_leaves_no_plist() {
        let dir = tempfile::tempdir().unwrap();
        let e = fake(dir.path(), true).install("work", &every(600)).unwrap_err();
        assert!(format!("{e:#}").contains("nope"), "{e:#}");
        assert!(!dir.path().join("agents/sessionsnap.schedule.work.plist").exists());
    }

    #[test]
    fn remove_all_takes_only_sessionsnap_agents() {
        let dir = tempfile::tempdir().unwrap();
        let l = fake(dir.path(), false);
        l.install("a", &every(600)).unwrap();
        l.install("b", &every(3600)).unwrap();
        std::fs::write(dir.path().join("agents/com.other.plist"), "x").unwrap();
        assert_eq!(l.remove_all().unwrap(), ["sessionsnap.schedule.a", "sessionsnap.schedule.b"]);
        let left: Vec<_> =
            std::fs::read_dir(dir.path().join("agents")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, ["com.other.plist"]);
    }
}
