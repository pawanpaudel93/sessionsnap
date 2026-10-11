//! The behaviour every `SessionHistory` must have, checked against each agent's history with
//! fixture files.

use std::time::Duration;

use crate::ports::SessionHistory;

/// What a fixture history holds: `recent` (in `cwd`, titled `title`, changed now) and `old`
/// (changed two days ago); `unknown` is in no history.
pub struct Fixture<'a> {
    pub agent: &'a str,
    pub recent: &'a str,
    pub old: &'a str,
    pub unknown: &'a str,
    pub cwd: &'a str,
    pub title: &'a str,
}

pub fn check(h: &dyn SessionHistory, f: &Fixture) {
    assert!(h.contains(f.recent));
    assert!(h.contains(f.old));
    assert!(!h.contains(f.unknown));

    let info = h.find(f.recent).unwrap().expect("finds the recent session");
    assert_eq!(info.agent, f.agent);
    assert_eq!(info.session_id, f.recent);
    assert_eq!(info.cwd.as_deref(), Some(f.cwd));
    assert_eq!(info.label(), f.title);
    assert!(info.recoverable);
    assert!(!info.modified.is_empty());
    assert!(h.find(f.unknown).unwrap().is_none());

    assert_eq!(h.title_for(f.cwd, f.recent).as_deref(), Some(f.title));
    // A tab whose folder is not the session's still gets the session's title.
    assert_eq!(h.title_for("/elsewhere", f.recent).as_deref(), Some(f.title));
    assert_eq!(h.title_for(f.cwd, f.unknown), None);

    let all: Vec<String> = h.scan(None).unwrap().into_iter().map(|s| s.session_id).collect();
    assert_eq!(all, [f.recent, f.old], "newest first");
    let day: Vec<String> =
        h.scan(Some(Duration::from_secs(24 * 3600))).unwrap().into_iter().map(|s| s.session_id).collect();
    assert_eq!(day, [f.recent]);
}

/// Sets a file's modification time to `ago` before now.
pub fn age(path: &std::path::Path, ago: Duration) {
    std::fs::File::options().write(true).open(path).unwrap().set_modified(std::time::SystemTime::now() - ago).unwrap();
}
