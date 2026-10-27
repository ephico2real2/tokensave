//! `tokensave dashboard` as a user runs it (inkcast #264): one dashboard per
//! machine, a busy port refused with its number, a non-loopback address
//! refused, and start, stop, enable and disable end to end. Each test has its
//! own home and data folder.
//!
//! Every command runs under a deadline and every server a test starts is
//! ended when the test ends, pass or fail: a regression that lets a refused
//! start serve fails here in seconds instead of holding the run until CI's
//! job timeout.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// How long one `tokensave dashboard` command may take; a background start
/// waits up to 10 s for its server.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

fn tokensave(home: &TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tokensave"));
    command
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("TOKENSAVE_HOME", home.path().join(".tokensave"))
        .env("TOKENSAVE_SKIP_AGENT_MAINTENANCE", "1")
        .env("TOKENSAVE_SKIP_UPDATE_CHECK", "1")
        .env_remove("CODEX_HOME");
    for name in ["ENABLED", "PORT", "BIND", "DEFAULT_RANGE", "THEME"] {
        command.env_remove(format!("TOKENSAVE_DASHBOARD_{name}"));
    }
    command
}

/// A child process, killed when this value is dropped.
struct Spawned(Child);

impl Drop for Spawned {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits up to `timeout` for `child` to exit.
fn exited_within(child: &mut Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => return false,
        }
    }
}

/// Runs `command` to its end, or kills it after `timeout` and returns `None`.
fn output_within(command: &mut Command, timeout: Duration) -> Option<Output> {
    let mut child = Spawned(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?,
    );
    let stdout = drain(child.0.stdout.take());
    let stderr = drain(child.0.stderr.take());
    if !exited_within(&mut child.0, timeout) {
        return None;
    }
    Some(Output {
        status: child.0.wait().ok()?,
        stdout: stdout.join().ok()?,
        stderr: stderr.join().ok()?,
    })
}

/// Reads `pipe` to its end on a thread of its own, so a full pipe cannot
/// stall the command that writes it.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

fn run(home: &TempDir, args: &[&str]) -> Output {
    run_command(tokensave(home).args(args), args)
}

fn run_command(command: &mut Command, args: &[&str]) -> Output {
    output_within(command, COMMAND_TIMEOUT).unwrap_or_else(|| {
        panic!(
            "`tokensave {}` did not exit within {} s; it was killed",
            args.join(" "),
            COMMAND_TIMEOUT.as_secs()
        )
    })
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A loopback port nothing listens on now.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Stops whatever dashboard `home` runs when the test ends, pass or fail.
/// When `stop` itself fails, the server its state file names is killed, so a
/// regression in `stop` leaves nothing running either.
struct StopOnDrop<'a>(&'a TempDir);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        let _ = output_within(
            tokensave(self.0).args(["dashboard", "stop"]),
            COMMAND_TIMEOUT,
        );
        if let Some(pid) = recorded_pid(self.0.path()) {
            kill_tokensave(pid);
        }
    }
}

/// The PID in the dashboard's state file, while the file exists.
fn recorded_pid(home: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(home.join(".tokensave").join("dashboard.json")).ok()?;
    let state: serde_json::Value = serde_json::from_str(&text).ok()?;
    u32::try_from(state.get("pid")?.as_u64()?).ok()
}

/// Kills `pid` if it is a tokensave process; a stale state file may name a
/// PID that another program now has.
fn kill_tokensave(pid: u32) {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::new(),
    );
    if let Some(process) = system.process(pid) {
        if process.name().to_string_lossy().starts_with("tokensave") {
            process.kill();
        }
    }
}

/// Waits until `dashboard status` says the dashboard runs.
fn wait_running(home: &TempDir) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = stdout(&run(home, &["dashboard", "status"]));
        if status.contains("Dashboard running") || Instant::now() > deadline {
            return status;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn http_status(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply.lines().next().unwrap_or_default().to_string()
}

fn foreground(home: &TempDir, port: u16) -> Spawned {
    Spawned(
        tokensave(home)
            .args([
                "dashboard",
                "start",
                "--foreground",
                "--port",
                &port.to_string(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

#[test]
fn a_second_start_reports_the_running_dashboard() {
    let home = TempDir::new().unwrap();
    let _stop = StopOnDrop(&home);
    let port = free_port();
    let mut first = foreground(&home, port);
    let status = wait_running(&home);
    let pid = first.0.id().to_string();
    assert!(status.contains(&format!("PID {pid}")), "{status}");
    assert!(status.contains(&format!("port {port}")), "{status}");

    // In the background or the foreground, on its port or another one. Each
    // names a free ephemeral port, so a start that wrongly went ahead would
    // never serve on the default one.
    let (same, other) = (port.to_string(), free_port().to_string());
    for args in [
        vec!["dashboard", "start", "--port", &same],
        vec!["dashboard", "start", "--port", &other],
        vec!["dashboard", "start", "--foreground", "--port", &other],
    ] {
        let second = run(&home, &args);
        assert!(second.status.success(), "{args:?}: {}", stderr(&second));
        let text = stdout(&second);
        assert!(
            text.contains("Dashboard already running"),
            "{args:?}: {text}"
        );
        assert!(text.contains(&format!("PID {pid}")), "{args:?}: {text}");
        assert!(text.contains(&format!("port {port}")), "{args:?}: {text}");
    }

    assert_eq!(http_status(port, "/api/status"), "HTTP/1.1 200 OK");
    let stopped = run(&home, &["dashboard", "stop"]);
    assert!(
        stdout(&stopped).contains(&format!("Dashboard stopped (PID {pid})")),
        "{}",
        stderr(&stopped)
    );
    assert!(
        exited_within(&mut first.0, COMMAND_TIMEOUT),
        "the stopped dashboard did not exit"
    );
    assert!(stdout(&run(&home, &["dashboard", "status"])).contains("not running"));
}

#[test]
fn a_port_held_by_another_program_is_refused_by_number() {
    let home = TempDir::new().unwrap();
    let _stop = StopOnDrop(&home);
    let holder = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = holder.local_addr().unwrap().port().to_string();
    for args in [
        vec!["dashboard", "start", "--foreground", "--port", &port],
        vec!["dashboard", "start", "--port", &port],
    ] {
        let output = run(&home, &args);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{args:?}: {}",
            stdout(&output)
        );
        let error = stderr(&output);
        assert!(
            error.contains(&format!(
                "port {port} on 127.0.0.1 is in use by another program"
            )),
            "{args:?}: {error}"
        );
    }
    assert!(stdout(&run(&home, &["dashboard", "status"])).contains("not running"));
}

/// The addresses are from the documentation ranges (RFC 5737), which no
/// machine has: were the loopback check to regress, the start would fail to
/// listen rather than serve on a real interface. `0.0.0.0` is refused in
/// `config`'s unit tests, which never listen.
#[test]
fn an_address_other_than_loopback_is_refused() {
    let home = TempDir::new().unwrap();
    let _stop = StopOnDrop(&home);
    let port = free_port().to_string();
    let args = ["dashboard", "start", "--foreground", "--port", &port];
    let output = run_command(
        tokensave(&home)
            .env("TOKENSAVE_DASHBOARD_BIND", "192.0.2.1")
            .args(args),
        &args,
    );
    assert_eq!(output.status.code(), Some(1));
    let error = stderr(&output);
    assert!(
        error
            .contains("TOKENSAVE_DASHBOARD_BIND is \"192.0.2.1\", which is not a loopback address"),
        "{error}"
    );

    let data = home.path().join(".tokensave");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("dashboard.toml"),
        "[dashboard]\nbind = \"198.51.100.7\"\n",
    )
    .unwrap();
    let output = run(&home, &["dashboard", "start", "--port", &port]);
    assert_eq!(output.status.code(), Some(1));
    let error = stderr(&output);
    assert!(error.contains("`bind` in"), "{error}");
    assert!(error.contains("not a loopback address"), "{error}");
    assert!(stdout(&run(&home, &["dashboard", "status"])).contains("not running"));
}

#[test]
fn start_in_the_background_then_stop() {
    let home = TempDir::new().unwrap();
    let _stop = StopOnDrop(&home);
    let port = free_port();
    let started = run(&home, &["dashboard", "start", "--port", &port.to_string()]);
    assert!(started.status.success(), "{}", stderr(&started));
    let text = stdout(&started);
    assert!(
        text.contains(&format!(
            "Dashboard started: http://127.0.0.1:{port}/ (PID "
        )),
        "{text}"
    );

    // No global.db yet: the API says so and the page is not built in tests.
    assert_eq!(http_status(port, "/api/status"), "HTTP/1.1 200 OK");
    assert_eq!(
        http_status(port, "/api/summary"),
        "HTTP/1.1 503 Service Unavailable"
    );
    assert!(!home.path().join(".tokensave").join("global.db").exists());

    let stopped = run(&home, &["dashboard", "stop"]);
    assert!(
        stdout(&stopped).contains("Dashboard stopped"),
        "{}",
        stderr(&stopped)
    );
    assert!(stdout(&run(&home, &["dashboard", "stop"])).contains("not running"));
}

#[test]
fn enable_and_disable_record_the_choice_and_start_or_stop() {
    let home = TempDir::new().unwrap();
    let _stop = StopOnDrop(&home);
    let port = free_port();
    let enabled = run(&home, &["dashboard", "enable", "--port", &port.to_string()]);
    assert!(enabled.status.success(), "{}", stderr(&enabled));
    assert!(
        stdout(&enabled).contains(&format!("http://127.0.0.1:{port}/")),
        "{}",
        stdout(&enabled)
    );
    let config =
        std::fs::read_to_string(home.path().join(".tokensave").join("dashboard.toml")).unwrap();
    assert!(config.contains("enabled = true"), "{config}");
    assert!(config.contains(&format!("port = {port}")), "{config}");

    // The saved port is used without the flag.
    let status = stdout(&run(&home, &["dashboard", "status"]));
    assert!(status.contains(&format!("port {port}")), "{status}");

    let disabled = run(&home, &["dashboard", "disable"]);
    assert!(
        stdout(&disabled).contains("Dashboard stopped"),
        "{}",
        stderr(&disabled)
    );
    let config =
        std::fs::read_to_string(home.path().join(".tokensave").join("dashboard.toml")).unwrap();
    assert!(config.contains("enabled = false"), "{config}");
}
