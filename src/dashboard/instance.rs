//! One dashboard per machine: a lock in tokensave's data folder, held for
//! the server's life, and a state file naming the process that holds it.
//!
//! The lock (`dashboard.lock`, an OS file lock) is the truth: it is released
//! when the process ends however it ends, so a crash leaves nothing to clean
//! up. The state file (`dashboard.json`) is read only while the lock is
//! held, so a stale one left by a killed server is never believed.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

const LOCK_FILE: &str = "dashboard.lock";
const STATE_FILE: &str = "dashboard.json";
/// Where a dashboard started in the background writes its output.
pub const LOG_FILE: &str = "dashboard.log";

/// How long a starting server keeps trying for a lock that a status check
/// holds for an instant.
const CLAIM_PATIENCE: Duration = Duration::from_millis(500);

/// The running dashboard, as its state file records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    pub pid: u32,
    pub bind: String,
    pub port: u16,
    /// Unix seconds.
    pub started_at: i64,
}

impl Instance {
    /// The page's address.
    pub fn url(&self) -> String {
        if self.bind.contains(':') {
            format!("http://[{}]:{}/", self.bind, self.port)
        } else {
            format!("http://{}:{}/", self.bind, self.port)
        }
    }
}

/// Whether a dashboard runs on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Stopped,
    /// The lock is held but the state file is not written yet.
    Starting,
    Running(Instance),
}

/// The lock, held for as long as this value lives; dropping it removes the
/// state file.
#[derive(Debug)]
pub struct Claim {
    _lock: File,
    state_path: PathBuf,
}

impl Claim {
    /// Records `instance` as the running dashboard.
    pub fn record(&self, instance: &Instance) -> io::Result<()> {
        let text = serde_json::to_string_pretty(instance).map_err(io::Error::other)?;
        let tmp = self.state_path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.state_path)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.state_path);
    }
}

/// Takes the lock in `home` (tokensave's data folder), or reports the
/// dashboard that holds it.
pub fn claim(home: &Path) -> io::Result<Result<Claim, State>> {
    std::fs::create_dir_all(home)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(home.join(LOCK_FILE))?;
    let deadline = Instant::now() + CLAIM_PATIENCE;
    loop {
        if lock.try_lock_exclusive().is_ok() {
            return Ok(Ok(Claim {
                _lock: lock,
                state_path: home.join(STATE_FILE),
            }));
        }
        if Instant::now() >= deadline {
            return Ok(Err(state(home)));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether a dashboard runs, from the lock and the state file in `home`.
pub fn state(home: &Path) -> State {
    let Ok(lock) = OpenOptions::new().read(true).open(home.join(LOCK_FILE)) else {
        return State::Stopped;
    };
    // A shared lock is granted only while no server holds its exclusive one.
    if lock.try_lock_shared().is_ok() {
        let _ = FileExt::unlock(&lock);
        return State::Stopped;
    }
    match std::fs::read_to_string(home.join(STATE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Instance>(&text).ok())
    {
        Some(instance) => State::Running(instance),
        None => State::Starting,
    }
}

/// Waits up to `timeout` for the state in `home` to satisfy `done`, and
/// returns the last state seen.
pub fn wait_for(home: &Path, timeout: Duration, done: impl Fn(&State) -> bool) -> State {
    let deadline = Instant::now() + timeout;
    loop {
        let current = state(home);
        if done(&current) || Instant::now() >= deadline {
            return current;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Asks the process `pid` to stop: `SIGTERM` where there are signals, so the
/// server shuts down cleanly; the process is ended where there are not.
/// Returns whether the request was delivered.
pub fn terminate(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System};

    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::new(),
    );
    let Some(process) = system.process(pid) else {
        return false;
    };
    process
        .kill_with(Signal::Term)
        .unwrap_or_else(|| process.kill())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn instance() -> Instance {
        Instance {
            pid: std::process::id(),
            bind: "127.0.0.1".into(),
            port: 7311,
            started_at: 1,
        }
    }

    #[test]
    fn a_held_lock_is_the_running_dashboard_and_a_second_claim_reports_it() {
        let home = tempfile::TempDir::new().unwrap();
        assert_eq!(state(home.path()), State::Stopped);
        let first = claim(home.path()).unwrap().unwrap();
        assert_eq!(state(home.path()), State::Starting);
        first.record(&instance()).unwrap();
        assert_eq!(state(home.path()), State::Running(instance()));
        assert_eq!(
            claim(home.path()).unwrap().unwrap_err(),
            State::Running(instance())
        );
        drop(first);
        assert_eq!(state(home.path()), State::Stopped);
        assert!(!home.path().join(STATE_FILE).exists());
        assert!(claim(home.path()).unwrap().is_ok());
    }

    #[test]
    fn a_state_file_without_the_lock_is_stale() {
        let home = tempfile::TempDir::new().unwrap();
        std::fs::write(home.path().join(LOCK_FILE), "").unwrap();
        std::fs::write(
            home.path().join(STATE_FILE),
            serde_json::to_string(&instance()).unwrap(),
        )
        .unwrap();
        assert_eq!(state(home.path()), State::Stopped);
    }

    #[test]
    fn an_ipv6_address_is_bracketed_in_the_url() {
        let mut v6 = instance();
        v6.bind = "::1".into();
        assert_eq!(v6.url(), "http://[::1]:7311/");
        assert_eq!(instance().url(), "http://127.0.0.1:7311/");
    }
}
