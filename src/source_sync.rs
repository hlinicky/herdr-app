//! A build from source keeps up with Herdr's author: the checkout it was built
//! from (or `HERDR_SOURCE_DIR`) fetches its `upstream` remote a little after
//! the app opens and every hour. New commits there show as a toast and in
//! Settings › Updates, whose Sync button rebases the checked-out branch onto
//! upstream, pushes it to `origin` (with lease), then builds and installs it
//! with the checkout's own installer. A conflict aborts the rebase: the
//! checkout stays as it was. A release build has none of this.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

const EVERY: Duration = Duration::from_secs(60 * 60);
const RETRY: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Status {
    /// A checkout with an `upstream` remote: the rest means something.
    pub available: bool,
    pub checkout: Option<String>,
    /// Why there's nothing to sync with (no checkout, no upstream remote).
    pub missing: Option<String>,
    pub branch: Option<String>,
    /// The upstream branch followed, e.g. `upstream/master`.
    pub upstream: Option<String>,
    /// Upstream commits this checkout doesn't have yet.
    pub behind: usize,
    pub commits: Vec<Commit>,
    pub head: Option<String>,
    /// idle, checking, fetching, rebasing, pushing, building, installed, error.
    pub state: &'static str,
    pub error: Option<String>,
    /// Something that went wrong without stopping the sync (the push).
    pub note: Option<String>,
    /// When upstream was last fetched (unix seconds).
    pub checked: Option<u64>,
    pub log: Option<String>,
}

static STATUS: Mutex<Option<Status>> = Mutex::new(None);
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
static BUSY: AtomicBool = AtomicBool::new(false);
/// The upstream head a toast was shown for (also kept in source-sync.json).
static TOLD: Mutex<Option<String>> = Mutex::new(None);
/// What the window should say once: a new upstream head, a finished sync.
static NEWS: Mutex<Vec<News>> = Mutex::new(Vec::new());

#[derive(Clone, Debug, PartialEq)]
pub enum News {
    Behind { n: usize, first: String },
    Installed,
    Failed(String),
}

pub fn status() -> Status {
    STATUS.lock().unwrap().clone().unwrap_or(Status { state: "idle", ..Status::default() })
}

fn update(change: impl FnOnce(&mut Status)) {
    let mut s = STATUS.lock().unwrap();
    let s = s.get_or_insert_with(|| Status { state: "idle", ..Status::default() });
    change(s);
}

/// Every news since the last call.
pub fn take_news() -> Vec<News> {
    std::mem::take(&mut *NEWS.lock().unwrap())
}

/// The checkout this build came from, when it's a build from source.
fn checkout() -> Option<PathBuf> {
    if option_env!("HERDR_APP_RELEASE").is_some() {
        return None;
    }
    let dir = std::env::var_os("HERDR_SOURCE_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    dir.join(".git").exists().then_some(dir)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn log_path() -> PathBuf {
    crate::mods::app_dir().join("source-sync.log")
}

fn told_path() -> PathBuf {
    crate::mods::app_dir().join("source-sync.json")
}

/// git in the checkout, never asking for anything; its output, or why it failed.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = crate::spawn::command("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_owned();
    if out.status.success() {
        Ok(text(&out.stdout))
    } else {
        let err = text(&out.stderr);
        Err(if err.is_empty() { format!("git {} failed", args.first().unwrap_or(&"")) } else { err })
    }
}

/// The upstream branch to follow: upstream's HEAD, else master, else main.
fn upstream_branch(dir: &Path) -> Option<String> {
    if let Ok(r) = git(dir, &["symbolic-ref", "--short", "refs/remotes/upstream/HEAD"])
        && !r.is_empty()
    {
        return Some(r);
    }
    ["upstream/master", "upstream/main"].into_iter().find(|b| git(dir, &["rev-parse", "--verify", "--quiet", b]).is_ok()).map(str::to_owned)
}

/// Starts the hourly look at upstream, when this is a build from source.
pub fn start() {
    clean_old_installs();
    let Some(dir) = checkout() else { return };
    let has_upstream = git(&dir, &["remote"]).is_ok_and(|r| r.lines().any(|l| l.trim() == "upstream"));
    update(|s| {
        s.checkout = Some(dir.display().to_string());
        s.log = Some(log_path().display().to_string());
        s.available = has_upstream;
        s.missing = (!has_upstream).then(|| "no upstream remote".to_owned());
    });
    if !has_upstream {
        return;
    }
    *TOLD.lock().unwrap() = std::fs::read_to_string(told_path())
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("told").and_then(|t| t.as_str()).map(str::to_owned));
    std::thread::spawn(move || {
        sleep(Duration::from_secs(15));
        loop {
            let wait = if BUSY.load(Ordering::Relaxed) {
                EVERY
            } else {
                match check(&dir) {
                    Ok(()) => EVERY,
                    Err(e) => {
                        tracing::warn!("source sync check: {e}");
                        update(|s| {
                            s.state = "error";
                            s.error = Some(e);
                        });
                        RETRY
                    }
                }
            };
            sleep(wait);
        }
    });
}

fn sleep(d: Duration) {
    let woken = WAKE.0.lock().unwrap();
    let (mut woken, _) = WAKE.1.wait_timeout_while(woken, d, |w| !*w).unwrap();
    *woken = false;
}

/// Looks at upstream now (Check now in Settings).
pub fn check_now() {
    *WAKE.0.lock().unwrap() = true;
    WAKE.1.notify_all();
}

fn check(dir: &Path) -> Result<(), String> {
    update(|s| {
        s.state = "checking";
        s.error = None;
    });
    git(dir, &["fetch", "--quiet", "upstream"])?;
    measure(dir)?;
    update(|s| {
        s.state = if s.state == "checking" { "idle" } else { s.state };
        s.checked = Some(now());
    });
    let s = status();
    if s.behind > 0 && s.head.is_some() && *TOLD.lock().unwrap() != s.head {
        *TOLD.lock().unwrap() = s.head.clone();
        let _ = crate::persist::write_atomic(&told_path(), serde_json::json!({ "told": s.head }).to_string() + "\n");
        NEWS.lock().unwrap().push(News::Behind { n: s.behind, first: s.commits.first().map(|c| c.subject.clone()).unwrap_or_default() });
    }
    Ok(())
}

/// How far the checkout is behind upstream, and what's new there.
fn measure(dir: &Path) -> Result<(), String> {
    let up = upstream_branch(dir).ok_or("upstream has no master or main branch")?;
    let range = format!("HEAD..{up}");
    let behind = git(dir, &["rev-list", "--count", &range])?.parse().unwrap_or(0);
    let commits = git(dir, &["log", "--format=%h%x09%s", "-n", "8", &range])?
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(sha, subject)| Commit { sha: sha.into(), subject: subject.into() }))
        .collect();
    let head = git(dir, &["rev-parse", &up]).ok();
    let branch = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).ok();
    update(|s| {
        s.upstream = Some(up);
        s.behind = behind;
        s.commits = commits;
        s.head = head;
        s.branch = branch;
    });
    Ok(())
}

/// Rebases onto upstream, pushes, builds and installs, in the background.
pub fn sync() -> Result<(), String> {
    let dir = checkout().ok_or("this is not a build from source")?;
    if !status().available {
        return Err("the checkout has no upstream remote".into());
    }
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err("a sync is already running".into());
    }
    update(|s| {
        s.state = "fetching";
        s.error = None;
        s.note = None;
    });
    std::thread::spawn(move || {
        let result = run_sync(&dir);
        let _ = measure(&dir);
        match result {
            Ok(()) => {
                update(|s| s.state = "installed");
                NEWS.lock().unwrap().push(News::Installed);
            }
            Err(e) => {
                tracing::warn!("source sync: {e}");
                update(|s| {
                    s.state = "error";
                    s.error = Some(e.clone());
                });
                NEWS.lock().unwrap().push(News::Failed(e));
            }
        }
        BUSY.store(false, Ordering::SeqCst);
    });
    Ok(())
}

fn run_sync(dir: &Path) -> Result<(), String> {
    let mut log = std::fs::File::create(log_path()).map_err(|e| format!("source-sync.log: {e}"))?;
    let step = |log: &mut std::fs::File, args: &[&str]| -> Result<String, String> {
        use std::io::Write;
        let _ = writeln!(log, "$ git {}", args.join(" "));
        let r = git(dir, args);
        let _ = writeln!(log, "{}\n", match &r { Ok(o) | Err(o) => o });
        r
    };
    if !step(&mut log, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err("The checkout has uncommitted changes: commit or stash them first.".into());
    }
    step(&mut log, &["fetch", "upstream"])?;
    let up = upstream_branch(dir).ok_or("upstream has no master or main branch")?;
    update(|s| s.state = "rebasing");
    if let Err(e) = step(&mut log, &["rebase", &up]) {
        let _ = step(&mut log, &["rebase", "--abort"]);
        tracing::warn!("source sync: rebase onto {up}: {e}");
        return Err("Your changes conflict with the author's; nothing changed. Rebase by hand (see the log).".into());
    }
    let branch = step(&mut log, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch != "HEAD" && step(&mut log, &["remote"]).is_ok_and(|r| r.lines().any(|l| l.trim() == "origin")) {
        update(|s| s.state = "pushing");
        if let Err(e) = step(&mut log, &["push", "--force-with-lease", "origin", &branch]) {
            tracing::warn!("source sync: push: {e}");
            update(|s| s.note = Some("Pushing to your fork failed (see the log); the build goes on.".into()));
        }
    }
    update(|s| s.state = "building");
    build(dir, &mut log)
}

/// The checkout's own installer: builds a release and installs it beside this app.
fn build(dir: &Path, log: &mut std::fs::File) -> Result<(), String> {
    use std::io::Write;
    let mut cmd = if cfg!(windows) {
        let mut c = crate::spawn::command("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", "scripts/install-windows.ps1"]);
        c
    } else {
        let mut c = crate::spawn::command("just");
        c.arg("install");
        c
    };
    // Opened from the desktop the app may not have the shell's PATH: add
    // rustup's cargo, and the LLVM the build's bindgen looks for (Windows).
    let mut path: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    path.insert(0, crate::mods::home().join(".cargo").join("bin"));
    cmd.env("PATH", std::env::join_paths(path).unwrap_or_default());
    if cfg!(windows) && std::env::var_os("LIBCLANG_PATH").is_none() && Path::new(r"C:\Program Files\LLVM\bin").is_dir() {
        cmd.env("LIBCLANG_PATH", r"C:\Program Files\LLVM\bin");
    }
    let _ = writeln!(log, "$ build and install");
    let _ = log.flush();
    let out = log.try_clone().map_err(|e| e.to_string())?;
    let err = log.try_clone().map_err(|e| e.to_string())?;
    let status = cmd.current_dir(dir).stdin(Stdio::null()).stdout(out).stderr(err).status().map_err(|e| format!("cannot start the installer: {e}"))?;
    if status.success() { Ok(()) } else { Err("The build failed (see the log).".into()) }
}

/// The installer leaves the replaced build beside the new one while it still
/// runs (`herdr-app.old-<id>`): gone once nothing runs from it.
fn clean_old_installs() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(root) = exe.parent() else { return };
    let (Some(parent), Some(name)) = (root.parent(), root.file_name().and_then(|n| n.to_str())) else { return };
    let prefix = format!("{name}.old-");
    for entry in std::fs::read_dir(parent).into_iter().flatten().flatten() {
        if entry.file_name().to_str().is_some_and(|n| n.starts_with(&prefix)) && entry.path().is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_before_anything_ran_is_idle_and_unavailable() {
        let s = Status { state: "idle", ..Status::default() };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["state"], "idle");
        assert_eq!(v["available"], false);
        assert_eq!(v["behind"], 0);
    }

    #[test]
    fn upstream_follows_its_head_else_master() {
        let dir = std::env::temp_dir().join(format!("herdr-source-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let g = |args: &[&str]| git(&dir, args).unwrap();
        g(&["init", "-q", "-b", "work"]);
        g(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "one"]);
        assert_eq!(upstream_branch(&dir), None, "no upstream yet");
        g(&["update-ref", "refs/remotes/upstream/master", "HEAD"]);
        assert_eq!(upstream_branch(&dir).as_deref(), Some("upstream/master"));
        g(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "two"]);
        g(&["update-ref", "refs/remotes/upstream/master", "HEAD"]);
        g(&["reset", "-q", "--hard", "HEAD~1"]);
        measure(&dir).unwrap();
        let s = status();
        assert_eq!(s.behind, 1);
        assert_eq!(s.commits.first().map(|c| c.subject.as_str()), Some("two"));
        assert_eq!(s.branch.as_deref(), Some("work"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
