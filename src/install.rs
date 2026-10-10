use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::error::{Exit, failure};
use crate::store::{home_dir, write_json_atomic};

pub const SKILL: &str = include_str!("../skill/SKILL.md");

/// `$CCSNAP_CMUX_CONFIG`, else `~/.config/cmux/cmux.json`.
pub fn cmux_config_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("CCSNAP_CMUX_CONFIG") {
        return Ok(p.into());
    }
    Ok(home_dir()?.join(".config/cmux/cmux.json"))
}

/// `$CCSNAP_SKILLS_DIR`, else `~/.claude/skills`.
pub fn skills_dir() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("CCSNAP_SKILLS_DIR") {
        return Ok(p.into());
    }
    Ok(home_dir()?.join(".claude/skills"))
}

pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Name of the workspace the palette commands open; it is never captured into snapshots.
pub const PALETTE_WORKSPACE: &str = "ccsnap";

/// Palette entries. Each opens its own "ccsnap" workspace: a plain palette command would type
/// into the focused tab, which is often a running Claude session.
pub fn palette_commands(exe: &str) -> Vec<Value> {
    let entry = |name: &str, keywords: &[&str], args: &str| {
        json!({
            "name": name,
            "keywords": keywords,
            "workspace": {
                "name": PALETTE_WORKSPACE,
                "cwd": "~",
                "restart": "recreate",
                "layout": {"pane": {"surfaces": [
                    {"type": "terminal", "name": "ccsnap", "command": format!("{} {args}", shell_quote(exe))}
                ]}}
            }
        })
    };
    vec![
        entry("ccsnap: Save / update snapshot", &["snapshot", "save", "claude"], "save --pick"),
        entry("ccsnap: Restore snapshot", &["snapshot", "restore", "claude"], "restore --pick"),
        entry("ccsnap: Recent sessions not open", &["recent", "recover", "claude"], "recent --pick"),
        entry("ccsnap: List snapshots", &["snapshot", "list"], "list"),
    ]
}

/// Adds (or replaces) the "ccsnap: …" palette commands; returns the backup path if a file existed.
pub fn install_cmux(config: &Path, exe: &str) -> Result<Option<PathBuf>> {
    let mut root: Value = if config.exists() {
        let text = std::fs::read_to_string(config)?;
        serde_json::from_str(&text).map_err(|e| {
            failure(
                Exit::Refused,
                format!(
                    "{} is not plain JSON ({e}); add the commands by hand from `ccsnap install-cmux --print`",
                    config.display()
                ),
            )
        })?
    } else {
        json!({})
    };
    let obj = root.as_object_mut().ok_or_else(|| failure(Exit::Refused, "cmux.json is not a JSON object"))?;
    let commands = obj.entry("commands").or_insert_with(|| json!([]));
    let list =
        commands.as_array_mut().ok_or_else(|| failure(Exit::Refused, "cmux.json \"commands\" is not an array"))?;
    list.retain(|c| !c.get("name").and_then(Value::as_str).is_some_and(|n| n.starts_with("ccsnap:")));
    list.extend(palette_commands(exe));
    let backup = if config.exists() {
        let b = config.with_file_name(format!("cmux.json.bak-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        std::fs::copy(config, &b).with_context(|| format!("backing up {}", config.display()))?;
        Some(b)
    } else {
        None
    };
    write_json_atomic(config, &root)?;
    Ok(backup)
}

pub fn install_skill(dir: &Path) -> Result<PathBuf> {
    let path = dir.join("ccsnap").join("SKILL.md");
    std::fs::create_dir_all(dir.join("ccsnap"))?;
    std::fs::write(&path, SKILL)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::exit_of;

    #[test]
    fn install_replaces_own_commands_and_backs_up() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("cmux.json");
        std::fs::write(
            &cfg,
            r#"{"commands":[{"name":"Mine","command":"ls"},{"name":"ccsnap: Old","command":"x"}],"other":1}"#,
        )
        .unwrap();
        let backup = install_cmux(&cfg, "/usr/local/bin/ccsnap").unwrap().unwrap();
        assert!(backup.exists());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let names: Vec<&str> = v["commands"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            [
                "Mine",
                "ccsnap: Save / update snapshot",
                "ccsnap: Restore snapshot",
                "ccsnap: Recent sessions not open",
                "ccsnap: List snapshots",
            ]
        );
        assert_eq!(v["other"], 1);
        assert_eq!(
            v["commands"][1]["workspace"]["layout"]["pane"]["surfaces"][0]["command"],
            "'/usr/local/bin/ccsnap' save --pick"
        );
    }

    #[test]
    fn creates_config_when_missing() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("sub/cmux.json");
        assert!(install_cmux(&cfg, "ccsnap").unwrap().is_none());
        assert!(cfg.exists());
    }

    #[test]
    fn refuses_non_json_config() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("cmux.json");
        std::fs::write(&cfg, "// comment\n{}").unwrap();
        assert_eq!(exit_of(&install_cmux(&cfg, "ccsnap").unwrap_err()), Exit::Refused);
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "// comment\n{}");
    }

    #[test]
    fn installs_skill_and_quotes() {
        let d = tempfile::tempdir().unwrap();
        let p = install_skill(d.path()).unwrap();
        assert!(std::fs::read_to_string(p).unwrap().contains("name: ccsnap"));
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
