//! Terminals sessionsnap can capture from and restore into: cmux and tmux.
//!
//! Layer rule: an adapter; implements `crate::ports::Terminal`, never depends on `app` or
//! `interfaces`.

pub mod cmux;
pub mod procs;
pub mod tmux;
