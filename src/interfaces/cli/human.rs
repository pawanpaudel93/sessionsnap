//! Text output for people. JSON output is built in run.rs.

use serde_json::Value;

use crate::app::agents::{AgentStatus, Support};
use crate::domain::diff::Change;
use crate::domain::model::{SessionRef, Tab, Version, tab_count};
use crate::ports::SessionInfo;

/// `path` with the home folder shown as `~`, as the web page shows it.
fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && (path == home || path.starts_with(&format!("{home}/"))) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
    }
}

/// "1 tab", "3 tabs".
pub fn count(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

pub fn saved(v: &Version, warnings: &[String]) -> String {
    let mut s = format!(
        "Saved {} v{}: {}, {} ({})",
        v.snapshot,
        v.version,
        count(v.workspaces.len(), "workspace"),
        count(tab_count(&v.workspaces), "tab"),
        v.message
    );
    for w in warnings {
        s.push_str(&format!("\n  ! {w}"));
    }
    s
}

/// Snapshots saved by hand, then (when there are any) the automatic ones under their own heading.
pub fn list(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "No snapshots yet. Save one with: sessionsnap save <name>".into();
    }
    let (auto, manual): (Vec<&Value>, Vec<&Value>) = rows.iter().partition(|r| r.get("schedule").is_some());
    if auto.is_empty() {
        return list_rows(&manual);
    }
    let indent = |s: String| s.lines().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n");
    let mut out = vec![];
    if !manual.is_empty() {
        out.push(format!("Manual\n{}", indent(list_rows(&manual))));
    }
    out.push(format!("Automatic\n{}", indent(list_rows(&auto))));
    out.join("\n")
}

/// `s`, cut to `max` characters with an ellipsis.
fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { format!("{}…", s.chars().take(max - 1).collect::<String>()) }
}

fn list_rows(rows: &[&Value]) -> String {
    rows.iter()
        .map(|r| {
            if let Some(why) = r["error"].as_str() {
                let (name, v) = (r["name"].as_str().unwrap_or(""), r["latestVersion"].as_u64().unwrap_or(0));
                return format!("{name:<20} v{v:<4} {why}");
            }
            let names: Vec<&str> =
                r["workspaceNames"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let names =
                if names.is_empty() { String::new() } else { format!("\n  {}", shorten(&names.join(", "), 76)) };
            format!(
                "{:<20} v{:<4} {:>3} ws {:>4} tabs  {}  {}{names}",
                r["name"].as_str().unwrap_or(""),
                r["latestVersion"].as_u64().unwrap_or(0),
                r["workspaces"].as_u64().unwrap_or(0),
                r["tabs"].as_u64().unwrap_or(0),
                r["createdAt"].as_str().unwrap_or(""),
                match r["schedule"]["interval"].as_str() {
                    Some(every) if r["schedule"]["running"] == false => format!("every {every} (stopped)"),
                    Some(every) => format!("every {every}"),
                    None => r["message"].as_str().unwrap_or("").to_string(),
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `schedule list`: each automatic snapshot, its interval, latest version and last save.
pub fn schedules(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "No automatic snapshots. Start one with: sessionsnap schedule add <name> --interval 30m".into();
    }
    rows.iter()
        .map(|r| {
            let latest = r["latestVersion"].as_u64().map_or("no version yet".to_string(), |v| format!("v{v}"));
            let last = &r["lastRun"];
            let run = match (last["result"].as_str(), last["reason"].as_str()) {
                (Some(result), Some(why)) => format!("last run {result} ({why})"),
                (Some(result), None) => format!("last run {result} {}", last["at"].as_str().unwrap_or("")),
                _ => "not run yet".to_string(),
            };
            let state = if r["running"] == false { "stopped" } else { "running" };
            format!(
                "{:<20} every {:<5} {state:<8} {:<15} {run}",
                r["name"].as_str().unwrap_or(""),
                r["interval"].as_str().unwrap_or(""),
                latest
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// "shell" for terminal tabs, the agent's kind ("claude", "codex", …) for agent tabs.
fn tab_kind(t: &Tab) -> &str {
    match t {
        Tab::Terminal { .. } => "shell",
        Tab::Agent { agent, .. } => agent,
    }
}

pub fn show(v: &Version, warnings: &[String]) -> String {
    let width =
        v.workspaces.iter().flat_map(|w| &w.tabs).map(|t| tab_kind(t).chars().count()).max().unwrap_or(0).max(6);
    let mut s = format!("{} v{} · {} · {}\n", v.snapshot, v.version, v.created_at, v.message);
    for w in &v.workspaces {
        s.push_str(&format!("\n{} ({})  {}\n", w.name, count(w.tabs.len(), "tab"), tilde(&w.cwd)));
        for (i, t) in w.tabs.iter().enumerate() {
            s.push_str(&format!("  #{:<3} {:<width$}  {}\n", i + 1, tab_kind(t), t.title()));
        }
    }
    for w in warnings {
        s.push_str(&format!("\n! {w}"));
    }
    s.trim_end().to_string()
}

pub fn history(name: &str, rows: &[Value]) -> String {
    let mut s = format!("{name}\n");
    for r in rows {
        if let Some(why) = r["error"].as_str() {
            s.push_str(&format!("  v{:<4} {why}\n", r["version"].as_u64().unwrap_or(0)));
            continue;
        }
        s.push_str(&format!(
            "  v{:<4} {}  {} ws {} tabs  {}\n",
            r["version"].as_u64().unwrap_or(0),
            r["createdAt"].as_str().unwrap_or(""),
            r["workspaces"].as_u64().unwrap_or(0),
            r["tabs"].as_u64().unwrap_or(0),
            r["message"].as_str().unwrap_or("")
        ));
    }
    s.trim_end().to_string()
}

pub fn changes(changes: &[Change]) -> String {
    if changes.is_empty() {
        return "no changes".into();
    }
    changes
        .iter()
        .map(|c| match c {
            Change::WorkspaceAdded { workspace } => format!("+ workspace {workspace}"),
            Change::WorkspaceRemoved { workspace } => format!("- workspace {workspace}"),
            Change::TabAdded { workspace, title } => format!("+ {workspace} › {title}"),
            Change::TabRemoved { workspace, title } => format!("- {workspace} › {title}"),
            Change::TabMoved { title, from, to } => format!("→ {title}: {from} → {to}"),
            Change::TabRenamed { workspace, from, to } => format!("~ {workspace} › {from} → {to}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn plan(p: &crate::domain::plan::Plan) -> String {
    use crate::domain::model::Tab;
    use crate::domain::plan::Action;
    // A tab's command runs on restore, so it is shown, escaped: the snapshot may come from elsewhere.
    let tab = |t: &Tab| match t {
        Tab::Terminal { command: Some(c), .. } => format!("    + {} (runs: {})\n", t.title(), c.escape_debug()),
        _ => format!("    + {}\n", t.title()),
    };
    let mut s = String::new();
    for action in &p.actions {
        match action {
            Action::CreateWorkspace { name, tabs, .. } => {
                s.push_str(&format!("+ create workspace {name} ({})\n", count(tabs.len(), "tab")));
                tabs.iter().for_each(|t| s.push_str(&tab(t)));
            }
            Action::AddTabs { workspace_name, tabs, .. } => {
                s.push_str(&format!("~ add to {workspace_name}:\n"));
                tabs.iter().for_each(|t| s.push_str(&tab(t)));
            }
        }
    }
    if !p.skipped.is_empty() {
        s.push_str(&format!("= {} already open or skipped\n", count(p.skipped.len(), "tab")));
    }
    for w in &p.warnings {
        s.push_str(&format!("! {w}\n"));
    }
    s.trim_end().to_string()
}

pub fn steps(steps: &[crate::domain::plan::ApplyStep]) -> String {
    steps
        .iter()
        .map(|s| {
            if s.ok {
                format!("✓ {}", s.description)
            } else {
                format!("✗ {}: {}", s.description, s.error.as_deref().unwrap_or("failed"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn sessions(list: &[SessionInfo], open: &std::collections::HashSet<SessionRef>) -> String {
    if list.is_empty() {
        return "No sessions.".into();
    }
    let width = list.iter().map(|s| s.agent.chars().count()).max().unwrap_or(0);
    list.iter()
        .map(|s| {
            let mark = if open.contains(&SessionRef::new(&s.agent, &s.session_id)) { "●" } else { " " };
            format!(
                "{mark} {:<width$}  {}  {}  {}  {}",
                s.agent,
                s.session_id,
                s.modified,
                s.cwd.as_deref().unwrap_or("?"),
                s.label()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn agents(list: &[AgentStatus]) -> String {
    let kind = list.iter().map(|a| a.kind.len()).max().unwrap_or(0);
    let name = list.iter().map(|a| a.name.len()).max().unwrap_or(0);
    let resume = list.iter().map(|a| a.resume.len()).max().unwrap_or(0);
    list.iter()
        .map(|a| {
            let support = match a.support {
                Support::Full => "full",
                Support::ResumeOnly => "resume only",
            };
            let path = if a.on_path { "on PATH" } else { "not on PATH" };
            format!("{:<kind$}  {:<name$}  {:<11}  {:<resume$}  {path}", a.kind, a.name, support, a.resume)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
