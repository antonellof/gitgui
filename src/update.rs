//! Update check: is a newer gitgui released on GitHub?
//!
//! The version list comes from `git ls-remote --tags` on the public
//! repository, so there is no HTTP client in the dependency list and the
//! user's proxy, CA and credential configuration apply unchanged. The check
//! runs on its own thread, never blocks the UI, and caches its answer for a
//! day in the user cache directory. `GITGUI_NO_UPDATE_CHECK=1` or
//! `--no-update-check` disables it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Repository the releases live in.
pub const REPO: &str = "https://github.com/antonellof/gitgui.git";
/// Page the footer notice sends the user to.
pub const RELEASES_URL: &str = "https://github.com/antonellof/gitgui/releases/latest";

/// The version this binary was built as.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// How long a check result is reused before the network is touched again.
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The newest released version, when it is newer than [`CURRENT`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Available {
    pub latest: String,
}

impl Available {
    /// Footer / terminal wording, "0.8.0 available".
    pub fn label(&self) -> String {
        format!("v{} available", self.latest)
    }
}

/// Handle the UI holds. Cheap to clone, `None` until the check answers.
#[derive(Debug, Clone, Default)]
pub struct Check(Arc<Mutex<Option<Available>>>);

impl Check {
    /// Start the background check unless the user turned it off.
    ///
    /// `GITGUI_UPDATE_LATEST=<version>` stands in for the network and
    /// resolves right here, so a headless frame can show the notice.
    pub fn start(&self) {
        if !enabled() {
            return;
        }
        if let Ok(v) = std::env::var("GITGUI_UPDATE_LATEST") {
            if let (Ok(mut g), Some(a)) = (self.0.lock(), newer(&v, CURRENT)) {
                *g = Some(a);
            }
            return;
        }
        let slot = self.0.clone();
        let _ = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                if let Some(a) = look_up() {
                    if let Ok(mut g) = slot.lock() {
                        *g = Some(a);
                    }
                }
            });
    }

    /// The pending notice, if the check found a newer release.
    pub fn available(&self) -> Option<Available> {
        self.0.lock().ok().and_then(|g| g.clone())
    }
}

/// False when `GITGUI_NO_UPDATE_CHECK` is set to anything but `0`.
pub fn enabled() -> bool {
    match std::env::var("GITGUI_NO_UPDATE_CHECK") {
        Ok(v) => v.is_empty() || v == "0",
        Err(_) => true,
    }
}

/// The cached answer when it is fresh, otherwise the network.
fn look_up() -> Option<Available> {
    if let Some((stamp, latest)) = read_cache() {
        if now().saturating_sub(stamp) < CACHE_TTL.as_secs() {
            return newer(&latest, CURRENT);
        }
    }
    let latest = fetch_latest()?;
    write_cache(&latest);
    newer(&latest, CURRENT)
}

/// Blocking: run `git ls-remote` and return the newest tag's version.
pub fn fetch_latest() -> Option<String> {
    use std::process::{Command, Stdio};
    let out = Command::new("git")
        .args([
            // Give up on a stalled connection instead of hanging forever.
            "-c",
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=10",
            "ls-remote",
            "--tags",
            "--refs",
            REPO,
            "v*",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    newest_tag(&String::from_utf8_lossy(&out.stdout))
}

/// Highest version in `git ls-remote --tags` output.
pub fn newest_tag(ls_remote: &str) -> Option<String> {
    ls_remote
        .lines()
        .filter_map(|l| l.split_once("refs/tags/"))
        .filter_map(|(_, tag)| parse(tag.trim()).map(|v| (v, tag.trim().trim_start_matches('v').to_owned())))
        .max_by_key(|(v, _)| *v)
        .map(|(_, tag)| tag)
}

/// `Some` when `latest` is strictly newer than `current`.
pub fn newer(latest: &str, current: &str) -> Option<Available> {
    let (l, c) = (parse(latest)?, parse(current)?);
    (l > c).then(|| Available {
        latest: latest.trim_start_matches('v').to_owned(),
    })
}

/// `v1.2.3` or `1.2.3` into comparable numbers. Anything else is skipped, so
/// a pre-release tag never wins over a release.
fn parse(s: &str) -> Option<[u32; 3]> {
    let mut out = [0u32; 3];
    let mut parts = s.trim().trim_start_matches('v').split('.');
    for slot in out.iter_mut() {
        *slot = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `$XDG_CACHE_HOME/gitgui/update` or `~/.cache/gitgui/update`.
fn cache_path() -> Option<PathBuf> {
    let dir = match std::env::var_os("XDG_CACHE_HOME") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
    };
    Some(dir.join("gitgui").join("update"))
}

/// `<unix seconds> <version>` written by the last successful check.
fn read_cache() -> Option<(u64, String)> {
    let text = std::fs::read_to_string(cache_path()?).ok()?;
    parse_cache(&text)
}

pub fn parse_cache(text: &str) -> Option<(u64, String)> {
    let (stamp, version) = text.trim().split_once(char::is_whitespace)?;
    Some((stamp.parse().ok()?, version.trim().to_owned()))
}

fn write_cache(latest: &str) {
    if let Some(p) = cache_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, format!("{} {latest}\n", now()));
    }
}

/// `--check-update`: print the running and the released version, no UI.
pub fn run_check() -> anyhow::Result<i32> {
    match fetch_latest() {
        Some(latest) => {
            match newer(&latest, CURRENT) {
                Some(_) => println!(
                    "gitgui {CURRENT}: {latest} is available\n  {RELEASES_URL}"
                ),
                None => println!("gitgui {CURRENT} is up to date (latest release {latest})"),
            }
            Ok(0)
        }
        None => {
            eprintln!("gitgui {CURRENT}: cannot reach {REPO}");
            Ok(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_highest_tag() {
        let out = "\
8e1f0b1c1ea0f0e9a1b2c3d4e5f60718293a4b5c\trefs/tags/v0.1.10
1111111111111111111111111111111111111111\trefs/tags/v0.7.8
2222222222222222222222222222222222222222\trefs/tags/v0.2.0
3333333333333333333333333333333333333333\trefs/tags/v0.8.0-rc1
";
        assert_eq!(newest_tag(out).as_deref(), Some("0.7.8"));
        assert_eq!(newest_tag("").as_deref(), None);
    }

    #[test]
    fn compares_numerically_not_lexically() {
        assert_eq!(newer("0.10.0", "0.9.9").unwrap().latest, "0.10.0");
        assert!(newer("0.7.8", "0.7.8").is_none());
        assert!(newer("0.7.7", "0.7.8").is_none());
        assert!(newer("1.0.0", "0.99.99").is_some());
        assert!(newer("nightly", "0.7.8").is_none());
    }

    #[test]
    fn labels_the_notice() {
        assert_eq!(newer("v0.9.0", "0.7.8").unwrap().label(), "v0.9.0 available");
    }

    #[test]
    fn reads_back_the_cache_line() {
        assert_eq!(parse_cache("1757462400 0.8.0\n"), Some((1757462400, "0.8.0".into())));
        assert_eq!(parse_cache("garbage"), None);
    }

    #[test]
    fn env_switch_disables_the_check() {
        // Serialized by the single-threaded assertions below, no other test
        // reads this variable.
        std::env::set_var("GITGUI_NO_UPDATE_CHECK", "1");
        assert!(!enabled());
        std::env::set_var("GITGUI_NO_UPDATE_CHECK", "0");
        assert!(enabled());
        std::env::remove_var("GITGUI_NO_UPDATE_CHECK");
        assert!(enabled());
    }
}
