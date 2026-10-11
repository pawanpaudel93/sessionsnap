//! Use cases: what each command does, independent of how it was asked for (CLI, JSON, web UI)
//! and how its result is shown.
//!
//! Layer rule: app may use `core`, `ports` and `adapters`; it never depends on `interfaces`, and
//! never prints or prompts.

pub mod restore;
