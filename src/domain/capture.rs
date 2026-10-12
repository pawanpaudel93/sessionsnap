use anyhow::Result;

use crate::domain::model::{Tab, Workspace, find_by_name, has_control};
use crate::ports::{AgentCatalog, OpenState, OpenTab, OpenWorkspace};

/// Turns open workspaces (all, or those named in `only`) into snapshot workspaces, with notes on
/// what was left out. Agent tabs take their title from the agent's session history when it has
/// one, else the tab's title.
pub fn capture(open: &OpenState, agents: &dyn AgentCatalog, only: &[String]) -> Result<(Vec<Workspace>, Vec<String>)> {
    let chosen: Vec<&OpenWorkspace> = if only.is_empty() {
        open.workspaces.iter().collect()
    } else {
        let names: Vec<&str> = open.workspaces.iter().map(|w| w.name.as_str()).collect();
        only.iter()
            .map(|q| find_by_name(&names, q, "open workspace").map(|i| &open.workspaces[i]))
            .collect::<Result<_>>()?
    };
    let mut used: Vec<String> = vec![];
    let mut notes = vec![];
    let workspaces = chosen
        .into_iter()
        .map(|w| {
            let name = unique_name(&w.name, &mut used);
            let tabs = w
                .tabs
                .iter()
                .map(|t| to_tab(t, agents, &mut |n| notes.push(format!("{name} › {}: {n}", t.title))))
                .collect();
            Workspace {
                name,
                color: None,
                cwd: w.cwd.clone(),
                git_branch: w.git_branch.clone(),
                cmux_workspace_id: Some(w.id.clone()),
                tabs,
            }
        })
        .collect();
    Ok((workspaces, notes))
}

/// Non-empty and unique (case-insensitive) within one capture: "workspace", "Alpha", "Alpha (2)", …
fn unique_name(name: &str, used: &mut Vec<String>) -> String {
    let base = if name.trim().is_empty() { "workspace" } else { name };
    let mut candidate = base.to_string();
    let mut n = 2;
    while used.iter().any(|u| u.eq_ignore_ascii_case(&candidate)) {
        candidate = format!("{base} ({n})");
        n += 1;
    }
    used.push(candidate.clone());
    candidate
}

/// An agent the catalog does not know is kept as it is, so a later sessionsnap can resume it. An
/// agent record with a control character in its kind, ID or flags is left out (`note` says so):
/// the tab is saved as a plain terminal.
fn to_tab(t: &OpenTab, agents: &dyn AgentCatalog, note: &mut dyn FnMut(&str)) -> Tab {
    let agent = t.agent.as_ref().filter(|a| {
        let bad = has_control(&a.kind) || has_control(&a.session_id) || a.args.iter().any(|x| has_control(x));
        if bad {
            note("left out its agent session (control characters in its kind, ID or flags)");
        }
        !bad
    });
    match agent {
        Some(a) => {
            let (args, dropped) = without_credentials(&a.args);
            if dropped {
                note("dropped a flag that looked like a credential");
            }
            Tab::Agent {
                title: agents
                    .history(&a.kind)
                    .and_then(|h| h.title_for(&t.cwd, &a.session_id))
                    .unwrap_or_else(|| t.title.clone()),
                cwd: t.cwd.clone(),
                agent: agents.kind_of(&a.kind).map_or_else(|| a.kind.clone(), str::to_string),
                session_id: a.session_id.clone(),
                args,
            }
        }
        None => Tab::Terminal { title: t.title.clone(), cwd: t.cwd.clone(), command: None },
    }
}

/// Last key segments that mark a credential.
const SECRET_ENDINGS: &[&str] =
    &["token", "secret", "password", "passwd", "credential", "credentials", "auth", "authorization", "bearer"];

/// A flag name or config key that names a credential: its last segment (split on `.`, `_`, `-`)
/// is one of `SECRET_ENDINGS`, or it contains an API key (`api_key`, `apikey`, `api-key`).
/// `model_auto_compact_token_limit` or `--remote-auth-token-env` are not.
fn looks_secret(name: &str) -> bool {
    let name = name.trim_start_matches('-').to_ascii_lowercase();
    if ["api_key", "apikey", "api-key"].iter().any(|k| name.contains(k)) {
        return true;
    }
    let last = name.rsplit(['.', '_', '-']).next().unwrap_or("");
    SECRET_ENDINGS.contains(&last)
}

/// Prefixes specific to one token format (Anthropic, OpenAI projects, GitHub, Slack, GitLab):
/// followed by 8 or more characters, a word is taken as a token.
const SECRET_PREFIXES: &[&str] = &[
    "sk-ant-",
    "sk-proj-",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "glpat-",
];

/// Short prefixes that ordinary words also start with (OpenAI and others `sk-`/`sk_`, AWS `AKIA`,
/// Google `AIza`): a token only when the rest looks random (see `random_rest`).
const GENERIC_PREFIXES: &[&str] = &["sk-", "sk_", "akia", "aiza"];

/// `rest` (what follows a generic prefix in `word`, original case) looks like a key: 20 or more
/// characters with a digit or with both upper and lower case, or an AWS key's 16 upper-case
/// letters and digits after `AKIA`.
fn random_rest(word: &str, rest: &str) -> bool {
    let aws = word.starts_with("AKIA")
        && rest.len() == 16
        && rest.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    let has = |f: fn(&char) -> bool| rest.chars().any(|c| f(&c));
    let random = has(char::is_ascii_digit) || (has(char::is_ascii_uppercase) && has(char::is_ascii_lowercase));
    aws || (rest.len() >= 20 && random)
}

/// A value that looks like a credential whatever flag it follows: a known token or a JWT anywhere
/// in it, a `Bearer` header, or a credential-named key followed by `:` or `=` anywhere in it
/// (`NAME=value`, inline JSON or TOML such as `{"GITHUB_TOKEN":"…"}`, `x-api-key: …`).
fn looks_secret_value(v: &str) -> bool {
    token_like(v) || names_a_credential(v) || url_with_password(v)
}

/// `v` holds a URL with a password: `scheme://user:pass@host` (or `scheme://:pass@host`).
fn url_with_password(v: &str) -> bool {
    v.match_indices("://").any(|(i, _)| {
        let rest = &v[i + 3..];
        let authority = rest.split(|c: char| "/?#\"' \\".contains(c) || c.is_whitespace()).next().unwrap_or("");
        authority
            .rsplit_once('@')
            .and_then(|(userinfo, _)| userinfo.split_once(':'))
            .is_some_and(|(_, password)| !password.is_empty())
    })
}

/// A known token prefix or a JWT at the start of any word of `v`, or a Bearer header.
fn token_like(v: &str) -> bool {
    let words = v.split(|c: char| !(c.is_ascii_alphanumeric() || "_-.".contains(c)));
    for word in words {
        // A word may run on into a dot ("ghp_….", a sentence's end); prefixes hold none.
        let plain = word.split('.').next().unwrap_or("");
        let lower = plain.to_ascii_lowercase();
        if SECRET_PREFIXES.iter().any(|p| lower.starts_with(p) && lower.len() >= p.len() + 8) {
            return true;
        }
        if GENERIC_PREFIXES.iter().any(|p| lower.starts_with(p) && random_rest(plain, &plain[p.len()..])) {
            return true;
        }
        let parts: Vec<&str> = word.split('.').collect();
        if word.to_ascii_lowercase().starts_with("eyj") && parts.len() == 3 && parts.iter().all(|p| !p.is_empty()) {
            return true;
        }
    }
    v.to_ascii_lowercase().contains("bearer ")
}

/// Key endings that name a credential in a value's `key: …` or `key=…`.
const SECRET_KEY_ENDINGS: &[&str] =
    &["token", "password", "passwd", "credential", "credentials", "auth", "authorization", "access_key", "secret_key"];
/// Key parts that name a credential wherever they appear in the key.
const SECRET_KEY_PARTS: &[&str] = &["api_key", "api-key", "apikey", "x-api-key", "secret", "private"];

/// `v` holds a key that names a credential, followed by `:` or `=` (after an optional closing
/// quote and spaces), ignoring case: `GITHUB_TOKEN=`, `"X-API-Key":`, `x-api-key: `.
fn names_a_credential(v: &str) -> bool {
    let key_char = |c: char| c.is_ascii_alphanumeric() || "_-.".contains(c);
    let chars: Vec<char> = v.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !key_char(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && key_char(chars[i]) {
            i += 1;
        }
        let key: String = chars[start..i].iter().collect::<String>().to_ascii_lowercase();
        let mut j = i;
        // Closing quotes, also escaped ones in JSON inside JSON (`\"TOKEN\":`).
        while j < chars.len() && (chars[j] == '"' || chars[j] == '\'' || chars[j] == '\\') {
            j += 1;
        }
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        let assigned = j < chars.len() && (chars[j] == ':' || chars[j] == '=') && !rule_tail(&chars, j);
        let credential = SECRET_KEY_ENDINGS.iter().any(|e| key.ends_with(e))
            || SECRET_KEY_PARTS.iter().any(|p| key.contains(p))
            || looks_secret(&key);
        if assigned && credential {
            return true;
        }
    }
    false
}

/// The `:` at `colon` ends a permission rule (`Bash(gh auth:*)`), not a value: it is inside an
/// open `(`, and followed by `*` and `)` (spaces allowed around the `*`) or directly by `)`.
fn rule_tail(chars: &[char], colon: usize) -> bool {
    let depth = chars[..colon].iter().fold(0i32, |d, c| match c {
        '(' => d + 1,
        ')' => d - 1,
        _ => d,
    });
    if depth <= 0 || chars[colon] != ':' {
        return false;
    }
    let rest = &chars[colon + 1..];
    if rest.first() == Some(&')') {
        return true;
    }
    let mut it = rest.iter().skip_while(|c| c.is_whitespace());
    it.next() == Some(&'*') && it.find(|c| !c.is_whitespace()) == Some(&')')
}

/// `v` is one permission rule or a list of them (split by commas or spaces): `Edit`,
/// `Bash(gh auth:*)`, `mcp__gh__get_issue`. A rule is a tool name with an optional `(…)`.
fn rule_shaped(v: &str) -> bool {
    let chars: Vec<char> = v.chars().collect();
    let mut i = 0;
    let mut rules = 0;
    while i < chars.len() {
        if chars[i] == ',' || chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if !(chars[i].is_ascii_alphabetic() || chars[i] == '_') {
            return false;
        }
        while i < chars.len() && (chars[i].is_ascii_alphanumeric() || "_.-".contains(chars[i])) {
            i += 1;
        }
        if i < chars.len() && chars[i] == '(' {
            let mut depth = 0;
            loop {
                match chars.get(i) {
                    None => return false,
                    Some('(') => depth += 1,
                    Some(')') => depth -= 1,
                    _ => {}
                }
                i += 1;
                if depth == 0 {
                    break;
                }
            }
        }
        if i < chars.len() && !(chars[i] == ',' || chars[i].is_whitespace()) {
            return false;
        }
        rules += 1;
    }
    rules > 0
}

/// Flags whose values are permission rules (`Bash(gh auth:*)`): they name commands, not
/// credentials, so only a token inside one drops it. Losing a deny rule would weaken safety.
const RULE_FLAGS: &[&str] =
    &["--allowedtools", "--allowed-tools", "--disallowedtools", "--disallowed-tools", "--tools"];

fn is_rule_flag(flag: &str) -> bool {
    RULE_FLAGS.contains(&flag.to_ascii_lowercase().as_str())
}

/// Launch flags without those that look like credentials (`--api-key x`, `--token=x`,
/// `-c openai_api_key=x`), with their values; and whether any was dropped. Snapshots are plain
/// files, shown in the UI.
fn without_credentials(args: &[String]) -> (Vec<String>, bool) {
    let mut out = vec![];
    let mut dropped = false;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        let config = a == "-c" || a == "--config";
        let (name, inline) = match a.split_once('=') {
            Some((n, _)) if a.starts_with('-') => (n, true),
            _ => (a.as_str(), false),
        };
        if a.starts_with('-') && !config && looks_secret(name) {
            dropped = true;
            if !inline {
                it.next_if(|v| !v.starts_with('-'));
            }
            continue;
        }
        if config && it.peek().is_some_and(|v| looks_secret(v.split('=').next().unwrap_or(""))) {
            dropped = true;
            it.next();
            continue;
        }
        if let Some(kv) = a.strip_prefix("--config=").or_else(|| a.strip_prefix("-c="))
            && looks_secret(kv.split('=').next().unwrap_or(""))
        {
            dropped = true;
            continue;
        }
        out.push(a.clone());
    }
    // A credential-looking value goes, with the bare flag right before it (`--auth sk-…`).
    let mut kept: Vec<String> = Vec::with_capacity(out.len());
    // After a rule flag in the spaced form (`--allowedTools A B`), the values that follow are
    // its rules while they are rule-shaped.
    let mut in_rules = false;
    for a in out {
        let secret = if a.starts_with('-') {
            let (name, value) = a.split_once('=').unwrap_or((a.as_str(), ""));
            let inline = a.contains('=');
            in_rules = is_rule_flag(name) && !inline;
            if is_rule_flag(name) && inline && rule_shaped(value) { token_like(value) } else { looks_secret_value(&a) }
        } else if in_rules && rule_shaped(&a) {
            token_like(&a)
        } else {
            looks_secret_value(&a)
        };
        if secret {
            dropped = true;
            if kept.last().is_some_and(|p| p.starts_with('-') && !p.contains('=')) {
                kept.pop();
            }
            continue;
        }
        kept.push(a);
    }
    (kept, dropped)
}

/// Replaces same-named workspaces of `base` with `captured` (keeping their color); appends new ones.
pub fn merge(base: &[Workspace], captured: Vec<Workspace>) -> Vec<Workspace> {
    let mut out = base.to_vec();
    for mut ws in captured {
        match out.iter().position(|b| b.name.eq_ignore_ascii_case(&ws.name)) {
            Some(i) => {
                ws.color = out[i].color.take().or(ws.color);
                out[i] = ws;
            }
            None => out.push(ws),
        }
    }
    out
}

/// For a full save: keeps what cmux cannot tell us from the previous version's same-named workspace
/// (its color, and the command of a terminal tab with the same folder and title).
pub fn carry_over(previous: &[Workspace], captured: Vec<Workspace>) -> Vec<Workspace> {
    captured
        .into_iter()
        .map(|mut ws| {
            let Some(prev) = previous.iter().find(|p| p.name.eq_ignore_ascii_case(&ws.name)) else {
                return ws;
            };
            if ws.color.is_none() {
                ws.color = prev.color.clone();
            }
            for tab in &mut ws.tabs {
                if let Tab::Terminal { title, cwd, command: command @ None } = tab {
                    *command = prev.tabs.iter().find_map(|p| match p {
                        Tab::Terminal { title: pt, cwd: pc, command } if pt == title && pc == cwd => command.clone(),
                        _ => None,
                    });
                }
            }
            ws
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::AgentRef;
    use crate::testing::{A, B, C, FakeHistory, agent_tab, agents, open_ws};

    fn open_state() -> OpenState {
        OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![
                open_ws("WA", "Alpha", "/w/a", &[("fix login", "/w/a", Some(A)), ("zsh", "/w/a", None)]),
                open_ws("WB", "Beta", "/w/b", &[]),
            ],
        }
    }

    #[test]
    fn captures_tabs_with_transcript_titles() {
        let history = agents(vec![FakeHistory::with(&[(A, "/w/a", "login fix")])]);
        let ws = capture(&open_state(), &history, &[]).unwrap().0;
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].cmux_workspace_id.as_deref(), Some("WA"));
        assert_eq!(ws[0].tabs[0], agent_tab("claude", "login fix", "/w/a", A));
        assert_eq!(ws[0].tabs[1], Tab::Terminal { title: "zsh".into(), cwd: "/w/a".into(), command: None });
    }

    #[test]
    fn captures_any_agent_with_its_flags() {
        let history = agents(vec![FakeHistory::codex(&[(B, "/w/a", "codex title")])]);
        let mut open = open_state();
        let tab = |kind: &str, id: &str, args: &[&str]| OpenTab {
            reference: None,
            title: format!("{kind} tab"),
            cwd: "/w/a".into(),
            agent: Some(AgentRef {
                kind: kind.into(),
                session_id: id.into(),
                args: args.iter().map(|a| a.to_string()).collect(),
                alt_session_id: None,
            }),
        };
        open.workspaces[1].tabs = vec![tab("codex", B, &["-a", "on-request"]), tab("aider", C, &["--x"])];
        let ws = capture(&open, &history, &["Beta".into()]).unwrap().0;
        assert_eq!(
            ws[0].tabs,
            [
                Tab::Agent {
                    title: "codex title".into(),
                    cwd: "/w/a".into(),
                    agent: "codex".into(),
                    session_id: B.into(),
                    args: vec!["-a".into(), "on-request".into()],
                },
                // Unknown to sessionsnap: kept as cmux reported it, titled from the tab.
                Tab::Agent {
                    title: "aider tab".into(),
                    cwd: "/w/a".into(),
                    agent: "aider".into(),
                    session_id: C.into(),
                    args: vec!["--x".into()],
                },
            ]
        );
    }

    #[test]
    fn leaves_out_agent_records_with_control_characters() {
        let history = agents(vec![]);
        let mut open = open_state();
        open.workspaces[1].tabs = vec![OpenTab {
            reference: None,
            title: "bad".into(),
            cwd: "/w/b".into(),
            agent: Some(AgentRef {
                kind: "codex".into(),
                session_id: A.into(),
                args: vec!["\u{3}x\r".into()],
                alt_session_id: None,
            }),
        }];
        let (ws, notes) = capture(&open, &history, &[]).unwrap();
        assert_eq!(ws[1].tabs, [Tab::Terminal { title: "bad".into(), cwd: "/w/b".into(), command: None }]);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].starts_with("Beta › bad: left out its agent session"), "{notes:?}");
    }

    #[test]
    fn drops_values_that_look_like_credentials_whatever_the_flag() {
        // Built at runtime so secret scanners don't flag the source.
        let github_token = format!("GH=ghp_{}", "a1".repeat(18));
        let args: Vec<String> = [
            "--auth",
            "sk-proj-abc123",
            "--header",
            "Authorization: Bearer abc",
            "--env",
            "OPENAI_API_KEY=x",
            "-e",
            github_token.as_str(),
            "--bearer",
            "b",
            "--authorization",
            "a",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln",
            "--model",
            "o3",
            "--sort-key",
            "name",
        ]
        .map(String::from)
        .to_vec();
        let (kept, dropped) = without_credentials(&args);
        assert!(dropped);
        assert_eq!(kept, ["--model", "o3", "--sort-key", "name"].map(String::from).to_vec());
    }

    /// Credentials inside a flag's value (inline JSON or TOML, a header) go with their flag.
    #[test]
    fn drops_values_holding_credentials_anywhere() {
        // Built at runtime so secret scanners don't flag the source.
        let ghp = format!("ghp_{}", "a1".repeat(18));
        let jwt = format!("eyJ{}.eyJ{}.{}", "hbGciOi", "zdWIiOiIx", "c2lnbmF0dXJl");
        let secret = [
            ("--mcp-config", format!(r#"{{"mcpServers":{{"gh":{{"env":{{"GITHUB_TOKEN":"{ghp}"}}}}}}}}"#)),
            (
                "--mcp-config",
                r#"{"mcpServers":{"x":{"url":"https://x","headers":{"X-API-Key":"abc123"}}}}"#.to_string(),
            ),
            ("-c", format!(r#"mcp_servers.gh.env={{GITHUB_TOKEN="{ghp}"}}"#)),
            ("-c", r#"mcp_servers.x.http_headers={"X-API-Key"="abc123"}"#.to_string()),
            ("--header", "x-api-key: abc123".to_string()),
            // A known token or a JWT anywhere in the value, not only at its start.
            ("--mcp-config", format!(r#"{{"args":["--key","{ghp}"]}}"#)),
            ("--note", format!("use {jwt} here")),
            ("-c", "mcp_servers.db.env={DB_PASSWORD='p'}".to_string()),
            ("--mcp-config", r#"{"env":{"PRIVATE_KEY_PATH" : "x", "Authorization":"Basic x"}}"#.to_string()),
        ];
        for (flag, value) in &secret {
            let args = vec!["-m".to_string(), "o3".to_string(), flag.to_string(), value.clone()];
            let (kept, dropped) = without_credentials(&args);
            assert!(dropped, "{flag} {value}");
            assert_eq!(kept, ["-m", "o3"], "{flag} {value}");
            // Written as one word too.
            let (kept, dropped) = without_credentials(&[format!("{flag}={value}")]);
            assert!(dropped && kept.is_empty(), "{flag}={value}");
        }
        let fine = [
            "--model",
            "o3",
            "--sort-key",
            "name",
            "--max-tokens",
            "100",
            "-c",
            "model=o3",
            "-c",
            "model_auto_compact_token_limit=5000",
            "--allowedTools",
            "Bash(git:*)",
            "--mcp-config",
            r#"{"mcpServers":{"gh":{"command":"npx","args":["-y","@modelcontextprotocol/server-github"]}}}"#,
            "--append-system-prompt",
            "Keep answers short: one line.",
            "--add-dir",
            "/w/task-runner",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(without_credentials(&fine), (fine.clone(), false));
    }

    /// Permission rules name commands, not credentials: a deny rule lost on restore weakens the
    /// user's safety, so `Bash(gh auth:*)` stays. A token inside one still goes.
    #[test]
    fn keeps_permission_rules_that_name_credential_commands() {
        let rules = ["Bash(gh auth:*)", "Bash(op secret:*)", "Bash(npm run auth:*)"];
        for flag in ["--disallowedTools", "--allowedTools", "--allowed-tools", "--disallowed-tools", "--tools"] {
            for rule in rules {
                let args = vec![flag.to_string(), rule.to_string()];
                assert_eq!(without_credentials(&args), (args.clone(), false), "{flag} {rule}");
                let inline = vec![format!("{flag}={rule}")];
                assert_eq!(without_credentials(&inline), (inline.clone(), false), "{flag}={rule}");
            }
            // Several rules after one flag.
            let args: Vec<String> = [flag, "Edit", "Bash(gh token:*)", "-m", "o3"].map(String::from).to_vec();
            assert_eq!(without_credentials(&args), (args.clone(), false), "{flag}");
        }
        // `key:*` and `key:)` are rule syntax wherever they appear.
        for rule in rules {
            let args = vec!["--permission-rule".to_string(), rule.to_string()];
            assert_eq!(without_credentials(&args), (args.clone(), false), "{rule}");
        }
        // A token inside a rule still goes.
        let ghp = format!("ghp_{}", "a1".repeat(18));
        let (kept, dropped) = without_credentials(&["--allowedTools".to_string(), format!("Bash(curl -H {ghp}:*)")]);
        assert!(dropped && kept.is_empty(), "{kept:?}");
    }

    /// The short generic prefixes (`sk-`, `sk_`, `akia`, `aiza`) count only before something
    /// random-looking, so paths and names that start with them stay.
    #[test]
    fn short_token_prefixes_need_a_random_looking_rest() {
        for value in ["/Users/me/src/sk-learn-experiments", "aizawa_takeshi", "/w/akiapolaau-notes", "sk_test_mode"] {
            let args = vec!["--add-dir".to_string(), value.to_string()];
            assert_eq!(without_credentials(&args), (args.clone(), false), "{value}");
        }
        // Built at runtime so secret scanners don't flag the source.
        let tokens = [
            format!("sk-{}", "a1b2c3d4e5".repeat(3)),
            format!("sk_live_{}", "x9".repeat(12)),
            format!("AKIA{}", "Q7".repeat(8)),
            format!("AIza{}", "Sy9".repeat(12)),
            format!("sk-ant-{}", "abcdefgh"),
            format!("sk-proj-{}", "abcdefgh"),
        ];
        for t in &tokens {
            let args = vec!["--add-dir".to_string(), format!("/w/{t}/x")];
            let (kept, dropped) = without_credentials(&args);
            assert!(dropped && kept.is_empty(), "{t}");
        }
    }

    /// More secret key names, connection strings with a password, and escaped JSON.
    #[test]
    fn drops_secret_keys_urls_with_passwords_and_escaped_json() {
        let ghp = format!("ghp_{}", "a1".repeat(18));
        let pw = "hunter2";
        let secret = [
            r#"{"env":{"SECRET_KEY":"abc"}}"#.to_string(),
            r#"{"env":{"AWS_SECRET_ACCESS_KEY":"abc"}}"#.to_string(),
            r#"{"env":{"AWS_ACCESS_KEY":"abc"}}"#.to_string(),
            r#"{"env":{"CLIENT_PRIVATE_PEM":"abc"}}"#.to_string(),
            format!(r#"{{"env":{{"DATABASE_URL":"postgres://u:{pw}@h/db"}}}}"#),
            format!("redis://:{pw}@cache:6379"),
            format!(r#"{{\"env\":{{\"GITHUB_TOKEN\":\"{ghp}\"}}}}"#).replace(&ghp, "x"),
        ];
        for value in &secret {
            let args = vec!["--mcp-config".to_string(), value.clone()];
            let (kept, dropped) = without_credentials(&args);
            assert!(dropped && kept.is_empty(), "{value}");
        }
        let fine = [
            "--mcp-config",
            r#"{"env":{"DATABASE_URL":"postgres://h/db","HOME_URL":"https://user@example.com/x"}}"#,
            "-c",
            "mcp_servers.x.bearer_token_env_var=GH_PAT",
            "--add-dir",
            "/w/a",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(without_credentials(&fine), (fine.clone(), false));
    }

    /// Only rule-shaped values after a rule flag skip the key rule, and `key:*)` is rule syntax
    /// only as the tail of an open `Tool(…`.
    #[test]
    fn rule_exemptions_are_narrow() {
        // A prompt-like value after a rule flag is checked as usual; the rule and flag stay.
        let args: Vec<String> = ["--allowedTools", "Edit", "DB password=hunter2"].map(String::from).to_vec();
        assert_eq!(without_credentials(&args), (vec!["--allowedTools".to_string(), "Edit".to_string()], true));
        // After the inline form nothing is exempt.
        let args: Vec<String> = ["--allowedTools=Edit", "SECRET_KEY=abc"].map(String::from).to_vec();
        assert_eq!(without_credentials(&args), (vec!["--allowedTools=Edit".to_string()], true));
        // An inline value that is not rule-shaped is checked as usual.
        let (kept, dropped) = without_credentials(&["--allowedTools=API_KEY=x".to_string()]);
        assert!(dropped && kept.is_empty(), "{kept:?}");
        // Lists of rules stay.
        for rules in ["Edit,Bash(gh auth:*)", "Edit Bash(gh auth:*) mcp__gh__get_issue", "Bash(op secret: * )"] {
            let args = vec!["--allowedTools".to_string(), rules.to_string()];
            assert_eq!(without_credentials(&args), (args.clone(), false), "{rules}");
        }
        let args = vec!["--note".to_string(), "Bash(op secret: * )".to_string()];
        assert_eq!(without_credentials(&args), (args.clone(), false));
        for value in ["api_key: *s3cretvalue", "x-api-key: *abc123def", "password:)x", "token:)x"] {
            let args = vec!["--header".to_string(), value.to_string()];
            let (kept, dropped) = without_credentials(&args);
            assert!(dropped && kept.is_empty(), "{value}: {kept:?}");
        }
    }

    /// A key whose rest is long and mixed-case counts even without a digit.
    #[test]
    fn mixed_case_rest_after_a_short_prefix_is_a_token() {
        let letters: String = ('A'..='Z').zip('a'..='z').flat_map(|(u, l)| [u, l]).take(22).collect();
        let (kept, dropped) = without_credentials(&["--x".to_string(), format!("sk-{letters}")]);
        assert!(dropped && kept.is_empty(), "{kept:?}");
        let args = vec!["--add-dir".to_string(), "/w/sk-learn-experiments-and-more-notes".to_string()];
        assert_eq!(without_credentials(&args), (args.clone(), false));
    }

    #[test]
    fn drops_flags_that_look_like_credentials() {
        let args: Vec<String> = [
            "-m",
            "o3",
            "--api-key",
            "sk-1",
            "--Auth-Token=t",
            "-c",
            "openai_api_key=k",
            "-c",
            "model=o3",
            "--config=github_token=g",
            "--password",
            "--verbose",
            "--SECRET-file",
            "/s",
            "--credentials",
            "-c",
            "model_auto_compact_token_limit=5000",
            "-c",
            "github_token=x",
            "--remote-auth-token-env",
            "OPENAI_KEY",
            "--token",
            "t",
            "-c",
            "mcp.server.API_KEY=z",
        ]
        .map(String::from)
        .to_vec();
        let (kept, dropped) = without_credentials(&args);
        assert!(dropped);
        assert_eq!(
            kept,
            [
                "-m",
                "o3",
                "-c",
                "model=o3",
                "--verbose",
                "--SECRET-file",
                "/s",
                "-c",
                "model_auto_compact_token_limit=5000",
                "--remote-auth-token-env",
                "OPENAI_KEY",
            ]
        );
        assert_eq!(without_credentials(&["-m".into(), "o3".into()]), (vec!["-m".to_string(), "o3".to_string()], false));
        let history = agents(vec![]);
        let mut open = open_state();
        open.workspaces[1].tabs = vec![OpenTab {
            reference: None,
            title: "codex".into(),
            cwd: "/w/b".into(),
            agent: Some(AgentRef {
                kind: "codex".into(),
                session_id: B.into(),
                args: vec!["--api-key".into(), "sk-1".into()],
                alt_session_id: None,
            }),
        }];
        let (ws, notes) = capture(&open, &history, &[]).unwrap();
        assert!(matches!(&ws[1].tabs[0], Tab::Agent { args, .. } if args.is_empty()));
        assert_eq!(notes, ["Beta › codex: dropped a flag that looked like a credential"]);
    }

    #[test]
    fn workspace_named_sessionsnap_is_captured() {
        let history = agents(vec![]);
        let open = OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![open_ws("W1", "Alpha", "/w/a", &[]), open_ws("W2", "sessionsnap", "~", &[])],
        };
        let all = capture(&open, &history, &[]).unwrap().0;
        assert_eq!(all.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["Alpha", "sessionsnap"]);
    }

    #[test]
    fn captured_names_are_unique() {
        let history = agents(vec![]);
        let open = OpenState {
            live: true,
            state_version: Some(1),
            workspaces: vec![
                open_ws("W1", "Alpha", "/w/a", &[]),
                open_ws("W2", "Alpha", "/w/b", &[]),
                open_ws("W3", "", "/w/c", &[]),
            ],
        };
        let ws = capture(&open, &history, &[]).unwrap().0;
        let names: Vec<&str> = ws.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "Alpha (2)", "workspace"]);
    }

    #[test]
    fn carry_over_keeps_color_and_terminal_commands() {
        let history = agents(vec![]);
        let mut prev = capture(&open_state(), &history, &[]).unwrap().0;
        prev[0].color = Some("blue".into());
        prev[0].tabs[1] = Tab::Terminal { title: "zsh".into(), cwd: "/w/a".into(), command: Some("make dev".into()) };
        let fresh = capture(&open_state(), &history, &[]).unwrap().0;
        let out = carry_over(&prev, fresh);
        assert_eq!(out[0].color.as_deref(), Some("blue"));
        assert_eq!(
            out[0].tabs[1],
            Tab::Terminal { title: "zsh".into(), cwd: "/w/a".into(), command: Some("make dev".into()) }
        );
        assert_eq!(out[1].color, None);
    }

    #[test]
    fn capture_filters_and_merge_replaces_by_name() {
        let history = agents(vec![]);
        let only_beta = capture(&open_state(), &history, &["beta".to_string()]).unwrap().0;
        assert_eq!(only_beta.len(), 1);
        let mut base = capture(&open_state(), &history, &[]).unwrap().0;
        base[1].color = Some("blue".into());
        let mut changed = only_beta;
        changed[0].cwd = "/w/b2".into();
        let merged = merge(&base, changed);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].cwd, "/w/b2");
        assert_eq!(merged[1].color.as_deref(), Some("blue"));
    }
}
