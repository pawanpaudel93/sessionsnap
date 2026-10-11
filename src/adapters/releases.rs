//! Releases on GitHub, reached with the tools macOS and Linux ship (`curl`, `shasum` or `sha256sum`,
//! `tar`) so the binary needs no HTTP or archive crates.
//!
//! Layer rule: an adapter; implements `crate::ports::Releases`, never depends on `app` or
//! `interfaces`.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::ports::Releases;

/// curl exit codes that mean the network or GitHub could not be reached (not an HTTP error).
const UNREACHABLE: [i32; 5] = [5, 6, 7, 28, 35];

pub struct GitHubReleases {
    /// "owner/name".
    pub repo: String,
    /// The curl program (tests point it at a fake).
    pub curl: String,
}

impl GitHubReleases {
    pub fn new(repo: &str) -> Self {
        GitHubReleases { repo: repo.to_string(), curl: "curl".to_string() }
    }
}

impl Releases for GitHubReleases {
    fn latest_tag(&self) -> Result<Option<String>> {
        let api = format!("https://api.github.com/repos/{}/releases/latest", self.repo);
        let (code, body) = self.curl(&[
            "-fsSL",
            "--connect-timeout",
            "15",
            "--max-time",
            "30",
            "-H",
            "Accept: application/vnd.github+json",
            &api,
        ])?;
        if code == 0
            && let Some(tag) = tag_from_api(&body)
        {
            return Ok(Some(tag));
        }
        // API rate-limited or blocked: GitHub's releases/latest page redirects to the tag.
        let page = format!("https://github.com/{}/releases/latest", self.repo);
        let (code, url) = self.curl(&[
            "-fsSLI",
            "--connect-timeout",
            "15",
            "--max-time",
            "30",
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            &page,
        ])?;
        if UNREACHABLE.contains(&code) {
            bail!("can't reach GitHub (curl exit {code})");
        }
        Ok(if code == 0 { tag_from_url(&url) } else { None })
    }

    fn download(&self, url: &str, dest: &Path) -> Result<()> {
        let dest = dest.to_string_lossy();
        let (code, _) = self.curl(&["-fsSL", "--retry", "3", "--connect-timeout", "15", "-o", &dest, url])?;
        if code != 0 {
            bail!("download failed (curl exit {code}): {url}");
        }
        Ok(())
    }

    fn sha256(&self, path: &Path) -> Result<String> {
        // macOS ships shasum; most Linux systems ship sha256sum (and not always perl's shasum).
        let out = match Command::new("shasum").args(["-a", "256"]).arg(path).output() {
            Ok(out) if out.status.success() => out,
            _ => Command::new("sha256sum").arg(path).output().context("run shasum or sha256sum")?,
        };
        let text = String::from_utf8_lossy(&out.stdout);
        match text.split_whitespace().next() {
            Some(hex) if out.status.success() => Ok(hex.to_ascii_lowercase()),
            _ => bail!("couldn't checksum {}", path.display()),
        }
    }

    fn extract(&self, archive: &Path, into: &Path) -> Result<()> {
        let st = Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(into).status().context("run tar")?;
        if !st.success() {
            bail!("couldn't unpack {}", archive.display());
        }
        Ok(())
    }

    fn version_of(&self, binary: &Path) -> Result<String> {
        let out = Command::new(binary).arg("--version").output().context("run the downloaded binary")?;
        if !out.status.success() {
            bail!("{} --version failed", binary.display());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

impl GitHubReleases {
    /// Runs curl; its exit code (-1 when killed) and stdout.
    fn curl(&self, args: &[&str]) -> Result<(i32, String)> {
        let out = Command::new(&self.curl).args(args).output().context("run curl")?;
        Ok((out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned()))
    }
}

/// `tag_name` from the GitHub API's release JSON.
fn tag_from_api(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("tag_name")?.as_str().filter(|t| !t.is_empty()).map(str::to_string)
}

/// The tag in a `.../releases/tag/<tag>` URL; None for any other page (no release yet).
fn tag_from_url(url: &str) -> Option<String> {
    let (_, tag) = url.trim().rsplit_once("/releases/tag/")?;
    let tag = tag.split(['?', '#', '/']).next().unwrap_or_default();
    (!tag.is_empty()).then(|| tag.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_tag_from_the_api() {
        assert_eq!(tag_from_api(r#"{"tag_name":"v0.2.0","name":"v0.2.0"}"#).as_deref(), Some("v0.2.0"));
        assert_eq!(tag_from_api(r#"{"message":"Not Found"}"#), None);
        assert_eq!(tag_from_api(r#"{"tag_name":""}"#), None);
        assert_eq!(tag_from_api("<html>"), None);
    }

    #[test]
    fn reads_the_tag_from_the_redirect() {
        let base = "https://github.com/pawanpaudel93/sessionsnap";
        assert_eq!(tag_from_url(&format!("{base}/releases/tag/v0.2.0")).as_deref(), Some("v0.2.0"));
        assert_eq!(tag_from_url(&format!("{base}/releases/tag/v0.2.0\n")).as_deref(), Some("v0.2.0"));
        // No release yet: GitHub redirects to the releases list.
        assert_eq!(tag_from_url(&format!("{base}/releases")), None);
        assert_eq!(tag_from_url(&format!("{base}/releases/tag/")), None);
    }
}

#[cfg(test)]
mod checksum_tests {
    use super::*;

    #[test]
    fn checksums_a_file_with_the_system_tool() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x");
        std::fs::write(&f, "abc").unwrap();
        let got = GitHubReleases::new("o/r").sha256(&f).unwrap();
        assert_eq!(got, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    /// A fake curl: the API call prints `api_out` and exits `api_code`; the redirect check prints
    /// `page_out` and exits `page_code`.
    fn fake(dir: &Path, api: (&str, i32), page: (&str, i32)) -> GitHubReleases {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("curl");
        let body = format!(
            "#!/bin/sh\ncase \"$*\" in\n  *api.github.com*) printf '%s' '{}'; exit {} ;;\n  *) printf '%s' '{}'; exit {} ;;\nesac\n",
            api.0, api.1, page.0, page.1
        );
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        GitHubReleases { repo: "o/r".into(), curl: script.display().to_string() }
    }

    #[test]
    fn latest_tag_from_the_api_then_the_redirect() {
        let d = tempfile::tempdir().unwrap();
        let tag = |api, page| fake(d.path(), api, page).latest_tag().map_err(|e| e.to_string());
        let page = "https://github.com/o/r/releases/tag/v0.3.0";
        assert_eq!(tag((r#"{"tag_name":"v0.2.0"}"#, 0), ("", 0)), Ok(Some("v0.2.0".into())));
        // Rate-limited or blocked API: the redirect decides.
        assert_eq!(tag(("", 22), (page, 0)), Ok(Some("v0.3.0".into())));
        assert_eq!(tag((r#"{"message":"rate limit"}"#, 0), (page, 0)), Ok(Some("v0.3.0".into())));
        // No release yet, or a private repo (404): none.
        assert_eq!(tag(("", 22), ("https://github.com/o/r/releases", 0)), Ok(None));
        assert_eq!(tag(("", 22), ("", 22)), Ok(None));
        // No network.
        let err = tag(("", 6), ("", 6)).unwrap_err();
        assert!(err.contains("can't reach GitHub"), "{err}");
    }

    #[test]
    fn download_failure_names_the_url() {
        let d = tempfile::tempdir().unwrap();
        let err = fake(d.path(), ("", 22), ("", 22)).download("https://x/y.tar.gz", &d.path().join("y")).unwrap_err();
        assert!(err.to_string().contains("https://x/y.tar.gz"), "{err}");
    }
}
