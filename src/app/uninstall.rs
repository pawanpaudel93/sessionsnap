//! `sessionsnap uninstall`: remove what sessionsnap installed, its data, and its binary.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::adapters::agents::claude::{SkillRemoval, uninstall_skill};
use crate::adapters::terminal::cmux::palette::uninstall_cmux;

/// Everything uninstall touches; `paths()` finds the real ones, tests pass temp dirs.
#[derive(Debug, Clone)]
pub struct Paths {
    pub cmux_config: PathBuf,
    pub skills_dir: PathBuf,
    pub data: PathBuf,
    /// This binary, canonical.
    pub exe: PathBuf,
    /// Where `cargo install` puts binaries.
    pub cargo_bin: Option<PathBuf>,
    pub home: PathBuf,
}

/// The paths of this install.
pub fn paths(data: &Path) -> Result<Paths> {
    use anyhow::Context;
    Ok(Paths {
        cmux_config: super::install::cmux_config_path()?,
        skills_dir: crate::adapters::agents::claude::skills_dir()?,
        data: data.to_path_buf(),
        exe: std::env::current_exe()?.canonicalize().context("find this binary")?,
        cargo_bin: super::update::cargo_bin_dir(),
        home: crate::adapters::home_dir()?,
    })
}

/// One step's result, e.g. `{what: "~/.sessionsnap", status: "removed"}`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Step {
    pub what: String,
    /// "removed", "already gone", "kept" or "left".
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

/// What `run` will remove, one line each, to ask about first.
pub fn preview(p: &Paths, keep_data: bool) -> Vec<String> {
    let mut lines = vec![
        format!("the \"sessionsnap: …\" palette commands in {} (backed up first)", p.cmux_config.display()),
        format!("the Claude Code skill in {}", p.skills_dir.join("sessionsnap").display()),
    ];
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
/// that is already gone is reported, not an error.
pub fn run(p: &Paths, keep_data: bool) -> Result<Report> {
    let mut steps = vec![];

    let config = p.cmux_config.display().to_string();
    steps.push(match uninstall_cmux(&p.cmux_config)? {
        Some(backup) => {
            step(format!("palette commands in {config}"), "removed", Some(format!("backup: {}", backup.display())))
        }
        None => step(format!("palette commands in {config}"), "already gone", None),
    });

    let skill = p.skills_dir.join("sessionsnap").display().to_string();
    steps.push(match uninstall_skill(&p.skills_dir)? {
        SkillRemoval::Removed => step(skill, "removed", None),
        SkillRemoval::Missing => step(skill, "already gone", None),
        SkillRemoval::NotOurs => step(skill, "left", Some("it has files sessionsnap did not write".into())),
    });

    let data = p.data.display().to_string();
    steps.push(if keep_data { step(data, "kept", Some("--keep-data".into())) } else { remove_data(&p.data)? });

    let exe = p.exe.display().to_string();
    steps.push(if from_cargo(p) {
        step(exe, "left", Some("installed with cargo; run: cargo uninstall sessionsnap".into()))
    } else {
        match std::fs::remove_file(&p.exe) {
            Ok(()) => step(exe, "removed", None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => step(exe, "already gone", None),
            Err(e) => return Err(anyhow::Error::new(e).context(format!("remove {exe}"))),
        }
    });

    Ok(Report { steps, left: leftovers(p) })
}

/// Names a sessionsnap data folder holds. Anything else there means the folder is not only ours
/// (say SESSIONSNAP_HOME points at a shared folder), so it is left alone.
const DATA_ENTRIES: &[&str] = &["snapshots", "plans"];

fn remove_data(data: &Path) -> Result<Step> {
    let what = data.display().to_string();
    let entries = match std::fs::read_dir(data) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(step(what, "already gone", None)),
        Err(e) => return Err(anyhow::Error::new(e).context(format!("read {what}"))),
    };
    let foreign: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !DATA_ENTRIES.contains(&n.as_str()) && n != ".DS_Store")
        .collect();
    if !foreign.is_empty() {
        return Ok(step(
            what,
            "left",
            Some(format!("it holds files sessionsnap did not make: {}", foreign.join(", "))),
        ));
    }
    std::fs::remove_dir_all(data).map_err(|e| anyhow::Error::new(e).context(format!("remove {what}")))?;
    Ok(step(what, "removed", None))
}

/// Shared things the installer set up that other tools may rely on: the PATH line for the
/// binary's folder in shell startup files.
fn leftovers(p: &Paths) -> Vec<String> {
    let Some(dir) = p.exe.parent().map(|d| d.display().to_string()) else {
        return vec![];
    };
    [".zshrc", ".bashrc", ".bash_profile", ".profile", ".config/fish/config.fish"]
        .iter()
        .map(|rc| p.home.join(rc))
        .filter(|rc| {
            std::fs::read_to_string(rc).is_ok_and(|t| t.lines().any(|l| l.contains("PATH") && l.contains(&dir)))
        })
        .map(|rc| format!("the PATH line for {dir} in {}", rc.display()))
        .collect()
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
            cargo_bin: Some(dir.join(".cargo/bin")),
            home: dir.to_path_buf(),
        };
        std::fs::create_dir_all(p.cmux_config.parent().unwrap()).unwrap();
        std::fs::write(&p.cmux_config, r#"{"commands": [{"name": "Mine", "command": "ls"}]}"#).unwrap();
        crate::adapters::terminal::cmux::palette::install_cmux(&p.cmux_config, "sessionsnap").unwrap();
        crate::adapters::agents::claude::install_skill(&p.skills_dir).unwrap();
        std::fs::create_dir_all(p.data.join("snapshots/work")).unwrap();
        std::fs::write(p.data.join("snapshots/work/0001.json"), "{}").unwrap();
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
    fn someone_elses_skill_is_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let p = installed(d.path());
        std::fs::write(p.skills_dir.join("sessionsnap/extra.md"), "mine").unwrap();
        let r = run(&p, false).unwrap();
        assert_eq!(r.steps[1].status, "left");
        assert!(p.skills_dir.join("sessionsnap/extra.md").exists());
    }
}
