//! `tokensave cursor sync` end to end (inkcast #261): the binary, a fake
//! Cursor app login and a local server standing in for Cursor. The endpoint
//! override exists only with the `test-transport` feature, so without it
//! these tests are not built and nothing can reach Cursor.
#![cfg(feature = "test-transport")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use tempfile::TempDir;

/// A fake Cursor app token: `sub` names a user, `exp` is in 2100.
const FAKE_TOKEN: &str = "eyJhbGciOiJub25lIn0.eyJzdWIiOiJhdXRoMHx1c2VyX0ZBS0UwMDEiLCJleHAiOjQxMDI0NDQ4MDB9.FAKE-SIGNATURE-0123456789";
const FAKE_ADMIN_KEY: &str = "key_FAKE-ADMIN-0123456789";

/// Serves `responses` in order, one per connection, and records each request.
fn serve(responses: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/usage", listener.local_addr().expect("addr"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    std::thread::spawn(move || {
        for (status, body) in responses {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream);
            let mut head = String::new();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                head.push_str(&line);
            }
            let mut request_body = vec![0; length];
            let _ = reader.read_exact(&mut request_body);
            seen.lock().expect("lock").push(format!(
                "{head}\r\n{}",
                String::from_utf8_lossy(&request_body)
            ));
            let mut stream = reader.into_inner();
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, requests)
}

fn state_db_path(home: &Path) -> PathBuf {
    let relative = Path::new("Cursor/User/globalStorage/state.vscdb");
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support").join(relative)
    } else if cfg!(windows) {
        home.join("AppData/Roaming").join(relative)
    } else {
        home.join(".config").join(relative)
    }
}

/// A home with a fake Cursor app login.
fn home_with_login() -> TempDir {
    let home = TempDir::new().expect("temp home");
    let path = state_db_path(home.path());
    std::fs::create_dir_all(path.parent().expect("parent")).expect("state dir");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let db = libsql::Builder::new_local(&path).build().await.expect("db");
        let conn = db.connect().expect("conn");
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)",
            (),
        )
        .await
        .expect("table");
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('cursorAuth/accessToken', ?1)",
            libsql::params![FAKE_TOKEN],
        )
        .await
        .expect("token");
    });
    home
}

fn sync(home: &TempDir, endpoint: &str, args: &[&str], admin_key: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tokensave"));
    command
        .env("HOME", home.path())
        .env("APPDATA", home.path().join("AppData/Roaming"))
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        .env("TOKENSAVE_HOME", home.path().join(".tokensave"))
        .env("TOKENSAVE_SKIP_UPDATE_CHECK", "1")
        .env("TOKENSAVE_SKIP_AGENT_MAINTENANCE", "1")
        .env("TOKENSAVE_CURSOR_TEST_ENDPOINT", endpoint)
        .env_remove("TOKENSAVE_CURSOR_ADMIN_KEY")
        .args(["cursor", "sync"])
        .args(args);
    if let Some(key) = admin_key {
        command.env("TOKENSAVE_CURSOR_ADMIN_KEY", key);
    }
    command.output().expect("run tokensave cursor sync")
}

/// Every byte tokensave wrote: its output and every file in its data folder.
fn everything_written(home: &TempDir, output: &Output) -> Vec<u8> {
    let mut bytes = output.stdout.clone();
    bytes.extend(&output.stderr);
    for entry in walkdir::WalkDir::new(home.path().join(".tokensave"))
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_file())
    {
        bytes.extend(std::fs::read(entry.path()).unwrap_or_default());
    }
    bytes
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn assert_no_credential(home: &TempDir, output: &Output) {
    let written = everything_written(home, output);
    // The whole token, its signature, and the admin key.
    for secret in [FAKE_TOKEN, "FAKE-SIGNATURE-0123456789", FAKE_ADMIN_KEY] {
        assert!(
            !contains(&written, secret),
            "a credential was written or printed"
        );
    }
}

fn page(rows: &str, total: u64) -> String {
    format!(r#"{{"totalUsageEventsCount":{total},"usageEventsDisplay":[{rows}]}}"#)
}

const ROWS: &str = r#"
    {"usageEventId":"e1","timestamp":"1790000000000","model":"grok-4.7-xhigh","conversationId":"c1",
     "chargedCents":1.5,"tokenUsage":{"inputTokens":100,"outputTokens":20,"cacheReadTokens":300,"totalCents":1.5}},
    {"usageEventId":"e1","timestamp":"1790000000000","model":"grok-4.7-xhigh","conversationId":"c1",
     "chargedCents":4.5,"tokenUsage":{"inputTokens":300,"outputTokens":60,"cacheReadTokens":900,"totalCents":4.5}},
    {"usageEventId":"e2","timestamp":"1790000100000","model":"claude-opus-5-5-high","conversationId":"c2",
     "chargedCents":10,"tokenUsage":{"inputTokens":50,"outputTokens":5,"totalCents":10}}"#;

#[test]
fn with_no_source_it_explains_and_requests_nothing() {
    let home = home_with_login();
    let (url, requests) = serve(vec![(200, page("", 0))]);
    let output = sync(&home, &url, &[], None);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No Cursor usage source is set up"),
        "{stdout}"
    );
    assert!(requests.lock().expect("lock").is_empty());
    assert_no_credential(&home, &output);
}

#[test]
fn the_login_source_records_turns_and_never_writes_the_token() {
    let home = home_with_login();
    let (url, requests) = serve(vec![(200, page(ROWS, 3))]);
    let output = sync(&home, &url, &["--use-login"], None);
    assert!(output.status.success(), "exit {:?}", output.status.code());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Source: Cursor login"), "{stdout}");
    assert!(stdout.contains("Turns inserted: 2"), "{stdout}");
    assert!(stdout.contains("1 repeated snapshots folded"), "{stdout}");
    let seen = requests.lock().expect("lock").join("\n");
    assert!(contains(
        seen.as_bytes(),
        "WorkosCursorSessionToken=user_FAKE001%3A%3A"
    ));
    assert_no_credential(&home, &output);

    // A second run starts an hour before the newest event and changes nothing.
    let (url, requests) = serve(vec![(200, page(ROWS, 3))]);
    let output = sync(&home, &url, &["--use-login"], None);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Turns inserted: 0, updated: 0, unchanged: 2"),
        "{stdout}"
    );
    let compact: String = requests
        .lock()
        .expect("lock")
        .join("")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    // The newest event is at 1790000100 s; an hour before it is 1789996500 s.
    assert!(compact.contains(r#""startDate":"1789996500000""#));
    assert_no_credential(&home, &output);
}

#[test]
fn an_expired_login_fails_and_writes_nothing() {
    let home = home_with_login();
    let (url, _) = serve(vec![(401, "{}".to_string())]);
    let output = sync(&home, &url, &["--use-login"], None);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Cursor login expired: sign in to the Cursor app again"),
        "{stderr}"
    );
    assert_no_credential(&home, &output);
}

#[test]
fn an_admin_key_is_used_first_and_never_written() {
    let home = home_with_login();
    let (url, requests) = serve(vec![(
        200,
        r#"{"usageEvents":[{"timestamp":"1790000000000","model":"m","tokenUsage":{"inputTokens":1,"totalCents":1}}],"pagination":{"hasNextPage":false}}"#
            .to_string(),
    )]);
    let output = sync(
        &home,
        &url,
        &["--use-login", "--dry-run"],
        Some(FAKE_ADMIN_KEY),
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Source: Cursor Admin API"), "{stdout}");
    assert!(stdout.contains("dry run: nothing written"), "{stdout}");
    let seen = requests.lock().expect("lock").join("\n");
    assert!(contains(seen.as_bytes(), "authorization: Basic "));
    assert!(!contains(seen.as_bytes(), "WorkosCursorSessionToken"));
    assert_no_credential(&home, &output);
}
