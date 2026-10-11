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

/// Name of the workspace the palette commands open.
pub const PALETTE_WORKSPACE: &str = "ccsnap";

/// Palette entries. Each opens its own "ccsnap" workspace: a plain palette command would type
/// into the focused tab, which is often a running Claude session.
pub fn palette_commands(exe: &str) -> Vec<Value> {
    let entry = |name: &str, keywords: &[&str], args: &str| {
        json!({
            "name": name,
            "keywords": keywords,
            "restart": "recreate",
            "workspace": {
                "name": PALETTE_WORKSPACE,
                "cwd": "~",
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
/// cmux.json is JSONC, so the file is edited as text: comments and formatting outside the
/// `commands` array stay as they are.
pub fn install_cmux(config: &Path, exe: &str) -> Result<Option<PathBuf>> {
    if !config.exists() {
        write_json_atomic(config, &json!({ "commands": palette_commands(exe) }))?;
        return Ok(None);
    }
    let text = std::fs::read_to_string(config)?;
    let updated = with_palette_commands(&text, exe).map_err(|why| {
        failure(
            Exit::Refused,
            format!("{}: {why}; add the commands by hand from `ccsnap install-cmux --print`", config.display()),
        )
    })?;
    let backup = config.with_file_name(format!("cmux.json.bak-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    std::fs::copy(config, &backup).with_context(|| format!("backing up {}", config.display()))?;
    let tmp = config.with_extension("json.tmp");
    std::fs::write(&tmp, updated)?;
    std::fs::rename(&tmp, config)?;
    Ok(Some(backup))
}

/// Returns `text` with the ccsnap palette commands in its top-level `commands` array.
fn with_palette_commands(text: &str, exe: &str) -> std::result::Result<String, String> {
    let plain = strip_jsonc(text);
    let root: Value = serde_json::from_str(&plain).map_err(|e| format!("not valid JSON ({e})"))?;
    let obj = root.as_object().ok_or("not a JSON object")?;
    let mut list = match obj.get("commands") {
        None => vec![],
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return Err("\"commands\" is not an array".into()),
    };
    list.retain(|c| !c.get("name").and_then(Value::as_str).is_some_and(|n| n.starts_with("ccsnap:")));
    list.extend(palette_commands(exe));
    let array = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?.replace('\n', "\n  ");
    if let Some((start, end)) = top_level_array(&plain, "commands") {
        if text[start..end] != plain[start..end] {
            return Err("its \"commands\" array contains comments".into());
        }
        return Ok(format!("{}{array}{}", &text[..start], &text[end..]));
    }
    let open = plain.find('{').ok_or("not a JSON object")?;
    let empty = plain[open + 1..].trim_start().starts_with('}');
    let sep = if empty { "\n" } else { ",\n" };
    Ok(format!("{}\n  \"commands\": {array}{sep}{}", &text[..=open], text[open + 1..].trim_start_matches([' ', '\t'])))
}

/// Blanks out `//` and `/* */` comments and trailing commas, keeping every byte offset, so
/// positions in the result are positions in the original text.
fn strip_jsonc(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = b.to_vec();
    let blank = |out: &mut Vec<u8>, i: usize| {
        if out[i] != b'\n' {
            out[i] = b' ';
        }
    };
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    blank(&mut out, i);
                    i += 1;
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = text[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
                while i < end {
                    blank(&mut out, i);
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    // Trailing commas: a comma followed only by whitespace before `}` or `]`.
    let mut last_comma: Option<usize> = None;
    let mut in_string = false;
    let mut j = 0;
    while j < out.len() {
        let c = out[j];
        if in_string {
            if c == b'\\' {
                j += 1;
            } else if c == b'"' {
                in_string = false;
            }
        } else if c == b'"' {
            in_string = true;
            last_comma = None;
        } else if c == b',' {
            last_comma = Some(j);
        } else if c == b'}' || c == b']' {
            if let Some(k) = last_comma.take() {
                out[k] = b' ';
            }
        } else if !c.is_ascii_whitespace() {
            last_comma = None;
        }
        j += 1;
    }
    String::from_utf8(out).expect("only ASCII bytes were replaced")
}

/// Byte range of the array value of `key` in the top-level object of comment-free JSON.
fn top_level_array(plain: &str, key: &str) -> Option<(usize, usize)> {
    let b = plain.as_bytes();
    let (mut depth, mut i) = (0usize, 0usize);
    while i < b.len() {
        match b[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                let is_key = plain[i + 1..].trim_start().starts_with(':');
                if depth == 1 && is_key && plain[start + 1..i] == *key {
                    let colon = i + 1 + plain[i + 1..].find(':')?;
                    let open = colon + 1 + plain[colon + 1..].find(|c: char| !c.is_whitespace())?;
                    if b[open] != b'[' {
                        return None;
                    }
                    let mut d = 0usize;
                    let mut k = open;
                    while k < b.len() {
                        match b[k] {
                            b'"' => {
                                k += 1;
                                while k < b.len() && b[k] != b'"' {
                                    k += if b[k] == b'\\' { 2 } else { 1 };
                                }
                            }
                            b'[' | b'{' => d += 1,
                            b']' | b'}' => {
                                d -= 1;
                                if d == 0 {
                                    return Some((open, k + 1));
                                }
                            }
                            _ => {}
                        }
                        k += 1;
                    }
                    return None;
                }
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    None
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
        // cmux reads `restart` from the command, not from its workspace definition.
        assert_eq!(v["commands"][1]["restart"], "recreate");
        assert!(v["commands"][1]["workspace"].get("restart").is_none());
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
    fn refuses_invalid_config() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("cmux.json");
        std::fs::write(&cfg, "// comment\n{ nope").unwrap();
        assert_eq!(exit_of(&install_cmux(&cfg, "ccsnap").unwrap_err()), Exit::Refused);
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "// comment\n{ nope");
    }

    const TEMPLATE: &str = "{\n  \"$schema\": \"https://example.com/cmux.schema.json\",\n  \"schemaVersion\": 1,\n\n  // This file uses JSON with comments (JSONC).\n  /* block */\n  //   \"app\" : {\n  //   },\n}\n";

    fn names(text: &str) -> Vec<String> {
        let v: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        v["commands"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn jsonc_template_keeps_comments() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("cmux.json");
        std::fs::write(&cfg, TEMPLATE).unwrap();
        assert!(install_cmux(&cfg, "ccsnap").unwrap().is_some());
        let text = std::fs::read_to_string(&cfg).unwrap();
        for line in ["  // This file uses JSON with comments (JSONC).", "  /* block */", "  //   \"app\" : {"] {
            assert!(text.contains(line), "lost {line:?} in:\n{text}");
        }
        assert_eq!(names(&text).len(), 4);
        let v: Value = serde_json::from_str(&strip_jsonc(&text)).unwrap();
        assert_eq!(v["schemaVersion"], 1);
        assert_eq!(v["$schema"], "https://example.com/cmux.schema.json");
    }

    #[test]
    fn jsonc_reinstall_replaces_only_own_commands() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("cmux.json");
        std::fs::write(
            &cfg,
            TEMPLATE.replacen("{\n", "{\n  \"commands\": [{\"name\": \"Mine\", \"command\": \"ls\"}],\n", 1),
        )
        .unwrap();
        install_cmux(&cfg, "ccsnap").unwrap();
        install_cmux(&cfg, "ccsnap").unwrap();
        let text = std::fs::read_to_string(&cfg).unwrap();
        let n = names(&text);
        assert_eq!(n.len(), 5, "{n:?}");
        assert_eq!(n[0], "Mine");
        assert!(text.contains("  /* block */"));
    }

    #[test]
    fn refuses_comments_inside_commands() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("cmux.json");
        let original = "{\n  \"commands\": [\n    // mine\n    {\"name\": \"Mine\"}\n  ]\n}\n";
        std::fs::write(&cfg, original).unwrap();
        assert_eq!(exit_of(&install_cmux(&cfg, "ccsnap").unwrap_err()), Exit::Refused);
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), original);
    }

    #[test]
    fn empty_object_stays_plain_json() {
        let d = tempfile::tempdir().unwrap();
        let cfg = d.path().join("cmux.json");
        std::fs::write(&cfg, "{}\n").unwrap();
        install_cmux(&cfg, "ccsnap").unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(v["commands"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn installs_skill_and_quotes() {
        let d = tempfile::tempdir().unwrap();
        let p = install_skill(d.path()).unwrap();
        assert!(std::fs::read_to_string(p).unwrap().contains("name: ccsnap"));
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
