use anyhow::Result;
use serde_json::Value;

use super::clean_title;
use super::state_file::FileWorkspace;
use crate::error::{Exit, failure};
use crate::ports::{OpenTab, OpenWorkspace};

#[derive(Debug, Clone, PartialEq)]
pub struct TreeWorkspace {
    pub id: String,
    pub reference: String,
    pub title: String,
    pub surfaces: Vec<TreeSurface>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TreeSurface {
    pub reference: String,
    pub title: String,
    pub tty: Option<String>,
}

/// Parses `cmux --json tree --all`; keeps terminal surfaces only, in pane order.
pub fn parse(text: &str) -> Result<Vec<TreeWorkspace>> {
    let root: Value = serde_json::from_str(text)
        .map_err(|e| failure(Exit::UnknownFormat, format!("cmux tree output is not JSON: {e}")))?;
    let mut out = vec![];
    for window in root.get("windows").and_then(Value::as_array).into_iter().flatten() {
        for ws in window.get("workspaces").and_then(Value::as_array).into_iter().flatten() {
            let mut surfaces = vec![];
            for pane in ws.get("panes").and_then(Value::as_array).into_iter().flatten() {
                for s in pane.get("surfaces").and_then(Value::as_array).into_iter().flatten() {
                    if s.get("type").and_then(Value::as_str) != Some("terminal") {
                        continue;
                    }
                    surfaces.push(TreeSurface {
                        reference: str_of(s, "ref"),
                        title: str_of(s, "title"),
                        tty: s.get("tty").and_then(Value::as_str).map(|t| norm_tty(t).to_string()),
                    });
                }
            }
            out.push(TreeWorkspace {
                id: str_of(ws, "id"),
                reference: str_of(ws, "ref"),
                title: str_of(ws, "title"),
                surfaces,
            });
        }
    }
    Ok(out)
}

/// The ref of a browser surface whose URL starts with `url` (Dock surfaces first) and whether it
/// is in the Dock, from `cmux --json tree --all`.
pub fn find_browser(text: &str, url: &str) -> Result<Option<(String, bool)>> {
    let root: Value = serde_json::from_str(text)
        .map_err(|e| failure(Exit::UnknownFormat, format!("cmux tree output is not JSON: {e}")))?;
    let mut found: Option<(bool, String)> = None;
    for window in root.get("windows").and_then(Value::as_array).into_iter().flatten() {
        for ws in window.get("workspaces").and_then(Value::as_array).into_iter().flatten() {
            for pane in ws.get("panes").and_then(Value::as_array).into_iter().flatten() {
                for s in pane.get("surfaces").and_then(Value::as_array).into_iter().flatten() {
                    let matches = s.get("type").and_then(Value::as_str) == Some("browser")
                        && s.get("url").and_then(Value::as_str).is_some_and(|u| u.starts_with(url));
                    if !matches {
                        continue;
                    }
                    let in_dock = s.get("dock_scope").is_some_and(|d| !d.is_null());
                    if found.as_ref().is_none_or(|(dock, _)| in_dock && !dock) {
                        found = Some((in_dock, str_of(s, "ref")));
                    }
                }
            }
        }
    }
    Ok(found.map(|(in_dock, reference)| (reference, in_dock)))
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn norm_tty(t: &str) -> &str {
    t.trim_start_matches("/dev/")
}

/// Live refs and order from the tree; folders and agent sessions from the state file, matched by
/// workspace ID and tty. Workspaces cmux has not written to its file yet get empty folders.
pub fn join(tree: Vec<TreeWorkspace>, file: &[FileWorkspace]) -> Vec<OpenWorkspace> {
    tree.into_iter()
        .map(|tw| {
            let fw = file.iter().find(|f| f.id == tw.id);
            let tabs = tw
                .surfaces
                .into_iter()
                .map(|s| {
                    let tty = s.tty.as_deref().map(norm_tty);
                    let panel = fw
                        .and_then(|f| f.panels.iter().find(|p| tty.is_some() && p.tty.as_deref().map(norm_tty) == tty));
                    OpenTab {
                        reference: Some(s.reference),
                        title: clean_title(&s.title),
                        cwd: panel.map(|p| p.cwd.clone()).unwrap_or_default(),
                        agent: panel.and_then(|p| p.agent.clone()),
                    }
                })
                .collect();
            OpenWorkspace {
                id: tw.id,
                reference: Some(tw.reference),
                name: fw.map(|f| f.name.clone()).unwrap_or_else(|| clean_title(&tw.title)),
                cwd: fw.map(|f| f.cwd.clone()).unwrap_or_default(),
                git_branch: fw.and_then(|f| f.git_branch.clone()),
                tabs,
            }
        })
        .collect()
}

/// Open state when cmux is not running: the state file alone, without live refs.
pub fn from_file(file: &[FileWorkspace]) -> Vec<OpenWorkspace> {
    file.iter()
        .map(|f| OpenWorkspace {
            id: f.id.clone(),
            reference: None,
            name: f.name.clone(),
            cwd: f.cwd.clone(),
            git_branch: f.git_branch.clone(),
            tabs: f
                .panels
                .iter()
                .map(|p| OpenTab {
                    reference: None,
                    title: clean_title(&p.title),
                    cwd: p.cwd.clone(),
                    agent: p.agent.clone(),
                })
                .collect(),
        })
        .collect()
}

/// A `cmux --json tree --all` reply matching tests/fixtures/cmux-state.json, plus one new workspace.
#[cfg(test)]
pub const TEST_TREE: &str = r#"{"windows":[{"ref":"window:1","workspaces":[
 {"id":"WS-ALPHA","ref":"workspace:1","title":"Alpha","panes":[{"ref":"pane:1","surfaces":[
   {"ref":"surface:2","title":"zsh","tty":"ttys002","type":"terminal"},
   {"ref":"surface:1","title":"✳ fix login","tty":"ttys001","type":"terminal"},
   {"ref":"surface:9","title":"docs","tty":null,"type":"browser"}]}]},
 {"id":"WS-NEW","ref":"workspace:7","title":"✳ Fresh","panes":[{"ref":"pane:2","surfaces":[
   {"ref":"surface:5","title":"zsh","tty":"ttys009","type":"terminal"}]}]}]}]}"#;

/// A tree with the UI open in a workspace browser tab and in the Dock.
#[cfg(test)]
pub const TEST_BROWSER_TREE: &str = r#"{"windows":[{"ref":"window:1","workspaces":[
 {"id":"WS-ALPHA","ref":"workspace:1","title":"Alpha","panes":[
   {"ref":"pane:1","surfaces":[{"ref":"surface:3","title":"sessionsnap","tty":null,"type":"browser","url":"http://127.0.0.1:47317/"}]},
   {"ref":"pane:29","dock_scope":"global","surfaces":[
     {"ref":"surface:71","title":"sessionsnap","tty":null,"type":"browser","url":"http://127.0.0.1:47317/#history","dock_scope":"global"},
     {"ref":"surface:72","title":"zsh","tty":"ttys010","type":"terminal","dock_scope":"global"}]}]}]}]}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::terminal::cmux::state_file;
    use crate::testing::{A, B};

    const FIXTURE: &str = include_str!("../../../../tests/fixtures/cmux-state.json");

    #[test]
    fn finds_ui_browser_dock_first() {
        let url = "http://127.0.0.1:47317/";
        assert_eq!(find_browser(TEST_BROWSER_TREE, url).unwrap(), Some(("surface:71".to_string(), true)));
        assert_eq!(find_browser(TEST_BROWSER_TREE, "http://127.0.0.1:9/").unwrap(), None);
        assert_eq!(find_browser(TEST_TREE, url).unwrap(), None);
    }

    #[test]
    fn joins_tree_with_state_file() {
        let (_, file) = state_file::parse(FIXTURE).unwrap();
        let ws = join(parse(TEST_TREE).unwrap(), &file);
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].reference.as_deref(), Some("workspace:1"));
        assert_eq!(ws[0].name, "Alpha");
        assert_eq!(ws[0].tabs.len(), 2);
        assert_eq!(ws[0].tabs[1].reference.as_deref(), Some("surface:1"));
        assert_eq!(ws[0].tabs[1].title, "fix login");
        assert_eq!(ws[0].tabs[1].agent.as_ref().unwrap().session_id, A);
        assert_eq!(ws[0].tabs[1].cwd, "/w/alpha/sub");
        assert_eq!(ws[1].name, "Fresh");
        assert_eq!(ws[1].tabs[0].agent, None);
    }

    #[test]
    fn offline_state_comes_from_file_only() {
        let (_, file) = state_file::parse(FIXTURE).unwrap();
        let ws = from_file(&file);
        assert_eq!(ws.len(), 2);
        assert!(ws[0].reference.is_none());
        assert_eq!(ws[1].tabs[0].agent.as_ref().unwrap().session_id, B);
    }
}
