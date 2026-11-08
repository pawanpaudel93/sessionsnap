use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::domain::model::Workspace;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "change", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum Change {
    WorkspaceAdded {
        workspace: String,
    },
    WorkspaceRemoved {
        workspace: String,
    },
    /// The same cmux workspace under a new name.
    WorkspaceRenamed {
        from: String,
        to: String,
    },
    /// Its folder, branch, color or pinning changed: `what` names which.
    WorkspaceChanged {
        workspace: String,
        what: Vec<&'static str>,
    },
    TabAdded {
        workspace: String,
        title: String,
    },
    TabRemoved {
        workspace: String,
        title: String,
    },
    TabMoved {
        title: String,
        from: String,
        to: String,
    },
    TabRenamed {
        workspace: String,
        from: String,
        to: String,
    },
}

/// Changes from `a` to `b`. Workspaces match by name (case-insensitive), then as a rename by cmux
/// workspace ID or, without one (tmux), by sharing most of their tabs; tabs match by `Tab::key`.
pub fn diff(a: &[Workspace], b: &[Workspace]) -> Vec<Change> {
    let names = |ws: &[Workspace]| ws.iter().map(|w| w.name.to_lowercase()).collect::<HashSet<_>>();
    let nb = names(b);
    // Unmatched old workspaces by cmux ID, so an unmatched new one with the same ID is a rename.
    let gone: HashMap<&str, &Workspace> = a
        .iter()
        .filter(|w| !nb.contains(&w.name.to_lowercase()))
        .filter_map(|w| Some((w.cmux_workspace_id.as_deref()?, w)))
        .collect();
    // New workspace name → its old self.
    let mut old: HashMap<String, &Workspace> = HashMap::new();
    let mut out = vec![];
    for w in b {
        let key = w.name.to_lowercase();
        if let Some(o) = a.iter().find(|o| o.name.to_lowercase() == key) {
            old.insert(key, o);
        } else if let Some(o) =
            w.cmux_workspace_id.as_deref().and_then(|id| gone.get(id).copied()).or_else(|| same_tabs(w, a, &nb, &old))
        {
            out.push(Change::WorkspaceRenamed { from: o.name.clone(), to: w.name.clone() });
            old.insert(key, o);
        } else {
            out.push(Change::WorkspaceAdded { workspace: w.name.clone() });
        }
    }
    let renamed: HashMap<String, String> = old.iter().map(|(new, o)| (o.name.to_lowercase(), new.clone())).collect();
    out.extend(
        a.iter()
            .filter(|w| !renamed.contains_key(&w.name.to_lowercase()))
            .map(|w| Change::WorkspaceRemoved { workspace: w.name.clone() }),
    );
    for w in b {
        let Some(o) = old.get(&w.name.to_lowercase()) else { continue };
        let what: Vec<&'static str> = [
            (o.cwd != w.cwd, "folder"),
            (o.git_branch != w.git_branch, "branch"),
            (o.color != w.color, "color"),
            (o.pinned != w.pinned, "pinned"),
        ]
        .into_iter()
        .filter_map(|(changed, name)| changed.then_some(name))
        .collect();
        if !what.is_empty() {
            out.push(Change::WorkspaceChanged { workspace: w.name.clone(), what });
        }
    }
    // Tabs match one for one by `Tab::key`: two shells in one folder are two tabs, so adding or
    // closing one of them is a change.
    let old_tabs: Vec<(&str, &crate::domain::model::Tab)> =
        a.iter().flat_map(|w| w.tabs.iter().map(move |t| (w.name.as_str(), t))).collect();
    let mut by_key: HashMap<String, std::collections::VecDeque<usize>> = HashMap::new();
    for (i, (_, t)) in old_tabs.iter().enumerate() {
        by_key.entry(t.key()).or_default().push_back(i);
    }
    let mut matched = vec![false; old_tabs.len()];
    for w in b {
        for t in &w.tabs {
            let Some(i) = by_key.get_mut(&t.key()).and_then(|q| q.pop_front()) else {
                out.push(Change::TabAdded { workspace: w.name.clone(), title: t.title().to_string() });
                continue;
            };
            matched[i] = true;
            let (from, old) = (old_tabs[i].0, old_tabs[i].1.title());
            if renamed.get(&from.to_lowercase()) != Some(&w.name.to_lowercase()) {
                // A tab of a renamed workspace has not moved.
                out.push(Change::TabMoved { title: t.title().to_string(), from: from.to_string(), to: w.name.clone() });
            } else if old != t.title() {
                out.push(Change::TabRenamed {
                    workspace: w.name.clone(),
                    from: old.to_string(),
                    to: t.title().to_string(),
                });
            }
        }
    }
    for (i, (from, t)) in old_tabs.iter().enumerate() {
        if !matched[i] {
            out.push(Change::TabRemoved { workspace: from.to_string(), title: t.title().to_string() });
        }
    }
    out
}

/// The unmatched workspace of `a` sharing the most tabs with `w`, if they share more than half of
/// the larger one's tabs: `w` under its old name.
fn same_tabs<'a>(
    w: &Workspace,
    a: &'a [Workspace],
    nb: &HashSet<String>,
    old: &HashMap<String, &Workspace>,
) -> Option<&'a Workspace> {
    let keys: HashSet<String> = w.tabs.iter().map(|t| t.key()).collect();
    a.iter()
        .filter(|o| !nb.contains(&o.name.to_lowercase()) && !old.values().any(|m| std::ptr::eq(*m, *o)))
        .map(|o| (o.tabs.iter().filter(|t| keys.contains(&t.key())).count(), o))
        .filter(|(shared, o)| shared * 2 > o.tabs.len().max(w.tabs.len()))
        .max_by_key(|(shared, _)| *shared)
        .map(|(_, o)| o)
}

/// One line such as "+2 tabs, -1 workspace, 1 tab moved".
pub fn summary(changes: &[Change]) -> String {
    let count = |pred: fn(&Change) -> bool| changes.iter().filter(|c| pred(c)).count();
    let parts = [
        (count(|c| matches!(c, Change::WorkspaceAdded { .. })), "+", "workspace", ""),
        (count(|c| matches!(c, Change::WorkspaceRemoved { .. })), "-", "workspace", ""),
        (count(|c| matches!(c, Change::WorkspaceRenamed { .. })), "", "workspace", " renamed"),
        (count(|c| matches!(c, Change::WorkspaceChanged { .. })), "", "workspace", " changed"),
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
        Workspace {
            windows: vec![],
            name: name.into(),
            color: None,
            cwd: "/w".into(),
            git_branch: None,
            pinned: false,
            cmux_workspace_id: None,
            tabs,
        }
    }

    /// Two shells in one folder are two tabs: opening or closing a second one is a change.
    #[test]
    fn shells_in_the_same_folder_count_one_by_one() {
        let shell = |title: &str| Tab::Terminal { title: title.into(), cwd: "/w/docs".into(), command: None };
        let one = vec![ws("docs", vec![shell("writing")])];
        let two = vec![ws("docs", vec![shell("writing"), shell("preview")])];
        assert_eq!(diff(&one, &two), [Change::TabAdded { workspace: "docs".into(), title: "preview".into() }]);
        assert_eq!(diff(&two, &one), [Change::TabRemoved { workspace: "docs".into(), title: "preview".into() }]);
        assert!(diff(&two, &two).is_empty());
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
        // cmux renamed a workspace and it moved folder: one rename and one change, no tab moved.
        let mut a = vec![ws("Alpha", vec![claude("one", A)])];
        a[0].cmux_workspace_id = Some("W1".into());
        let mut b = a.clone();
        b[0].name = "Alpha2".into();
        b[0].cwd = "/x".into();
        b[0].git_branch = Some("dev".into());
        let c = diff(&a, &b);
        assert_eq!(
            c,
            [
                Change::WorkspaceRenamed { from: "Alpha".into(), to: "Alpha2".into() },
                Change::WorkspaceChanged { workspace: "Alpha2".into(), what: vec!["folder", "branch"] },
            ]
        );
        assert_eq!(summary(&c), "1 workspace renamed, 1 workspace changed");
        // Without a cmux ID (tmux), the same tabs make it a rename.
        let (mut a2, mut b2) = (a.clone(), b.clone());
        a2[0].cmux_workspace_id = None;
        b2[0].cmux_workspace_id = None;
        assert_eq!(summary(&diff(&a2, &b2)), "1 workspace renamed, 1 workspace changed");
        // Half the tabs or fewer in common: a new workspace that a tab moved to.
        a2[0].tabs.push(claude("two", B));
        b2[0].tabs = vec![claude("two", B), claude("three", C)];
        assert_eq!(summary(&diff(&a2, &b2)), "+1 workspace, -1 workspace, +1 tab, -1 tab, 1 tab moved");
        assert_eq!(summary(&diff(&a, &a)), "no changes");
        // Changed launch flags show as a changed tab.
        let mut flagged = a.clone();
        if let Tab::Agent { args, .. } = &mut flagged[0].tabs[0] {
            args.push("--model".into());
        }
        assert_eq!(summary(&diff(&a, &flagged)), "+1 tab, -1 tab");
    }
}
