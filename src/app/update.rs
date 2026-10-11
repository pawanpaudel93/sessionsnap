//! `sessionsnap update`: replace this binary with the latest release.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::adapters::releases::GitHubReleases;
use crate::error::{Exit, failure};
use crate::ports::Releases;

pub const REPO: &str = "pawanpaudel93/sessionsnap";
/// The installer the README and the website point to.
pub const INSTALL_URL: &str = "https://sessionsnap.pawanpaudel.com.np/install.sh";

/// What an update did, or would do.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// Already on the latest release.
    Latest {
        current: String,
    },
    /// A newer release exists (`--check`); `how` is the command that installs it.
    Available {
        current: String,
        latest: String,
        how: String,
    },
    Updated {
        from: String,
        to: String,
        path: PathBuf,
    },
}

/// Updates the running binary from GitHub (only reports with `check`).
pub fn run(check: bool) -> Result<Update> {
    let target = release_target(std::env::consts::OS, std::env::consts::ARCH)?;
    let exe = std::env::current_exe()?.canonicalize().context("find this binary")?;
    update(
        &GitHubReleases::new(REPO),
        env!("CARGO_PKG_VERSION"),
        target,
        &exe,
        cargo_bin_dir().as_deref(),
        &std::env::temp_dir(),
        check,
    )
}

/// The release asset's target for this OS and CPU: one universal binary for macOS, static musl
/// builds for Linux.
pub fn release_target(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("macos", _) => Ok("universal-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        _ => Err(failure(
            Exit::Error,
            format!(
                "no sessionsnap release is built for {os} on {arch}; build from source: cargo install --git https://github.com/{REPO}"
            ),
        )),
    }
}

/// Makes a new folder only this user can open, under `parent`, with a name no one could have
/// guessed. `mkdir` fails on anything already there (a symlink too), so the folder is ours.
fn private_dir(parent: &Path) -> Result<PathBuf> {
    let mut last = None;
    for _ in 0..8 {
        let dir = parent.join(format!("sessionsnap-update-{}", unique_suffix()));
        match create_private(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("no folder")).context(format!("make a folder in {}", parent.display())))
}

/// Creates `dir` with mode 0700; fails if anything exists at that path.
fn create_private(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new().mode(0o700).create(dir).with_context(|| format!("create {}", dir.display()))?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
        anyhow::bail!("{} is not a private folder", dir.display());
    }
    Ok(())
}

/// Where `cargo install` puts binaries.
pub(crate) fn cargo_bin_dir() -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))?;
    let bin = home.join("bin");
    Some(bin.canonicalize().unwrap_or(bin))
}

fn cargo_command() -> String {
    format!("cargo install --git https://github.com/{REPO} --force")
}

/// Finds the latest release and, unless `check`, installs its `target` build over `exe` (canonical
/// path). The download goes to a private folder made under `tmp` and removed afterwards.
pub fn update(
    releases: &dyn Releases,
    current: &str,
    target: &str,
    exe: &Path,
    cargo_bin: Option<&Path>,
    tmp: &Path,
    check: bool,
) -> Result<Update> {
    let elsewhere =
        format!("install with: curl -fsSL {INSTALL_URL} | sh  or: cargo install --git https://github.com/{REPO}");
    let tag = releases
        .latest_tag()
        .map_err(|e| failure(Exit::Error, format!("couldn't check for updates: {e:#}; {elsewhere}")))?
        .ok_or_else(|| {
            failure(
                Exit::NotFound,
                format!("no sessionsnap release found (none published yet, or the repo is private); {elsewhere}"),
            )
        })?;
    let latest = tag.strip_prefix('v').unwrap_or(&tag).to_string();
    if !is_version(&latest) {
        return Err(failure(Exit::Error, format!("the latest release has an unexpected tag: {tag:?}")));
    }
    if !is_newer(&latest, current) {
        return Ok(Update::Latest { current: current.to_string() });
    }
    // `cargo install` owns its copy: replacing it would leave cargo's records wrong.
    let from_cargo = cargo_bin.is_some_and(|bin| exe.starts_with(bin));
    if check {
        let how = if from_cargo { cargo_command() } else { "sessionsnap update".to_string() };
        return Ok(Update::Available { current: current.to_string(), latest, how });
    }
    if from_cargo {
        return Err(failure(
            Exit::Error,
            format!(
                "{} was installed with cargo; update it with: {}  or switch to the installer: curl -fsSL {INSTALL_URL} | sh",
                exe.display(),
                cargo_command()
            ),
        ));
    }

    let work = private_dir(tmp)?;
    let result = install(releases, &latest, target, exe, &work);
    let _ = std::fs::remove_dir_all(&work);
    result?;
    Ok(Update::Updated { from: current.to_string(), to: latest, path: exe.to_path_buf() })
}

/// Downloads, verifies and unpacks release `latest` in `work`, then puts its binary at `exe`.
fn install(releases: &dyn Releases, latest: &str, target: &str, exe: &Path, work: &Path) -> Result<()> {
    let name = format!("sessionsnap-{latest}-{target}.tar.gz");
    let base = format!("https://github.com/{REPO}/releases/download/v{latest}");
    let archive = work.join(&name);
    let sums = work.join(format!("{name}.sha256"));
    releases.download(&format!("{base}/{name}"), &archive)?;
    releases.download(&format!("{base}/{name}.sha256"), &sums)?;
    let want = std::fs::read_to_string(&sums)?.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    let got = releases.sha256(&archive)?;
    if want.is_empty() || want != got {
        return Err(failure(
            Exit::Error,
            format!("checksum mismatch for {name} (expected {want:?}, got {got:?}); nothing was changed"),
        ));
    }
    let unpacked = work.join("unpacked");
    std::fs::create_dir_all(&unpacked)?;
    releases.extract(&archive, &unpacked)?;
    let new = unpacked.join("sessionsnap");
    if !new.is_file() {
        return Err(failure(Exit::Error, format!("{name} has no sessionsnap binary")));
    }
    let reported = releases.version_of(&new)?;
    if reported != format!("sessionsnap {latest}") {
        return Err(failure(
            Exit::Error,
            format!("the downloaded binary reports {reported:?}, expected \"sessionsnap {latest}\""),
        ));
    }
    replace(&new, exe)
}

/// Puts `new` at `exe`: a copy next to it, then a rename over it, so a running sessionsnap (an
/// open UI) keeps its old file and nothing ever sees a half-written binary.
pub fn replace(new: &Path, exe: &Path) -> Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let dir = exe.parent().context("the binary has no folder")?;
    let tmp = dir.join(format!(".sessionsnap.update-{}", unique_suffix()));
    // create_new: never write through a file or symlink someone left at that name.
    let staged = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o700).open(&tmp).and_then(|mut out| {
        let copied = std::fs::File::open(new)
            .and_then(|mut src| std::io::copy(&mut src, &mut out))
            .and_then(|_| out.set_permissions(std::fs::Permissions::from_mode(0o755)))
            .and_then(|()| std::fs::rename(&tmp, exe));
        if copied.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        copied
    });
    staged.map_err(|e| {
        failure(
            Exit::Error,
            format!(
                "can't replace {exe} ({e}); update as its owner: sudo {exe} update  or reinstall there: curl -fsSL {INSTALL_URL} | SESSIONSNAP_INSTALL_DIR={dir} sh",
                exe = exe.display(),
                dir = dir.display()
            ),
        )
    })
}

/// 16 hex digits no other process could predict.
fn unique_suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    format!("{:016x}", h.finish())
}

/// X.Y.Z with an optional pre-release (`-rc1`): the only tags that may reach a URL or file name.
fn is_version(v: &str) -> bool {
    let (num, pre) = v.split_once('-').map_or((v, None), |(n, p)| (n, Some(p)));
    let parts: Vec<&str> = num.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && pre.is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.'))
}

/// Whether `latest` is a higher X.Y.Z than `current`, by semver precedence: missing parts count
/// as 0, a pre-release ("0.2.0-rc.1") comes before its release, and pre-releases compare by their
/// dot-separated identifiers (numbers numerically and below words, so rc.2 < rc.10).
fn is_newer(latest: &str, current: &str) -> bool {
    use std::cmp::Ordering;
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    enum Id<'a> {
        Num(u64),
        Word(&'a str),
    }
    fn parse(v: &str) -> ([u64; 3], Option<Vec<Id<'_>>>) {
        let v = v.split('+').next().unwrap_or_default();
        let (num, pre) = v.split_once('-').map_or((v, None), |(n, p)| (n, Some(p)));
        let mut x = [0u64; 3];
        for (slot, p) in x.iter_mut().zip(num.split('.')) {
            *slot = p.parse().unwrap_or(0);
        }
        (x, pre.map(|p| p.split('.').map(|id| id.parse().map_or(Id::Word(id), Id::Num)).collect()))
    }
    let ((a, a_pre), (b, b_pre)) = (parse(latest), parse(current));
    match a.cmp(&b) {
        Ordering::Equal => match (a_pre, b_pre) {
            (None, Some(_)) => true,
            (Some(a), Some(b)) => a > b,
            _ => false,
        },
        o => o == Ordering::Greater,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::exit_of;
    use crate::testing::FakeReleases;

    const MAC: &str = "universal-apple-darwin";

    /// A temp dir with an installed binary at `bin/sessionsnap`; returns (dir, exe).
    fn installed() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        let exe = dir.path().join("bin/sessionsnap");
        std::fs::write(&exe, "old").unwrap();
        (dir, exe)
    }

    #[test]
    fn picks_the_asset_for_this_platform() {
        assert_eq!(release_target("macos", "aarch64").unwrap(), MAC);
        assert_eq!(release_target("macos", "x86_64").unwrap(), MAC);
        assert_eq!(release_target("linux", "x86_64").unwrap(), "x86_64-unknown-linux-musl");
        assert_eq!(release_target("linux", "aarch64").unwrap(), "aarch64-unknown-linux-musl");
        assert!(release_target("linux", "riscv64").is_err());
        assert!(release_target("windows", "x86_64").is_err());
    }

    #[test]
    fn linux_downloads_its_own_asset() {
        let (dir, exe) = installed();
        let fake = FakeReleases::new(Some("v0.2.0"));
        update(&fake, "0.1.0", "x86_64-unknown-linux-musl", &exe, None, dir.path(), false).unwrap();
        assert!(fake.downloads.borrow()[0].ends_with("/sessionsnap-0.2.0-x86_64-unknown-linux-musl.tar.gz"));
    }

    #[test]
    fn scratch_folder_is_new_private_and_removed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let work = private_dir(dir.path()).unwrap();
        assert_eq!(std::fs::metadata(&work).unwrap().permissions().mode() & 0o777, 0o700);
        assert_ne!(private_dir(dir.path()).unwrap(), work);
        // After an update nothing is left behind.
        let (home, exe) = installed();
        let tmp = home.path().join("tmp");
        std::fs::create_dir(&tmp).unwrap();
        update(&FakeReleases::new(Some("v0.2.0")), "0.1.0", MAC, &exe, None, &tmp, false).unwrap();
        assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
    }

    #[test]
    fn an_existing_path_is_refused_not_reused() {
        let dir = tempfile::tempdir().unwrap();
        let taken = dir.path().join("taken");
        std::fs::create_dir(&taken).unwrap();
        assert!(create_private(&taken).is_err());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&taken, &link).unwrap();
        assert!(create_private(&link).is_err());
        let dangling = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("nowhere"), &dangling).unwrap();
        assert!(create_private(&dangling).is_err());
        assert!(!dir.path().join("nowhere").exists());
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.99.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("0.1.0-rc1", "0.1.0"));
        assert!(is_newer("0.2.0", "0.2.0-rc1"));
        assert!(is_newer("0.2.0-rc.2", "0.2.0-rc.1"));
        assert!(is_newer("0.2.0-rc.10", "0.2.0-rc.2"));
        assert!(is_newer("0.2.0-rc.1", "0.2.0-rc"));
        assert!(is_newer("0.2.0-rc.1", "0.2.0-beta.9"));
        assert!(is_newer("0.2.0-alpha", "0.2.0-1"));
        assert!(!is_newer("0.2.0-rc.1", "0.2.0-rc.1"));
        assert!(!is_newer("0.2.0-rc.1", "0.2.0"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(is_newer("1.0.1", "1.0"));
    }

    #[test]
    fn only_plain_versions_pass() {
        assert!(is_version("0.2.0"));
        assert!(is_version("1.10.3-rc.1"));
        for bad in ["0.2", "0.2.0.1", "x.2.0", "0.2.0-", "0.2.0/../x", "0.2.0-a b", "", "0..1"] {
            assert!(!is_version(bad), "{bad}");
        }
    }

    #[test]
    fn same_version_is_latest() {
        let (dir, exe) = installed();
        let fake = FakeReleases::new(Some("v0.1.0"));
        let got = update(&fake, "0.1.0", MAC, &exe, None, dir.path(), false).unwrap();
        assert_eq!(got, Update::Latest { current: "0.1.0".into() });
        assert!(fake.downloads.borrow().is_empty());
    }

    #[test]
    fn check_reports_without_downloading() {
        let (dir, exe) = installed();
        let fake = FakeReleases::new(Some("v0.2.0"));
        let got = update(&fake, "0.1.0", MAC, &exe, None, dir.path(), true).unwrap();
        assert_eq!(
            got,
            Update::Available { current: "0.1.0".into(), latest: "0.2.0".into(), how: "sessionsnap update".into() }
        );
        assert!(fake.downloads.borrow().is_empty());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
    }

    #[test]
    fn no_release_or_odd_tag_is_an_error() {
        let (dir, exe) = installed();
        let err = update(&FakeReleases::new(None), "0.1.0", MAC, &exe, None, dir.path(), false).unwrap_err();
        assert_eq!(exit_of(&err), Exit::NotFound);
        assert!(err.to_string().contains(INSTALL_URL), "{err}");
        let err =
            update(&FakeReleases::new(Some("v1.0.0;rm")), "0.1.0", MAC, &exe, None, dir.path(), false).unwrap_err();
        assert!(err.to_string().contains("unexpected tag"), "{err}");
    }

    #[test]
    fn cargo_installs_are_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join(".cargo/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("sessionsnap");
        std::fs::write(&exe, "old").unwrap();
        let fake = FakeReleases::new(Some("v0.2.0"));
        let work = dir.path();

        let err = update(&fake, "0.1.0", MAC, &exe, Some(&bin), work, false).unwrap_err();
        assert!(err.to_string().contains(&cargo_command()), "{err}");
        assert!(fake.downloads.borrow().is_empty());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");

        match update(&fake, "0.1.0", MAC, &exe, Some(&bin), work, true).unwrap() {
            Update::Available { how, .. } => assert_eq!(how, cargo_command()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn checksum_mismatch_changes_nothing() {
        let (dir, exe) = installed();
        let mut fake = FakeReleases::new(Some("v0.2.0"));
        fake.digest = "0".repeat(64);
        let err = update(&fake, "0.1.0", MAC, &exe, None, dir.path(), false).unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"), "{err}");
        assert!(!fake.extracted.get());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
    }

    #[test]
    fn wrong_version_in_the_archive_changes_nothing() {
        let (dir, exe) = installed();
        let mut fake = FakeReleases::new(Some("v0.2.0"));
        fake.reports = "sessionsnap 0.1.9".into();
        let err = update(&fake, "0.1.0", MAC, &exe, None, dir.path(), false).unwrap_err();
        assert!(err.to_string().contains("expected \"sessionsnap 0.2.0\""), "{err}");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
    }

    #[test]
    fn newer_release_replaces_the_binary() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, exe) = installed();
        let fake = FakeReleases::new(Some("v0.2.0"));
        let got = update(&fake, "0.1.0", MAC, &exe, None, dir.path(), false).unwrap();
        assert_eq!(got, Update::Updated { from: "0.1.0".into(), to: "0.2.0".into(), path: exe.clone() });
        let base = "https://github.com/pawanpaudel93/sessionsnap/releases/download/v0.2.0";
        assert_eq!(
            *fake.downloads.borrow(),
            vec![
                format!("{base}/sessionsnap-0.2.0-universal-apple-darwin.tar.gz"),
                format!("{base}/sessionsnap-0.2.0-universal-apple-darwin.tar.gz.sha256"),
            ]
        );
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new binary");
        assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777, 0o755);
        // Only the binary is left in its folder.
        assert_eq!(std::fs::read_dir(dir.path().join("bin")).unwrap().count(), 1);
    }

    #[test]
    fn replace_keeps_the_old_inode_for_running_copies() {
        use std::os::unix::fs::MetadataExt;
        let (dir, exe) = installed();
        let new = dir.path().join("new");
        std::fs::write(&new, "new").unwrap();
        let held = std::fs::File::open(&exe).unwrap();
        let old_inode = held.metadata().unwrap().ino();
        replace(&new, &exe).unwrap();
        assert_ne!(std::fs::metadata(&exe).unwrap().ino(), old_inode);
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(std::io::read_to_string(held).unwrap(), "old");
    }

    #[test]
    fn replace_explains_an_unwritable_folder() {
        let dir = tempfile::tempdir().unwrap();
        let new = dir.path().join("new");
        std::fs::write(&new, "new").unwrap();
        let err = replace(&new, &dir.path().join("missing/sessionsnap")).unwrap_err();
        assert!(err.to_string().contains("SESSIONSNAP_INSTALL_DIR="), "{err}");
        assert!(!dir.path().join("missing").exists());
    }
}
