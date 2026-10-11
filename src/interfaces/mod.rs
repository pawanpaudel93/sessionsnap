//! Interfaces: how people and programs drive sessionsnap — the command line (with `--json`) and
//! the local web UI. They parse requests, prompt, call `app` use cases and print the results.
//!
//! Layer rule: interfaces may use `app`, `domain`, `ports` and `adapters` (to build the context);
//! nothing else depends on them. The web UI speaks the CLI's command protocol (`run_json`).

pub mod cli;
pub mod web;
