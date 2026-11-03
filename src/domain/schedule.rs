//! Automatic snapshots: a snapshot saved from what is open in cmux every so often. The schedule
//! itself (when to run) belongs to the OS (`ports::Scheduler`); this is what sessionsnap records.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::error::{Exit, failure};

/// Shortest interval: a save reads all of cmux, so more often than this only adds load.
pub const MIN_INTERVAL: u64 = 60;

/// Versions a new automatic snapshot keeps when `--keep` was not given.
pub const DEFAULT_KEEP: u32 = 50;

/// Every automatic snapshot, by its stored name (`<root>/schedules.json`).
pub type Schedules = BTreeMap<String, Schedule>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    /// Seconds between saves.
    pub interval: u64,
    pub created_at: String,
    /// Stopped with `schedule stop`: no saves until `schedule start`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<LastRun>,
    /// The terminal it saves from, when not cmux (`--terminal`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<crate::domain::terminal::TerminalKind>,
    /// Versions to keep after each save (`--keep`); 0 or None (a schedule made before `--keep`,
    /// whose history nobody agreed to cut): all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep: Option<u32>,
}

impl Schedule {
    /// How many versions to keep; 0 keeps them all.
    pub fn keep(&self) -> u32 {
        self.keep.unwrap_or(0)
    }
}

/// What the latest scheduled save did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastRun {
    pub at: String,
    /// "saved", "unchanged", "skipped" or "failed".
    pub result: String,
    /// The version it wrote, or the latest when nothing changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// Why it skipped or failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Seconds in an interval written as a number and a unit: `90s`, `10m`, `2h`, `1d`; at least
/// `MIN_INTERVAL`.
pub fn parse_interval(text: &str) -> Result<u64> {
    let bad =
        || failure(Exit::Error, format!("invalid interval '{text}': use a number and a unit, e.g. 30s, 10m, 2h, 1d"));
    let t = text.trim();
    let split = t.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?;
    let (n, unit) = t.split_at(split);
    let n: u64 = n.parse().map_err(|_| bad())?;
    let unit = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return Err(bad()),
    };
    let secs = n.checked_mul(unit).ok_or_else(bad)?;
    if secs < MIN_INTERVAL {
        return Err(failure(Exit::Error, format!("interval '{text}' is too short: the shortest is 1m")));
    }
    Ok(secs)
}

/// The interval in its largest whole unit: 600 → "10m", 90 → "90s".
pub fn format_interval(secs: u64) -> String {
    [(86_400, "d"), (3600, "h"), (60, "m")]
        .iter()
        .find(|(unit, _)| secs.is_multiple_of(*unit))
        .map_or_else(|| format!("{secs}s"), |(unit, name)| format!("{}{name}", secs / unit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::exit_of;

    #[test]
    fn parses_intervals_in_each_unit() {
        assert_eq!(parse_interval("90s").unwrap(), 90);
        assert_eq!(parse_interval("10m").unwrap(), 600);
        assert_eq!(parse_interval(" 2h ").unwrap(), 7200);
        assert_eq!(parse_interval("1d").unwrap(), 86_400);
    }

    #[test]
    fn refuses_bad_or_too_short_intervals() {
        for bad in
            ["", "10", "m", "10x", "-5m", "1.5h", "10 m", "30s", "99999999999999999999d", "18446744073709551615d"]
        {
            let e = parse_interval(bad).unwrap_err();
            assert_eq!(exit_of(&e), Exit::Error, "{bad:?}");
        }
    }

    #[test]
    fn formats_in_the_largest_whole_unit() {
        assert_eq!(format_interval(600), "10m");
        assert_eq!(format_interval(90), "90s");
        assert_eq!(format_interval(7200), "2h");
        assert_eq!(format_interval(86_400 * 2), "2d");
        assert_eq!(format_interval(5400), "90m");
    }
}
