//! Helpers shared by unit and integration tests. Not part of ccsnap's interface.

use std::path::{Path, PathBuf};

use crate::claude::Claude;

pub const A: &str = "11111111-1111-4111-8111-111111111111";
pub const B: &str = "22222222-2222-4222-8222-222222222222";
pub const C: &str = "33333333-3333-4333-8333-333333333333";

/// Writes a minimal interactive transcript for `id`, started in `cwd`, named `title`.
pub fn write_transcript(claude_root: &Path, cwd: &str, id: &str, title: &str) -> PathBuf {
    let dir = claude_root.join("projects").join(Claude::project_dir_name(cwd));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{id}.jsonl"));
    let lines = [
        serde_json::json!({"type": "user", "cwd": cwd, "entrypoint": "cli", "sessionId": id,
                           "message": {"content": format!("start {title}")}}),
        serde_json::json!({"type": "custom-title", "customTitle": title, "sessionId": id}),
    ];
    std::fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    path
}

/// An open workspace with ref "workspace:<id>"; tabs are (title, cwd, session).
pub fn open_ws(id: &str, name: &str, cwd: &str, tabs: &[(&str, &str, Option<&str>)]) -> crate::cmux::OpenWorkspace {
    crate::cmux::OpenWorkspace {
        id: id.to_string(),
        reference: Some(format!("workspace:{id}")),
        name: name.to_string(),
        cwd: cwd.to_string(),
        git_branch: None,
        tabs: tabs
            .iter()
            .enumerate()
            .map(|(i, (title, cwd, session))| crate::cmux::OpenTab {
                reference: Some(format!("surface:{id}-{i}")),
                title: title.to_string(),
                cwd: cwd.to_string(),
                session_id: session.map(str::to_string),
            })
            .collect(),
    }
}
