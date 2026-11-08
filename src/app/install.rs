//! Installing sessionsnap into its hosts: cmux's command palette and Claude Code's skills.

use std::path::PathBuf;

use anyhow::Result;

pub use crate::adapters::terminal::cmux::palette::{cmux_config_path, install_cmux, palette_commands};

/// The cmux palette (cmux.json commands) belongs to the macOS app; cmux on Linux is a different
/// product with another config format, so sessionsnap does not write into it there.
pub fn check_palette_supported(os: &str) -> Result<()> {
    if os == "macos" {
        return Ok(());
    }
    Err(crate::error::failure(
        crate::error::Exit::Refused,
        format!("the cmux palette commands are for cmux on macOS; not installing them on {os}"),
    ))
}

/// This program's path, for the palette commands to run.
pub fn current_exe() -> Result<String> {
    Ok(std::env::current_exe()?.to_string_lossy().into_owned())
}

/// Installs the sessionsnap skill for Claude Code and in the shared Agent Skills folder (Pi and
/// other agents); returns where it went.
pub fn install_skill() -> Result<Vec<PathBuf>> {
    use crate::adapters::agents::claude::{agent_skills_dir, install_skill, skills_dir};
    Ok(vec![install_skill(&skills_dir()?)?, install_skill(&agent_skills_dir()?)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Exit, exit_of};

    #[test]
    fn palette_is_macos_only() {
        assert!(check_palette_supported("macos").is_ok());
        let err = check_palette_supported("linux").unwrap_err();
        assert_eq!(exit_of(&err), Exit::Refused);
        assert!(err.to_string().contains("macOS"), "{err}");
    }
}
