//! Text output for people. JSON output is built in run.rs.

use serde_json::Value;

use crate::adapters::agents::registry::Support;
use crate::app::agents::AgentStatus;
use crate::domain::diff::Change;
use crate::domain::model::{SessionRef, Tab, Version, tab_count};
use crate::ports::SessionInfo;

pub fn saved(v: &Version, warnings: &[String]) -> String {
    let mut s = format!(
        "Saved {} v{}: {} workspaces, {} tabs ({})",
        v.snapshot,
        v.version,
        v.workspaces.len(),
        tab_count(&v.workspaces),
        v.message
    );
    for w in warnings {
        s.push_str(&format!("\n  ! {w}"));
    }
    s
}

pub fn list(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "No snapshots yet. Save one with: sessionsnap save <name>".into();
    }
    rows.iter()
        .map(|r| {
            if let Some(why) = r["error"].as_str() {
                let (name, v) = (r["name"].as_str().unwrap_or(""), r["latestVersion"].as_u64().unwrap_or(0));
                return format!("{name:<20} v{v:<4} {why}");
            }
            format!(
                "{:<20} v{:<4} {:>3} ws {:>4} tabs  {}  {}",
                r["name"].as_str().unwrap_or(""),
                r["latestVersion"].as_u64().unwrap_or(0),
                r["workspaces"].as_u64().unwrap_or(0),
                r["tabs"].as_u64().unwrap_or(0),
                r["createdAt"].as_str().unwrap_or(""),
                r["message"].as_str().unwrap_or("")
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
        s.push_str(&format!("\n{} ({} tabs)  {}\n", w.name, w.tabs.len(), w.cwd));
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
    use crate::domain::plan::Action;
    let mut s = String::new();
    for action in &p.actions {
        match action {
            Action::CreateWorkspace { name, tabs, .. } => {
                s.push_str(&format!("+ create workspace {name} ({} tabs)\n", tabs.len()));
                for t in tabs {
                    s.push_str(&format!("    + {}\n", t.title()));
                }
            }
            Action::AddTabs { workspace_name, tabs, .. } => {
                s.push_str(&format!("~ add to {workspace_name}:\n"));
                for t in tabs {
                    s.push_str(&format!("    + {}\n", t.title()));
                }
            }
        }
    }
    if !p.skipped.is_empty() {
        s.push_str(&format!("= {} tab(s) already open or skipped\n", p.skipped.len()));
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
