//! `sessionsnap uninstall`: remove what sessionsnap installed, its data, and its binary.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::adapters::agents::claude::{SkillRemoval, uninstall_skill};
use crate::adapters::scheduler::launchd::{LABEL_PREFIX, Launchd};
use crate::adapters::terminal::cmux::palette::uninstall_cmux;

/// Everything uninstall touches; `paths()` finds the real ones, tests pass temp dirs.
#[derive(Debug, Clone)]
pub struct Paths {
    pub cmux_config: PathBuf,
    pub skills_dir: PathBuf,
    pub data: PathBuf,
    /// This binary, canonical.
    pub exe: PathBuf,
    /// This binary as it was run (before resolving symlinks): the PATH line names this folder.
    pub exe_as_run: PathBuf,
    /// Where `cargo install` puts binaries.
    pub cargo_bin: Option<PathBuf>,
    /// Shell startup files the installer may have added a PATH line to.
    pub rc_files: Vec<PathBuf>,
    /// Where scheduled saves' launchd agents are (macOS only).
    pub launch_agents: Option<PathBuf>,
}

/// The paths of this install.
pub fn paths(data: &Path) -> Result<Paths> {
    use anyhow::Context;
    let exe_as_run = std::env::current_exe()?;
    let env_dir = |name| std::env::var_os(name).map(PathBuf::from).filter(|p: &PathBuf| p.is_absolute());
    Ok(Paths {
        cmux_config: super::install::cmux_config_path()?,
        skills_dir: crate::adapters::agents::claude::skills_dir()?,
        data: data.to_path_buf(),
        exe: exe_as_run.canonicalize().context("find this binary")?,
        exe_as_run,
        cargo_bin: super::update::cargo_bin_dir(),
        rc_files: rc_files(&crate::adapters::home_dir()?, env_dir("ZDOTDIR"), env_dir("XDG_CONFIG_HOME")),
        launch_agents: (std::env::consts::OS == "macos")
            .then(|| crate::adapters::home_dir().map(|h| h.join("Library/LaunchAgents")))
            .transpose()?,
    })
}

/// The startup files install.sh may write to: the same ones it picks, plus the defaults.
pub fn rc_files(home: &Path, zdotdir: Option<PathBuf>, xdg_config: Option<PathBuf>) -> Vec<PathBuf> {
    let mut files = vec![zdotdir.unwrap_or_else(|| home.to_path_buf()).join(".zshrc"), home.join(".zshrc")];
    files.extend([".bashrc", ".bash_profile", ".profile"].map(|f| home.join(f)));
    files.push(xdg_config.unwrap_or_else(|| home.join(".config")).join("fish/config.fish"));
    files.push(home.join(".config/fish/config.fish"));
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| seen.insert(f.clone()));
    files
}

/// One step's result, e.g. `{what: "~/.sessionsnap", status: "removed"}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Step {
    pub what: String,
    /// "removed", "already gone", "kept", "left" or "failed" (the note says why).
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub steps: Vec<Step>,
    /// Shared things left in place, for the user to remove if nothing else needs them.
    pub left: Vec<String>,
}

impl Report {
    /// The steps that failed.
    pub fn failed(&self) -> Vec<&Step> {
        self.steps.iter().filter(|s| s.status == "failed").collect()
    }
}

/// What `run` will remove, one line each, to ask about first.
pub fn preview(p: &Paths, keep_data: bool) -> Vec<String> {
    let mut lines = vec![
        format!("the \"sessionsnap: …\" palette commands in {} (backed up first)", p.cmux_config.display()),
        format!("the Claude Code skill in {}", p.skills_dir.join("sessionsnap").display()),
    ];
    if p.launch_agents.is_some() {
        lines.push("every automatic snapshot's schedule (its launchd agent)".into());
    }
    if keep_data {
        lines.push(format!("(keeping your snapshots in {})", p.data.display()));
    } else {
        lines.push(format!("{} (your snapshots)", p.data.display()));
    }
    if from_cargo(p) {
        lines.push(format!("(not {}: run `cargo uninstall sessionsnap`)", p.exe.display()));
    } else {
        lines.push(p.exe.display().to_string());
    }
    lines
}

fn from_cargo(p: &Paths) -> bool {
    p.cargo_bin.as_deref().is_some_and(|bin| p.exe.starts_with(bin))
}

fn step(what: impl Into<String>, status: &'static str, note: Option<String>) -> Step {
    Step { what: what.into(), status, note }
}

/// Removes the palette commands, the skill, the data (unless `keep_data`) and the binary. A piece
/// that is already gone is reported, not an error. A step that fails is reported as "failed" with
/// its error, and the others still run, so the report always says what was removed.
pub fn run(p: &Paths, keep_data: bool) -> Result<Report> {
    let mut steps = vec![];
    let failed = |what: String, e: anyhow::Error| step(what, "failed", Some(format!("{e:#}")));

    let config = p.cmux_config.display().to_string();
    steps.push(match uninstall_cmux(&p.cmux_config) {
        Ok(Some(backup)) => {
            step(format!("palette commands in {config}"), "removed", Some(format!("backup: {}", backup.display())))
        }
        Ok(None) => step(format!("palette commands in {config}"), "already gone", None),
        // A file sessionsnap can't edit safely (say, comments inside "commands") must not stop
        // the rest of the uninstall.
        Err(e) if crate::error::exit_of(&e) == crate::error::Exit::Refused => {
            step(format!("palette commands in {config}"), "left", Some(format!("{e:#}")))
        }
        Err(e) => failed(format!("palette commands in {config}"), e),
    });

    let skill = p.skills_dir.join("sessionsnap").display().to_string();
    steps.push(match uninstall_skill(&p.skills_dir) {
        Ok(SkillRemoval::Removed) => step(skill, "removed", None),
        Ok(SkillRemoval::Missing) => step(skill, "already gone", None),
        Ok(SkillRemoval::NotOurs) => step(skill, "left", Some("it has files sessionsnap did not write".into())),
        Err(e) => failed(skill, e),
    });

    if let Some(dir) = &p.launch_agents {
        let launchd =
            Launchd { agents_dir: dir.clone(), program: p.exe.clone(), launchctl: "launchctl".into(), env: vec![] };
        let what = format!("scheduled saves ({}/{LABEL_PREFIX}*)", dir.display());
        steps.push(match launchd.remove_all() {
            Ok(labels) if labels.is_empty() => step(what, "already gone", None),
            Ok(labels) => step(what, "removed", Some(labels.join(", "))),
            Err(e) => failed(what, e),
        });
    }

    let data = p.data.display().to_string();
    steps.push(if keep_data {
        step(data, "kept", Some("--keep-data".into()))
    } else {
        remove_data(&p.data).unwrap_or_else(|e| failed(data, e))
    });

    let exe = p.exe.display().to_string();
    steps.push(if from_cargo(p) {
        step(exe, "left", Some("installed with cargo; run: cargo uninstall sessionsnap".into()))
    } else {
        match std::fs::remove_file(&p.exe) {
            Ok(()) => step(exe, "removed", None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => step(exe, "already gone", None),
            Err(e) => failed(exe.clone(), anyhow::Error::new(e).context(format!("remove {exe}"))),
        }
    });

    Ok(Report { steps, left: leftovers(p) })
}

/// Names a sessionsnap data folder holds. Anything else there means the folder is not only ours
/// (say SESSIONSNAP_HOME points at a shared folder), so it is left alone.
const DATA_ENTRIES: &[&str] =
    &["snapshots", "plans", "locks", "sessions.json", "sessions.pending", "sessions.dirty", "schedules.json"];

fn remove_data(data: &Path) -> Result<Step> {
    let what = data.display().to_string();
    match std::fs::symlink_metadata(data) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(step(what, "already gone", None)),
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(data).map(|t| t.display().to_string()).unwrap_or_default();
            return Ok(step(what, "left", Some(format!("it is a symlink to {target}; remove that by hand"))));
        }
        _ => {}
    }
    let entries = match std::fs::read_dir(data) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(step(what, "already gone", None)),
        Err(e) => return Err(anyhow::Error::new(e).context(format!("read {what}"))),
    };
    let foreign: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !DATA_ENTRIES.contains(&n.as_str()) && n != ".DS_Store" && !n.starts_with("sessions.json.bad-"))
        .collect();
    if !foreign.is_empty() {
        return Ok(step(
            what,
            "left",
            Some(format!("it holds files sessionsnap did not make: {}", foreign.join(", "))),
        ));
    }
    if !looks_like_ours(data) {
        return Ok(step(
            what,
            "left",
            Some("its snapshots or plans folder holds files sessionsnap did not make".into()),
        ));
    }
    std::fs::remove_dir_all(data).map_err(|e| anyhow::Error::new(e).context(format!("remove {what}")))?;
    Ok(step(what, "removed", None))
}

/// True when `snapshots/` holds only folders of `.json` versions and `plans/` only `.json` plans,
/// as the store writes them (and leftovers of interrupted writes).
fn looks_like_ours(data: &Path) -> bool {
    let only = |dir: &Path, ok: &dyn Fn(&std::fs::DirEntry) -> bool| match std::fs::read_dir(dir) {
        Ok(entries) => entries.filter_map(|e| e.ok()).all(|e| e.file_name() == ".DS_Store" || ok(&e)),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    };
    let json_file = |e: &std::fs::DirEntry| {
        {
            let name = e.file_name().to_string_lossy().into_owned();
            // `.tmp-<pid>`: a write interrupted before its rename.
            e.file_type().is_ok_and(|t| t.is_file()) && (name.ends_with(".json") || name.contains(".tmp-"))
        }
    };
    only(&data.join("plans"), &json_file)
        && only(&data.join("snapshots"), &|e| e.file_type().is_ok_and(|t| t.is_dir()) && only(&e.path(), &json_file))
}

/// Shared things the installer set up that other tools may rely on: the PATH line for the
/// binary's folder in shell startup files.
fn leftovers(p: &Paths) -> Vec<String> {
    let mut dirs: Vec<String> =
        [&p.exe_as_run, &p.exe].iter().filter_map(|e| e.parent()).map(|d| d.display().to_string()).collect();
    dirs.dedup();
    let mut left = vec![];
    for rc in &p.rc_files {
        let Ok(text) = std::fs::read_to_string(rc) else { continue };
        let path_line =
            |dir: &str| text.lines().any(|l| l.contains(dir) && (l.contains("PATH") || l.contains("fish_add_path")));
        if let Some(dir) = dirs.iter().find(|d| path_line(d)) {
            left.push(format!("the PATH line for {dir} in {}", rc.display()));
        }
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full install under a temp dir: palette commands next to another entry, the skill, a
    /// snapshot, the binary in ~/.local/bin and its PATH line in .zshrc.
    fn installed(dir: &Path) -> Paths {
        let p = Paths {
            cmux_config: dir.join(".config/cmux/cmux.json"),
            skills_dir: dir.join(".claude/skills"),
            data: dir.join(".sessionsnap"),
            exe: dir.join(".local/bin/sessionsnap"),
            exe_as_run: dir.join(".local/bin/sessionsnap"),
            cargo_bin: Some(dir.join(".cargo/bin")),
            rc_files: rc_files(dir, None, None),
            launch_agents: None,
        };
        std::fs::create_dir_all(p.cmux_config.parent().unwrap()).unwrap();
        std::fs::write(&p.cmux_config, r#"{"commands": [{"name": "Mine", "command": "ls"}]}"#).unwrap();
        crate::adapters::terminal::cmux::palette::install_cmux(&p.cmux_config, "sessionsnap").unwrap();
        crate::adapters::agents::claude::install_skill(&p.skills_dir).unwrap();
        std::fs::create_dir_all(p.data.join("snapshots/work")).unwrap();
        std::fs::write(p.data.join("snapshots/work/0001.json"), "{}").unwrap();
        // Everything else the store keeps at its root.
        std::fs::create_dir_all(p.data.join("locks")).unwrap();
        std::fs::write(p.data.join("locks/schedules.lock"), "").unwrap();
        std::fs::create_dir_all(p.data.join("sessions.pending")).unwrap();
        for f in ["sessions.json", "sessions.json.bad-20260101T000000", "schedules.json"] {
            std::fs::write(p.data.join(f), "{}").unwrap();
        }
        std::fs::create_dir_all(p.exe.parent().unwrap()).unwrap();
        std::fs::write(&p.exe, "binary").unwrap();
        let line = format!("export PATH=\"{}:$PATH\"", dir.join(".local/bin").display());
        std::fs::write(dir.join(".zshrc"), format!("\n{line}\n")).unwrap();
        p
    }

    fn statuses(r: &Report) -> Vec<&'static str> {
        r.steps.iter().map(|s| s.status).collect()
    }

    #[test]
    fn removes_everything_it_installed() {
        let d = tempfile::tempdir().unwrap();
        let p = installed(d.path());
        let r = run(&p, false).unwrap();
        assert_eq!(statuses(&r), ["removed"; 4]);
        let config = std::fs::read_to_string(&p.cmux_config).unwrap();
        assert!(config.contains("\"Mine\"") && !config.contains("sessionsnap:"), "{config}");
        assert!(r.steps[0].note.as_deref().unwrap().starts_with("backup: "));
        assert!(!p.skills_dir.join("sessionsnap").exists());
        assert!(p.skills_dir.exists());
        assert!(!p.data.exists());
        assert!(!p.exe.exists());
        assert!(p.exe.parent().unwrap().exists());
        assert_eq!(r.left.len(), 1);
        assert!(r.left[0].contains(".zshrc"), "{:?}", r.left);
        assert!(std::fs::read_to_string(d.path().join(".zshrc")).unwrap().contains("PATH"));
    }

    /// A step that fails (here the binary, in a folder it can't write) does not hide what the
    /// earlier steps already removed: the report lists them, then the failure.
    #[test]
    fn a_failed_step_still_reports_what_was_removed() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = installed(d.path());
        let bin = p.exe.parent().unwrap();
        std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = run(&p, false);
        std::fs::set_permissions(bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let r = r.unwrap();
        assert_eq!(statuses(&r), ["removed", "removed", "removed", "failed"]);
        assert!(!p.data.exists());
        let note = r.steps[3].note.as_deref().unwrap();
        assert!(note.contains("remove") && note.contains("ermission denied"), "{note}");
        assert_eq!(r.failed().len(), 1);
    }

    #[test]
    fn lists_the_fish_path_line_too() {
        let d = tempfile::tempdir().unwrap();
        let p = installed(d.path());
        let fish = d.path().join(".config/fish/config.fish");
        std::fs::create_dir_all(fish.parent().unwrap()).unwrap();
        std::fs::write(&fish, format!("fish_add_path \"{}\"\n", d.path().join(".local/bin").display())).unwrap();
        let left = leftovers(&p);
        assert_eq!(left.len(), 2, "{left:?}");
        assert!(left[1].contains("config.fish"), "{left:?}");
    }

    #[test]
    fn keep_data_keeps_snapshots() {
        let d = tempfile::tempdir().unwrap();
        let p = installed(d.path());
        let r = run(&p, true).unwrap();
        assert_eq!(statuses(&r), ["removed", "removed", "kept", "removed"]);
        assert!(p.data.join("snapshots/work/0001.json").exists());
        assert!(preview(&p, true).iter().any(|l| l.contains("keeping")));
    }

    #[test]
    fn missing_pieces_are_already_gone() {
        let d = tempfile::tempdir().unwrap();
        let p = installed(d.path());
        run(&p, false).unwrap();
        let again = run(&p, false).unwrap();
        assert_eq!(statuses(&again), ["already gone"; 4]);
    }

    #[test]
    fn cargo_installs_keep_their_binary() {
        let d = tempfile::tempdir().unwrap();
        let mut p = installed(d.path());
        p.exe = d.path().join(".cargo/bin/sessionsnap");
        p.exe_as_run = p.exe.clone();
        std::fs::create_dir_all(p.exe.parent().unwrap()).unwrap();
        std::fs::write(&p.exe, "binary").unwrap();
        let r = run(&p, false).unwrap();
        assert_eq!(r.steps[3].status, "left");
        assert!(r.steps[3].note.as_deref().unwrap().contains("cargo uninstall sessionsnap"));
        assert!(p.exe.exists());
        assert!(preview(&p, false).iter().any(|l| l.contains("cargo uninstall")));
    }

    #[test]
    fn a_shared_data_folder_is_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let mut p = installed(d.path());
        // SESSIONSNAP_HOME pointed at a folder with other things in it.
        p.data = d.path().to_path_buf();
        let r = run(&p, false).unwrap();
        assert_eq!(r.steps[2].status, "left");
        assert!(d.path().join(".zshrc").exists());
    }

    #[test]
    fn a_palette_it_cannot_edit_does_not_stop_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let p = installed(d.path());
        let text = std::fs::read_to_string(&p.cmux_config).unwrap().replacen('[', "[ // mine\n", 1);
        std::fs::write(&p.cmux_config, &text).unwrap();
        let r = run(&p, false).unwrap();
        assert_eq!(statuses(&r), ["left", "removed", "removed", "removed"]);
        assert!(r.steps[0].note.as_deref().unwrap().contains("by hand"), "{:?}", r.steps[0]);
        assert_eq!(std::fs::read_to_string(&p.cmux_config).unwrap(), text);
    }

    #[test]
    fn a_symlinked_data_folder_is_left_with_its_target() {
        let d = tempfile::tempdir().unwrap();
        let mut p = installed(d.path());
        let link = d.path().join("data-link");
        std::os::unix::fs::symlink(&p.data, &link).unwrap();
        p.data = link.clone();
        let r = run(&p, false).unwrap();
        assert_eq!(r.steps[2].status, "left");
        assert!(r.steps[2].note.as_deref().unwrap().contains("symlink"));
        assert!(link.exists());
        assert!(d.path().join(".sessionsnap/snapshots/work/0001.json").exists());
    }

    #[test]
    fn a_snapshots_folder_with_other_files_is_left() {
        let d = tempfile::tempdir().unwrap();
        let p = installed(d.path());
        std::fs::write(p.data.join("snapshots/work/holiday.jpg"), "mine").unwrap();
        assert_eq!(run(&p, false).unwrap().steps[2].status, "left");
        assert!(p.data.join("snapshots/work/holiday.jpg").exists());

        let d = tempfile::tempdir().unwrap();
        let p = installed(d.path());
        std::fs::write(p.data.join("snapshots/notes.txt"), "mine").unwrap();
        assert_eq!(run(&p, false).unwrap().steps[2].status, "left");
    }

    #[test]
    fn path_lines_in_zdotdir_and_xdg_config_and_via_symlinks_are_listed() {
        let d = tempfile::tempdir().unwrap();
        let mut p = installed(d.path());
        std::fs::remove_file(d.path().join(".zshrc")).unwrap();
        // The binary was run through a symlinked folder; the installer wrote that folder.
        let linked = d.path().join("bin-link");
        std::os::unix::fs::symlink(d.path().join(".local/bin"), &linked).unwrap();
        p.exe_as_run = linked.join("sessionsnap");
        let (zdot, xdg) = (d.path().join("zdot"), d.path().join("xdg"));
        std::fs::create_dir_all(xdg.join("fish")).unwrap();
        std::fs::create_dir_all(&zdot).unwrap();
        std::fs::write(zdot.join(".zshrc"), format!("export PATH=\"{}:$PATH\"\n", linked.display())).unwrap();
        std::fs::write(xdg.join("fish/config.fish"), format!("fish_add_path \"{}\"\n", linked.display())).unwrap();
        p.rc_files = rc_files(d.path(), Some(zdot.clone()), Some(xdg.clone()));
        let left = leftovers(&p);
        assert_eq!(left.len(), 2, "{left:?}");
        assert!(left[0].contains("zdot/.zshrc") && left[1].contains("xdg/fish/config.fish"), "{left:?}");
    }

    #[test]
    fn someone_elses_skill_is_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let p = installed(d.path());
        std::fs::write(p.skills_dir.join("sessionsnap/extra.md"), "mine").unwrap();
        let r = run(&p, false).unwrap();
        assert_eq!(r.steps[1].status, "left");
        assert!(p.skills_dir.join("sessionsnap/extra.md").exists());
    }
}
