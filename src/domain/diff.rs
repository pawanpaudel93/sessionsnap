use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::domain::model::Workspace;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "change", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum Change {
    WorkspaceAdded { workspace: String },
    WorkspaceRemoved { workspace: String },
    TabAdded { workspace: String, title: String },
    TabRemoved { workspace: String, title: String },
    TabMoved { title: String, from: String, to: String },
    TabRenamed { workspace: String, from: String, to: String },
}

/// Changes from `a` to `b`. Workspaces match by name (case-insensitive), tabs by `Tab::key`.
pub fn diff(a: &[Workspace], b: &[Workspace]) -> Vec<Change> {
    let names = |ws: &[Workspace]| ws.iter().map(|w| w.name.to_lowercase()).collect::<HashSet<_>>();
    let (na, nb) = (names(a), names(b));
    let mut out = vec![];
    out.extend(
        b.iter()
            .filter(|w| !na.contains(&w.name.to_lowercase()))
            .map(|w| Change::WorkspaceAdded { workspace: w.name.clone() }),
    );
    out.extend(
        a.iter()
            .filter(|w| !nb.contains(&w.name.to_lowercase()))
            .map(|w| Change::WorkspaceRemoved { workspace: w.name.clone() }),
    );
    let (ia, ib) = (tab_index(a), tab_index(b));
    let mut seen = HashSet::new();
    for w in b {
        for t in &w.tabs {
            let key = t.key();
            if !seen.insert(key.clone()) {
                continue;
            }
            match ia.get(&key) {
                None => out.push(Change::TabAdded { workspace: w.name.clone(), title: t.title().to_string() }),
                Some((from, _)) if !from.eq_ignore_ascii_case(&w.name) => {
                    out.push(Change::TabMoved { title: t.title().to_string(), from: from.clone(), to: w.name.clone() })
                }
                Some((_, old)) if old != t.title() => out.push(Change::TabRenamed {
                    workspace: w.name.clone(),
                    from: old.clone(),
                    to: t.title().to_string(),
                }),
                Some(_) => {}
            }
        }
    }
    for w in a {
        for t in &w.tabs {
            if !ib.contains_key(&t.key()) {
                out.push(Change::TabRemoved { workspace: w.name.clone(), title: t.title().to_string() });
            }
        }
    }
    out
}

/// key → (workspace name, title) of the first tab with that key.
fn tab_index(ws: &[Workspace]) -> HashMap<String, (String, String)> {
    let mut m = HashMap::new();
    for w in ws {
        for t in &w.tabs {
            m.entry(t.key()).or_insert_with(|| (w.name.clone(), t.title().to_string()));
        }
    }
    m
}

/// One line such as "+2 tabs, -1 workspace, 1 tab moved".
pub fn summary(changes: &[Change]) -> String {
    let count = |pred: fn(&Change) -> bool| changes.iter().filter(|c| pred(c)).count();
    let parts = [
        (count(|c| matches!(c, Change::WorkspaceAdded { .. })), "+", "workspace", ""),
        (count(|c| matches!(c, Change::WorkspaceRemoved { .. })), "-", "workspace", ""),
        (count(|c| matches!(c, Change::TabAdded { .. })), "+", "tab", ""),
        (count(|c| matches!(c, Change::TabRemoved { .. })), "-", "tab", ""),
        (count(|c| matches!(c, Change::TabMoved { .. })), "", "tab", " moved"),
        (count(|c| matches!(c, Change::TabRenamed { .. })), "", "tab", " renamed"),
    ];
    let text: Vec<String> = parts
        .iter()
        .filter(|(n, ..)| *n > 0)
        .map(|(n, sign, noun, suffix)| format!("{sign}{n} {noun}{}{suffix}", if *n == 1 { "" } else { "s" }))
        .collect();
    if text.is_empty() { "no changes".into() } else { text.join(", ") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::Tab;
    use crate::testing::{A, B, C};

    fn claude(title: &str, id: &str) -> Tab {
        crate::testing::agent_tab("claude", title, "/w", id)
    }

    fn ws(name: &str, tabs: Vec<Tab>) -> Workspace {
        Workspace { name: name.into(), color: None, cwd: "/w".into(), git_branch: None, cmux_workspace_id: None, tabs }
    }

    #[test]
    fn detects_adds_removes_moves_renames() {
        let a = vec![ws("Alpha", vec![claude("one", A), claude("two", B)]), ws("Old", vec![])];
        let b = vec![ws("Alpha", vec![claude("uno", A)]), ws("Beta", vec![claude("two", B), claude("three", C)])];
        let c = diff(&a, &b);
        assert!(c.contains(&Change::WorkspaceAdded { workspace: "Beta".into() }));
        assert!(c.contains(&Change::WorkspaceRemoved { workspace: "Old".into() }));
        assert!(c.contains(&Change::TabRenamed { workspace: "Alpha".into(), from: "one".into(), to: "uno".into() }));
        assert!(c.contains(&Change::TabMoved { title: "two".into(), from: "Alpha".into(), to: "Beta".into() }));
        assert!(c.contains(&Change::TabAdded { workspace: "Beta".into(), title: "three".into() }));
        assert_eq!(summary(&c), "+1 workspace, -1 workspace, +1 tab, 1 tab moved, 1 tab renamed");
        assert_eq!(summary(&diff(&a, &a)), "no changes");
        // Changed launch flags show as a changed tab.
        let mut flagged = a.clone();
        if let Tab::Agent { args, .. } = &mut flagged[0].tabs[0] {
            args.push("--model".into());
        }
        assert_eq!(summary(&diff(&a, &flagged)), "+1 tab, -1 tab");
    }
}
