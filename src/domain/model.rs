use serde::{Deserialize, Serialize};

use anyhow::Result;

use crate::error::{Exit, failure};

/// Version of sessionsnap's own file format. 2: agent tabs (`"type": "agent"`) replace Claude tabs.
pub const FORMAT: u32 = 2;

/// One immutable version of a named snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub sessionsnap: u32,
    pub snapshot: String,
    pub version: u32,
    pub created_at: String,
    pub message: String,
    pub source: Source,
    pub workspaces: Vec<Workspace>,
}

/// Where a version came from: "live" (captured from the terminal), "scheduled", "edit", "copy"
/// or "import".
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmux_state_version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The terminal it was captured from, when not cmux.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<crate::domain::terminal::TerminalKind>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    /// Pinned in cmux when saved.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmux_workspace_id: Option<String>,
    pub tabs: Vec<Tab>,
    /// tmux: the windows its tabs sat in, in order, when one held several panes. Each takes the
    /// next `tabs` tabs and arranges them by `layout`. Empty: one window per tab.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<Window>,
}

/// Gives each workspace in `new` the layouts it had in `old` when they differ only in sizes, so
/// resizing a pane is not a change (a scheduled save would write a version for every drag).
pub fn keep_layouts(old: &[Workspace], new: &mut [Workspace]) {
    for w in new.iter_mut() {
        let Some(o) = old.iter().find(|o| o.name == w.name && o.tabs == w.tabs) else { continue };
        let shape = |ws: &[Window]| ws.iter().map(|x| (x.tabs, shape(&x.layout))).collect::<Vec<_>>();
        if shape(&o.windows) == shape(&w.windows) {
            w.windows.clone_from(&o.windows);
        }
    }
}

/// A layout without its sizes: the checksum before the first `,` dropped, every number made 0,
/// and (tmux 3.8) without which pane has focus (`"a"`) or had it last (`"l"`). Side by side
/// (`{`, `"t":"h"`) and stacked (`[`, `"t":"v"`) stay apart.
fn shape(layout: &str) -> String {
    let body = if layout.starts_with('{') { layout } else { layout.split_once(',').map_or(layout, |(_, b)| b) };
    let (mut out, mut in_number) = (String::new(), false);
    for c in body.chars() {
        let digit = c.is_ascii_digit();
        if !digit {
            out.push(c);
        } else if !in_number {
            out.push('0');
        }
        in_number = digit;
    }
    out.replace(r#","a":true"#, "").replace(r#","l":0"#, "")
}

/// True when `windows` hold exactly `tabs` tabs, each at least one (counts in an edited file may
/// be anything, so the sum must not overflow).
pub fn covers(windows: &[Window], tabs: usize) -> bool {
    windows.iter().all(|w| w.tabs > 0)
        && windows.iter().try_fold(0usize, |sum, w| sum.checked_add(w.tabs)) == Some(tabs)
}

/// A tmux window holding `tabs` of a workspace's tabs as panes, arranged by `layout` (tmux's
/// `window_layout`, e.g. "b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2}").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub tabs: usize,
    pub layout: String,
}

impl Workspace {
    /// Its windows when they cover its tabs exactly; None otherwise (an edit changed its tabs,
    /// or it has no split windows), and then each tab opens in a window of its own.
    pub fn split_windows(&self) -> Option<&[Window]> {
        (covers(&self.windows, self.tabs.len()) && self.windows.iter().any(|w| w.tabs > 1))
            .then_some(self.windows.as_slice())
    }
}

/// A saved tab: a plain terminal (optionally running a command) or a coding agent's session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum Tab {
    Terminal {
        title: String,
        cwd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    Agent {
        title: String,
        cwd: String,
        /// The agent's kind, e.g. "claude" or "codex" (kept as saved, even when unknown).
        agent: String,
        session_id: String,
        /// The flags the agent was started with, passed again when it resumes.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        /// Why the session can't be resumed from what was saved (its flags were left out because
        /// one could not be kept, e.g. a deny rule): the tab then restores as a plain shell, with
        /// this reason. Names flags only, never values.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        left_out: Option<String>,
        /// The kind of `left_out`: true when it only says the session's flags are undecided (its
        /// saved versions disagree, see `domain::known`), which a live reading of the session
        /// decides; false (and for tabs written before it existed) when a launch's flags couldn't
        /// be kept, which is for good.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        left_out_unsure: bool,
    },
}

/// One agent session: which agent, and its ID there. Two tabs hold the same session when these match.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    pub agent: String,
    pub session_id: String,
}

impl SessionRef {
    pub fn new(agent: impl Into<String>, session_id: impl Into<String>) -> Self {
        SessionRef { agent: agent.into(), session_id: session_id.into() }
    }
}

impl Tab {
    pub fn title(&self) -> &str {
        match self {
            Tab::Agent { title, .. } | Tab::Terminal { title, .. } => title,
        }
    }

    pub fn cwd(&self) -> &str {
        match self {
            Tab::Agent { cwd, .. } | Tab::Terminal { cwd, .. } => cwd,
        }
    }

    /// The agent session of an agent tab.
    pub fn session(&self) -> Option<SessionRef> {
        match self {
            Tab::Agent { agent, session_id, .. } => Some(SessionRef::new(agent, session_id)),
            Tab::Terminal { .. } => None,
        }
    }

    pub fn set_title(&mut self, new: String) {
        match self {
            Tab::Agent { title, .. } | Tab::Terminal { title, .. } => *title = new,
        }
    }

    pub fn set_cwd(&mut self, new: String) {
        match self {
            Tab::Agent { cwd, .. } | Tab::Terminal { cwd, .. } => *cwd = new,
        }
    }

    /// The first of this tab's values that holds a control character (a char below 0x20, or
    /// 0x7f), by name. Such a tab is never run: those characters would act on the terminal when
    /// the command is typed into it. The title is not checked (it is never typed).
    pub fn control_field(&self) -> Option<&'static str> {
        let fields: Vec<(&'static str, &str)> = match self {
            Tab::Terminal { cwd, command, .. } => vec![("folder", cwd), ("command", command.as_deref().unwrap_or(""))],
            Tab::Agent { cwd, agent, session_id, args, .. } => {
                let mut f =
                    vec![("agent", agent.as_str()), ("session ID", session_id.as_str()), ("folder", cwd.as_str())];
                f.extend(args.iter().map(|a| ("flags", a.as_str())));
                f
            }
        };
        fields.into_iter().find(|(_, v)| has_control(v)).map(|(name, _)| name)
    }

    /// Identity used by diffs: the session and its flags for agent tabs (so a change of flags shows
    /// as a changed tab), folder + command for terminals.
    pub fn key(&self) -> String {
        match self {
            Tab::Agent { agent, session_id, args, left_out, .. } => {
                format!("agent:{agent}:{session_id}|{}|{}", args.join("\u{1f}"), left_out.as_deref().unwrap_or(""))
            }
            Tab::Terminal { cwd, command, .. } => format!("terminal:{cwd}|{}", command.as_deref().unwrap_or("")),
        }
    }
}

/// True when `s` holds a control character: below 0x20 (newline, CR, tab, ^C, ESC …) or 0x7f.
pub fn has_control(s: &str) -> bool {
    s.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}')
}

/// True for UUIDs, the session IDs of Claude Code and Codex.
pub fn is_session_id(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(
            |(i, c)| {
                if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() }
            },
        )
}

/// A session ID as sessionsnap compares it: a UUID in lowercase (an uppercase UUID names the same
/// session, and a case-insensitive file system finds its transcript), anything else as it is.
pub fn canonical_id(id: &str) -> String {
    if is_session_id(id) { id.to_ascii_lowercase() } else { id.to_string() }
}

pub fn tab_count(workspaces: &[Workspace]) -> usize {
    workspaces.iter().map(|w| w.tabs.len()).sum()
}

/// Index of the name matching `query`: an exact case-insensitive match wins, otherwise a unique
/// case-insensitive substring. `noun` names the items in error messages.
pub fn find_by_name(names: &[&str], query: &str, noun: &str) -> anyhow::Result<usize> {
    let q = query.to_lowercase();
    if let Some(i) = names.iter().position(|n| n.to_lowercase() == q) {
        return Ok(i);
    }
    let hits: Vec<usize> =
        names.iter().enumerate().filter(|(_, n)| n.to_lowercase().contains(&q)).map(|(i, _)| i).collect();
    match hits.as_slice() {
        [i] => Ok(*i),
        [] => Err(failure(Exit::NotFound, format!("no {noun} matches '{query}'"))),
        many => Err(failure(
            Exit::Error,
            format!(
                "'{query}' matches several {noun}s: {}",
                many.iter().map(|i| names[*i]).collect::<Vec<_>>().join(", ")
            ),
        )),
    }
}

/// A name for a new snapshot: `validate_name`, and no leading `--`, which reads as an option on a
/// command line. Snapshots made before this rule keep working.
pub fn validate_new_name(name: &str) -> Result<()> {
    if name.starts_with("--") {
        return Err(failure(Exit::Error, format!("invalid snapshot name '{name}': it can't start with '--'")));
    }
    validate_name(name)
}

pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if ok {
        Ok(())
    } else {
        Err(failure(Exit::Error, format!("invalid snapshot name '{name}': use letters, digits, '.', '_' or '-'")))
    }
}

/// A version file's text as a version, or why it isn't one ("format 1, expected 2", "not valid
/// JSON: …"; an error starting "format" is another format). Used for the store's files and imports.
pub fn parse_version(text: &str) -> std::result::Result<Version, String> {
    let raw: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let format = raw.get("sessionsnap").and_then(serde_json::Value::as_u64);
    if format != Some(u64::from(FORMAT)) {
        let found = format.map_or("an unknown format".to_string(), |f| format!("format {f}"));
        return Err(format!("{found}, expected {FORMAT}"));
    }
    serde_json::from_value(raw).map_err(|e| format!("not a sessionsnap version: {e}"))
}

/// Moves every folder under a mapped one (`old` itself, or below it) to the same place under its
/// `new`: the first matching map wins. For snapshots made on another machine.
pub fn map_folders(workspaces: &mut [Workspace], maps: &[(String, String)]) {
    let map = |path: &mut String| {
        for (old, new) in maps {
            let old = old.trim_end_matches('/');
            let rest = path.strip_prefix(old).filter(|r| r.is_empty() || r.starts_with('/'));
            if let Some(rest) = rest.filter(|_| !old.is_empty()) {
                *path = format!("{}{rest}", new.trim_end_matches('/'));
                return;
            }
        }
    };
    for w in workspaces {
        map(&mut w.cwd);
        for tab in &mut w.tabs {
            match tab {
                Tab::Terminal { cwd, .. } | Tab::Agent { cwd, .. } => map(cwd),
            }
        }
    }
}

/// "work" → ("work", None); "work@3" → ("work", Some(3)); "work@latest" → ("work", None).
pub fn parse_ref(s: &str) -> Result<(String, Option<u32>)> {
    let (name, v) = match s.split_once('@') {
        Some((n, v)) => (n, Some(v)),
        None => (s, None),
    };
    validate_name(name)?;
    let version = match v {
        None | Some("latest") => None,
        Some(v) => Some(v.parse::<u32>().map_err(|_| failure(Exit::Error, format!("invalid version '{v}' in '{s}'")))?),
    };
    Ok((name.to_string(), version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_shape_ignores_sizes_but_not_orientation() {
        assert_eq!(
            shape("b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2}"),
            shape("c3f1,80x24,0,0{20x24,0,0,1,59x24,21,0,2}")
        );
        assert_ne!(
            shape("b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2}"),
            shape("b25d,80x24,0,0[80x12,0,0,1,80x11,0,13,2]")
        );
        assert_eq!(shape(r#"{"t":"h","w":160}"#), r#"{"t":"h","w":0}"#);
        // tmux 3.8 after `select-pane`: only the focus marks moved.
        let before = r#"{"V":2,"L":{"t":"h","w":160,"h":48,"x":0,"y":0,"c":[{"t":"p","w":80,"h":48,"x":0,"y":0,"l":1,"i":0,"I":"%0"},{"t":"p","w":79,"h":48,"x":81,"y":0,"a":true,"i":1,"I":"%1"}]}}"#;
        let after = r#"{"V":2,"L":{"t":"h","w":160,"h":48,"x":0,"y":0,"c":[{"t":"p","w":80,"h":48,"x":0,"y":0,"a":true,"i":0,"I":"%0"},{"t":"p","w":79,"h":48,"x":81,"y":0,"l":1,"i":1,"I":"%1"}]}}"#;
        assert_eq!(shape(before), shape(after));
        assert!(!covers(
            &[Window { tabs: usize::MAX, layout: String::new() }, Window { tabs: 2, layout: String::new() }],
            1
        ));
        assert!(!covers(&[Window { tabs: 0, layout: String::new() }], 0) && covers(&[], 0));
    }
    use crate::error::{Exit, exit_of};

    #[test]
    fn folders_move_on_path_boundaries_only() {
        let tab = |cwd: &str| Tab::Terminal { title: "t".into(), cwd: cwd.into(), command: None };
        let mut ws = vec![Workspace {
            windows: vec![],
            name: "w".into(),
            color: None,
            cwd: "/Users/a/code".into(),
            git_branch: None,
            pinned: false,
            cmux_workspace_id: None,
            tabs: vec![tab("/Users/a/code/api"), tab("/Users/ab/x"), tab("/Users/a"), tab("/opt/y")],
        }];
        let maps = [("/Users/a/".to_string(), "/home/b".to_string()), ("/".to_string(), "/never".to_string())];
        map_folders(&mut ws, &maps);
        let cwds: Vec<&str> = ws[0]
            .tabs
            .iter()
            .map(|t| match t {
                Tab::Terminal { cwd, .. } | Tab::Agent { cwd, .. } => cwd.as_str(),
            })
            .collect();
        assert_eq!(ws[0].cwd, "/home/b/code");
        assert_eq!(cwds, ["/home/b/code/api", "/Users/ab/x", "/home/b", "/opt/y"], "a bare / maps nothing");
    }

    #[test]
    fn a_version_of_another_format_or_no_json_is_refused_with_why() {
        assert!(parse_version("{").unwrap_err().starts_with("not valid JSON"));
        assert_eq!(parse_version(r#"{"sessionsnap": 1}"#).unwrap_err(), format!("format 1, expected {FORMAT}"));
        assert!(
            parse_version(&format!(r#"{{"sessionsnap": {FORMAT}}}"#)).unwrap_err().starts_with("not a sessionsnap")
        );
    }

    const ID: &str = "97aeb422-20c1-41f7-a9c6-0f7c5a658991";

    #[test]
    fn agent_tab_json_shape() {
        let t = Tab::Agent {
            title: "fix".into(),
            cwd: "/w".into(),
            agent: "codex".into(),
            session_id: ID.into(),
            args: vec!["-a".into(), "on-request".into()],
            left_out: None,
            left_out_unsure: false,
        };
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"type": "agent", "title": "fix", "cwd": "/w", "agent": "codex", "sessionId": ID,
                               "args": ["-a", "on-request"]})
        );
        assert_eq!(serde_json::from_value::<Tab>(v).unwrap(), t);
        // No args: the field is left out.
        let t = Tab::Agent {
            title: "t".into(),
            cwd: "/w".into(),
            agent: "claude".into(),
            session_id: ID.into(),
            args: vec![],
            left_out: None,
            left_out_unsure: false,
        };
        assert!(serde_json::to_value(&t).unwrap().get("args").is_none());
        assert_eq!(t.session(), Some(SessionRef::new("claude", ID)));
        // Format 1's Claude tabs are not read.
        assert!(
            serde_json::from_value::<Tab>(
                serde_json::json!({"type": "claude", "title": "t", "cwd": "/w", "sessionId": ID})
            )
            .is_err()
        );
    }

    #[test]
    fn terminal_tab_omits_missing_command() {
        let t = Tab::Terminal { title: "sh".into(), cwd: "/w".into(), command: None };
        assert_eq!(
            serde_json::to_value(&t).unwrap(),
            serde_json::json!({"type": "terminal", "title": "sh", "cwd": "/w"})
        );
    }

    #[test]
    fn session_id_validation() {
        assert!(is_session_id(ID));
        assert!(!is_session_id("work"));
        assert!(!is_session_id("97aeb422-20c1-41f7-a9c6-0f7c5a65899z"));
        assert!(!is_session_id("97aeb422;rm -rf ~;20c1-41f7-a9c6-0f7c"));
    }

    #[test]
    fn find_by_name_prefers_exact_then_unique_substring() {
        let names = ["Cascadia", "Cascadia Community", "skills", "my-skills"];
        assert_eq!(find_by_name(&names, "cascadia", "workspace").unwrap(), 0);
        assert_eq!(find_by_name(&names, "community", "workspace").unwrap(), 1);
        assert_eq!(find_by_name(&names, "skills", "workspace").unwrap(), 2);
        assert!(find_by_name(&names, "skill", "workspace").unwrap_err().to_string().contains("several"));
        assert_eq!(exit_of(&find_by_name(&names, "nope", "workspace").unwrap_err()), Exit::NotFound);
    }

    #[test]
    fn refs_and_names() {
        assert_eq!(parse_ref("work").unwrap(), ("work".to_string(), None));
        assert_eq!(parse_ref("work@3").unwrap(), ("work".to_string(), Some(3)));
        assert_eq!(parse_ref("work@latest").unwrap(), ("work".to_string(), None));
        assert!(parse_ref("work@x").is_err());
        assert!(validate_name("../etc").is_err());
        assert!(validate_name("my-day_2.0").is_ok());
        assert!(validate_name("-x").is_ok());
        assert!(validate_new_name("-x").is_ok());
        // A new name would read as an option; an existing one still works.
        for name in ["--", "--x", "--yes"] {
            assert!(validate_name(name).is_ok(), "{name}");
            let err = validate_new_name(name).unwrap_err().to_string();
            assert!(err.contains("can't start with '--'"), "{name}: {err}");
        }
    }
}
