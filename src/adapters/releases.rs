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

/// curl's exit code when `--max-time` or `--connect-timeout` runs out.
const TIMED_OUT: i32 = 28;

/// Seconds to connect, and for a whole small request (release metadata, a checksum) or archive.
const CONNECT_SECS: &str = "10";
const METADATA_SECS: u32 = 30;
const ARCHIVE_SECS: u32 = 300;

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
            CONNECT_SECS,
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
            CONNECT_SECS,
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

    /// Downloads with a time limit per try: 30 s for a checksum file, 300 s for an archive, so a
    /// connection that stalls can't hang `update`. A retry starts only within that time, so the
    /// whole wait stays under twice the limit.
    fn download(&self, url: &str, dest: &Path) -> Result<()> {
        let dest = dest.to_string_lossy();
        let secs = if url.ends_with(".sha256") { METADATA_SECS } else { ARCHIVE_SECS };
        let max = secs.to_string();
        // `--max-time` bounds each try; `--retry-max-time` bounds the retries together.
        let args = [
            "-fsSL",
            "--retry",
            "3",
            "--retry-max-time",
            &max,
            "--connect-timeout",
            CONNECT_SECS,
            "--max-time",
            &max,
            "-o",
            &dest,
            url,
        ];
        let (code, _) = self.curl(&args)?;
        match code {
            0 => Ok(()),
            TIMED_OUT => bail!(
                "download timed out (limit {secs} s per try, or {CONNECT_SECS} s to connect): {url}; check your \
                 connection and run `sessionsnap update` again"
            ),
            _ => bail!("download failed (curl exit {code}): {url}"),
        }
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

    /// Every curl call has an overall time limit, so a stalled connection can't hang `update`.
    #[test]
    fn downloads_have_a_time_limit() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("args");
        let script = d.path().join("curl");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$*\" >>'{}'\nexit 28\n", log.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let r = GitHubReleases { repo: "o/r".into(), curl: script.display().to_string() };
        let err = r.download("https://x/y.tar.gz", &d.path().join("y")).unwrap_err().to_string();
        assert!(err.contains("timed out (limit 300 s per try") && err.contains("https://x/y.tar.gz"), "{err}");
        let err = r.download("https://x/y.tar.gz.sha256", &d.path().join("y")).unwrap_err().to_string();
        assert!(err.contains("timed out (limit 30 s per try"), "{err}");
        let err = r.latest_tag().unwrap_err().to_string();
        assert!(err.contains("can't reach GitHub"), "{err}");
        let calls = std::fs::read_to_string(&log).unwrap();
        let calls: Vec<&str> = calls.lines().collect();
        assert_eq!(calls.len(), 4, "{calls:?}");
        assert!(calls[0].contains("--connect-timeout 10") && calls[0].contains("--max-time 300"), "{}", calls[0]);
        assert!(calls[1].contains("--connect-timeout 10") && calls[1].contains("--max-time 30 "), "{}", calls[1]);
        for c in &calls[2..] {
            assert!(c.contains("--connect-timeout 10") && c.contains("--max-time 30 "), "{c}");
        }
    }

    #[test]
    fn download_failure_names_the_url() {
        let d = tempfile::tempdir().unwrap();
        let err = fake(d.path(), ("", 22), ("", 22)).download("https://x/y.tar.gz", &d.path().join("y")).unwrap_err();
        assert!(err.to_string().contains("https://x/y.tar.gz"), "{err}");
    }
}
