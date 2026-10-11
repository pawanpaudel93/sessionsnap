//! Installing sessionsnap into its hosts: cmux's command palette and Claude Code's skills.

use std::path::PathBuf;

use anyhow::Result;

pub use crate::adapters::terminal::cmux::palette::{cmux_config_path, install_cmux, palette_commands};

/// This program's path, for the palette commands to run.
pub fn current_exe() -> Result<String> {
    Ok(std::env::current_exe()?.to_string_lossy().into_owned())
}

/// Installs the sessionsnap skill for Claude Code; returns its path.
pub fn install_skill() -> Result<PathBuf> {
    use crate::adapters::agents::claude::{install_skill, skills_dir};
    install_skill(&skills_dir()?)
}
