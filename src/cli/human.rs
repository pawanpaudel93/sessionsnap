//! Text output for people. JSON output is built in run.rs.

use serde_json::Value;

use crate::diff::Change;
use crate::model::{Version, tab_count};

pub fn saved(v: &Version, warnings: &[String]) -> String {
    let mut s = format!(
        "Saved {} v{}: {} workspaces, {} tabs ({})",
        v.snapshot, v.version, v.workspaces.len(), tab_count(&v.workspaces), v.message
    );
    for w in warnings {
        s.push_str(&format!("\n  ! {w}"));
    }
    s
}

pub fn list(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "No snapshots yet. Save one with: ccsnap save <name>".into();
    }
    rows.iter()
        .map(|r| {
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

pub fn show(v: &Version, warnings: &[String]) -> String {
    let mut s = format!("{} v{} · {} · {}\n", v.snapshot, v.version, v.created_at, v.message);
    for w in &v.workspaces {
        s.push_str(&format!("\n{} ({} tabs)  {}\n", w.name, w.tabs.len(), w.cwd));
        for (i, t) in w.tabs.iter().enumerate() {
            let kind = if t.session_id().is_some() { "claude" } else { "shell " };
            s.push_str(&format!("  #{:<3} {kind}  {}\n", i + 1, t.title()));
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
