//! Terminals sessionsnap can capture from and restore into. Today: cmux.
//!
//! Layer rule: an adapter; implements `crate::ports::Terminal`, never depends on `app` or
//! `interfaces`.

pub mod cmux;
