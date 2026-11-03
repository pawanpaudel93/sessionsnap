//! Scheduled saves as systemd user timers (Linux): for each automatic snapshot, a
//! `sessionsnap-schedule-<name>.service` that runs `sessionsnap --terminal <t> schedule run -- <name>`
//! and a `.timer` that starts it every interval, in `~/.config/systemd/user`. On a machine nobody is
//! logged in to (a server), `loginctl enable-linger` keeps them running.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::adapters::home_dir;
use crate::adapters::process::Output;
use crate::domain::schedule::Schedule;
use crate::error::{Exit, failure};
use crate::ports::Scheduler;

/// The start of every unit name (and file name) sessionsnap gives its timers.
pub const UNIT_PREFIX: &str = "sessionsnap-schedule-";

pub struct Systemd {
    /// Where the user's units go (`$XDG_CONFIG_HOME/systemd/user`).
    pub units_dir: PathBuf,
    /// The sessionsnap binary the units run.
    pub program: PathBuf,
    /// `systemctl`; tests pass a fake.
    pub systemctl: String,
    /// Variables the runs need (a user service starts with few).
    pub env: Vec<(String, String)>,
}

impl Systemd {
    pub fn open_default() -> Result<Self> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .map_or_else(|| home_dir().map(|h| h.join(".config")), Ok)?;
        Ok(Systemd {
            units_dir: config.join("systemd/user"),
            program: std::env::current_exe()?,
            systemctl: "systemctl".into(),
            env: super::run_env(),
        })
    }

    fn unit(name: &str) -> String {
        format!("{UNIT_PREFIX}{}", name.to_ascii_lowercase())
    }

    fn systemctl(&self, args: &[&str]) -> Result<Output> {
        let args: Vec<&str> = ["--user"].into_iter().chain(args.iter().copied()).collect();
        super::run(&self.systemctl, &args).with_context(|| format!("running {}", self.systemctl))
    }

    fn systemctl_ok(&self, args: &[&str]) -> Result<()> {
        let out = self.systemctl(args)?;
        if !out.success {
            return Err(failure(
                Exit::Error,
                format!("systemctl --user {} failed: {}", args.join(" "), out.stderr.trim()),
            ));
        }
        Ok(())
    }

    /// Stops and deletes every sessionsnap timer in `units_dir`; returns their unit names.
    pub fn remove_all(&self) -> Result<Vec<String>> {
        let mut removed = vec![];
        let Ok(entries) = std::fs::read_dir(&self.units_dir) else { return Ok(removed) };
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            let Some(unit) = file.strip_suffix(".timer").filter(|u| u.starts_with(UNIT_PREFIX)) else { continue };
            self.remove_unit(unit)?;
            removed.push(unit.to_string());
        }
        removed.sort();
        Ok(removed)
    }

    fn remove_unit(&self, unit: &str) -> Result<()> {
        // Fine when it isn't loaded.
        let _ = self.systemctl(&["disable", "--now", &format!("{unit}.timer")]);
        for ext in ["timer", "service"] {
            let path = self.units_dir.join(format!("{unit}.{ext}"));
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(anyhow::Error::new(e).context(format!("removing {}", path.display())));
                }
                _ => {}
            }
        }
        let _ = self.systemctl(&["daemon-reload"]);
        Ok(())
    }
}

impl Scheduler for Systemd {
    fn install(&self, name: &str, schedule: &Schedule) -> Result<()> {
        let unit = Self::unit(name);
        let (service, timer) = units(&self.program, name, schedule, &self.env)?;
        std::fs::create_dir_all(&self.units_dir).with_context(|| format!("creating {}", self.units_dir.display()))?;
        let paths = [self.units_dir.join(format!("{unit}.service")), self.units_dir.join(format!("{unit}.timer"))];
        for (path, text) in paths.iter().zip([service, timer]) {
            std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
        }
        // Loaded and (re)started, so a changed interval applies now.
        let loaded = self
            .systemctl_ok(&["daemon-reload"])
            .and_then(|()| self.systemctl_ok(&["enable", &format!("{unit}.timer")]))
            .and_then(|()| self.systemctl_ok(&["restart", &format!("{unit}.timer")]));
        if let Err(e) = loaded {
            let _ = self.remove_unit(&unit);
            return Err(e);
        }
        Ok(())
    }

    fn remove(&self, name: &str) -> Result<()> {
        self.remove_unit(&Self::unit(name))
    }
}

/// The service and timer of `name`: the service runs `<program> --terminal <t> schedule run --
/// <name>` once; the timer starts it every interval (first one interval from now: `schedule add`
/// has just saved).
pub fn units(program: &Path, name: &str, schedule: &Schedule, env: &[(String, String)]) -> Result<(String, String)> {
    let terminal = schedule.terminal.unwrap_or_default().name();
    let program = program.to_string_lossy();
    let exec = [program.as_ref(), "--terminal", terminal, "schedule", "run", "--", name]
        .iter()
        .map(|w| quote(w, true))
        .collect::<Result<Vec<_>>>()?
        .join(" ");
    let environment = env
        .iter()
        .map(|(k, v)| Ok(format!("Environment={}\n", quote(&format!("{k}={v}"), false)?)))
        .collect::<Result<String>>()?;
    // Free text, not a quoted word: only specifiers are escaped (the name is checked: no control
    // characters, so it can't start another setting).
    let description = format!("sessionsnap: save the automatic snapshot {}", name.replace('%', "%%"));
    if description.chars().any(char::is_control) {
        return Err(failure(Exit::Error, "not writing a systemd unit with a control character in its name"));
    }
    let service =
        // Exit 3 is "nothing to do" (no changes, or the terminal isn't running): not a failure.
        format!("[Unit]\nDescription={description}\n\n[Service]\nType=oneshot\nSuccessExitStatus=3\n{environment}ExecStart={exec}\n");
    let interval = schedule.interval;
    let timer = format!(
        "[Unit]\nDescription={description}\n\n[Timer]\nOnActiveSec={interval}s\nOnUnitActiveSec={interval}s\nAccuracySec=10s\n\n[Install]\nWantedBy=timers.target\n"
    );
    Ok((service, timer))
}

/// `value` as one double-quoted systemd word: backslashes and quotes escaped, `%` (a specifier)
/// doubled, and in a command line (`exec`) `$` doubled too, so nothing in it is expanded. A control
/// character (a newline would start another setting) is refused.
fn quote(value: &str, exec: bool) -> Result<String> {
    if value.chars().any(char::is_control) {
        return Err(failure(Exit::Error, format!("not writing a systemd unit with a control character: {value:?}")));
    }
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' if exec => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every(interval: u64, terminal: Option<&str>) -> Schedule {
        Schedule {
            interval,
            created_at: "t".into(),
            stopped: false,
            last_run: None,
            terminal: terminal.and_then(crate::domain::terminal::TerminalKind::parse),
            keep: None,
        }
    }

    /// A systemd whose systemctl only logs its arguments to `dir/log` (and fails `restart` when asked).
    fn fake(dir: &Path, fail_restart: bool) -> Systemd {
        let script = dir.join("systemctl");
        let fail = if fail_restart { "[ \"$2\" = restart ] && { echo nope >&2; exit 5; }\n" } else { "" };
        std::fs::write(&script, format!("#!/bin/sh\necho \"$@\" >> '{}'\n{fail}exit 0\n", dir.join("log").display()))
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Systemd {
            units_dir: dir.join("units"),
            program: "/opt/s s/sessionsnap".into(),
            systemctl: script.to_string_lossy().into_owned(),
            env: vec![("PATH".into(), "/bin:/a b".into())],
        }
    }

    fn log(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("log")).unwrap_or_default()
    }

    #[test]
    fn the_service_runs_the_named_save_and_the_timer_repeats_it() {
        let env = [("PATH".to_string(), "/a b:/c".to_string()), ("SESSIONSNAP_HOME".into(), "/h/100%".into())];
        let (service, timer) =
            units(Path::new("/opt/s s/sessionsnap"), "-work", &every(600, Some("tmux")), &env).unwrap();
        assert!(
            service.contains(r#"ExecStart="/opt/s s/sessionsnap" "--terminal" "tmux" "schedule" "run" "--" "-work""#),
            "{service}"
        );
        assert!(service.contains("Type=oneshot\nSuccessExitStatus=3\n"), "{service}");
        assert!(service.contains("Description=sessionsnap: save the automatic snapshot -work\n"), "{service}");
        assert!(service.contains(r#"Environment="PATH=/a b:/c""#), "{service}");
        assert!(service.contains(r#"Environment="SESSIONSNAP_HOME=/h/100%%""#), "a % is not a specifier: {service}");
        assert!(timer.contains("OnActiveSec=600s\nOnUnitActiveSec=600s"), "{timer}");
        assert!(timer.contains("WantedBy=timers.target"));
        let (cmux, _) = units(Path::new("/s"), "w", &every(60, None), &[]).unwrap();
        assert!(cmux.contains(r#""--terminal" "cmux""#), "the terminal is always named: {cmux}");
    }

    /// Nothing a path or variable holds is expanded or ends the line: `$`, `%`, quotes, backslashes.
    #[test]
    fn hostile_values_stay_inside_their_word() {
        let env = [("PATH".to_string(), "/x\"; rm -rf ~; \"$HOME\\".to_string())];
        let (service, _) = units(Path::new("/opt/$HOME/%h/s\"x"), "w", &every(60, None), &env).unwrap();
        assert!(service.contains(r#"ExecStart="/opt/$$HOME/%%h/s\"x""#), "{service}");
        assert!(service.contains(r#"Environment="PATH=/x\"; rm -rf ~; \"$HOME\\""#), "{service}");
        let newline = [("PATH".to_string(), "/a\n[Service]\nExecStart=/bin/evil".to_string())];
        assert!(units(Path::new("/s"), "w", &every(60, None), &newline).is_err());
        assert!(units(Path::new("/s\n"), "w", &every(60, None), &[]).is_err());
    }

    #[test]
    fn install_loads_and_starts_the_timer_and_remove_deletes_it() {
        let dir = tempfile::tempdir().unwrap();
        let s = fake(dir.path(), false);
        s.install("Work", &every(600, None)).unwrap();
        let timer = dir.path().join("units/sessionsnap-schedule-work.timer");
        assert!(timer.exists() && dir.path().join("units/sessionsnap-schedule-work.service").exists());
        let calls = log(dir.path());
        assert!(calls.contains("--user daemon-reload"), "{calls}");
        assert!(calls.contains("--user enable sessionsnap-schedule-work.timer"), "{calls}");
        assert!(calls.contains("--user restart sessionsnap-schedule-work.timer"), "{calls}");
        s.remove("work").unwrap();
        assert!(!timer.exists());
        assert!(log(dir.path()).contains("--user disable --now sessionsnap-schedule-work.timer"));
        s.remove("work").unwrap();
    }

    #[test]
    fn a_failed_start_leaves_no_units() {
        let dir = tempfile::tempdir().unwrap();
        let e = fake(dir.path(), true).install("work", &every(600, None)).unwrap_err();
        assert!(format!("{e:#}").contains("nope"), "{e:#}");
        assert_eq!(std::fs::read_dir(dir.path().join("units")).unwrap().count(), 0);
    }

    #[test]
    fn remove_all_takes_only_sessionsnap_timers() {
        let dir = tempfile::tempdir().unwrap();
        let s = fake(dir.path(), false);
        s.install("a", &every(600, None)).unwrap();
        s.install("b", &every(3600, None)).unwrap();
        std::fs::write(dir.path().join("units/other.timer"), "x").unwrap();
        assert_eq!(s.remove_all().unwrap(), ["sessionsnap-schedule-a", "sessionsnap-schedule-b"]);
        let left: Vec<_> =
            std::fs::read_dir(dir.path().join("units")).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, ["other.timer"]);
    }
}
