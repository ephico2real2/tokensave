use std::fs;
use std::process::Command;

use tempfile::TempDir;

fn tokensave_command(home: &TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tokensave"));
    command
        .env("HOME", home.path())
        .env("TOKENSAVE_HOME", home.path().join(".tokensave"))
        .env("TOKENSAVE_SKIP_UPDATE_CHECK", "1")
        // Codex's own data folder override would point outside `home`.
        .env_remove("CODEX_HOME");
    command
}

fn isolated_home_with_claude_session() -> TempDir {
    let home = TempDir::new().expect("temp home");
    let tokensave_dir = home.path().join(".tokensave");
    fs::create_dir_all(&tokensave_dir).expect("tokensave dir");
    fs::write(
        tokensave_dir.join("config.toml"),
        "last_pricing_fetch_at = 4102444800\n",
    )
    .expect("config");

    let session_dir = home.path().join(".claude").join("projects").join("project");
    fs::create_dir_all(&session_dir).expect("claude session dir");
    fs::write(
        session_dir.join("session.jsonl"),
        "{\"type\":\"assistant\",\"message\":{\"id\":\"msg-1\",\"model\":\"claude-opus-4-6\",\"role\":\"assistant\",\"usage\":{\"input_tokens\":1000,\"output_tokens\":200,\"cache_creation_input_tokens\":500,\"cache_read_input_tokens\":800},\"content\":[]},\"timestamp\":\"2026-07-31T12:00:00.000Z\"}\n",
    )
    .expect("claude session");
    home
}

#[test]
fn cost_without_droid_preserves_existing_output() {
    let home = isolated_home_with_claude_session();
    let output = tokensave_command(&home)
        .args(["cost", "all"])
        .output()
        .expect("run tokensave cost");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("1.0k"), "{stdout}");
    assert!(!stdout.contains("Coverage:"), "{stdout}");
    assert!(!stdout.contains("Droid"), "{stdout}");
    assert!(!stdout.contains("Augment"), "{stdout}");
    assert!(!stdout.contains("Copilot"), "{stdout}");

    let json = tokensave_command(&home)
        .args(["cost", "all", "--export", "json"])
        .output()
        .expect("run tokensave cost json");
    assert!(
        json.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&json.stdout).expect("valid json output");
    let object = value.as_object().expect("json object");
    assert_eq!(value["total_input_tokens"], 1000);
    assert!(!object.contains_key("by_agent"), "{value}");
    assert!(!object.contains_key("coverage"), "{value}");
}

#[test]
fn removed_droid_source_does_not_surface_stale_rows() {
    let home = isolated_home_with_claude_session();
    let droid_dir = home
        .path()
        .join(".factory")
        .join("sessions")
        .join("project");
    fs::create_dir_all(&droid_dir).expect("droid session dir");
    fs::write(
        droid_dir.join("session.settings.json"),
        r#"{
            "providerLockTimestamp": "2026-07-31T12:00:00.000Z",
            "model": "claude-opus-4",
            "tokenUsage": {
                "inputTokens": 2000,
                "outputTokens": 400,
                "factoryCredits": 1500
            }
        }"#,
    )
    .expect("droid session");

    let first = tokensave_command(&home)
        .args(["cost", "all", "--by-agent"])
        .output()
        .expect("ingest Droid source");
    assert!(first.status.success());
    assert!(
        String::from_utf8_lossy(&first.stdout).contains("droid"),
        "{}",
        String::from_utf8_lossy(&first.stdout)
    );

    fs::remove_dir_all(home.path().join(".factory")).expect("remove Droid source");

    let second = tokensave_command(&home)
        .args(["cost", "all", "--by-agent"])
        .output()
        .expect("query after Droid removal");
    assert!(second.status.success());
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(!stdout.contains("droid"), "{stdout}");
    assert!(!stdout.contains("Coverage:"), "{stdout}");
}

#[test]
fn codex_turns_count_toward_the_cost_and_their_agent() {
    let home = isolated_home_with_claude_session();
    let sessions = home.path().join(".codex").join("sessions").join("2026");
    fs::create_dir_all(&sessions).expect("codex session dir");
    fs::write(
        sessions.join("rollout-a.jsonl"),
        concat!(
            r#"{"timestamp":"2026-07-31T12:00:00.000Z","type":"session_meta","payload":{"id":"T1","session_id":"T1","cwd":"/work/r","source":"exec"}}"#,
            "\n",
            r#"{"timestamp":"2026-07-31T12:00:01.000Z","type":"turn_context","payload":{"cwd":"/work/r","model":"claude-opus-4-6","effort":"high"}}"#,
            "\n",
            r#"{"timestamp":"2026-07-31T12:00:02.000Z","type":"token_usage_record","payload":{"response_id":"r1","session_id":"T1","thread_id":"T1","usage":{"input_tokens":3000,"cached_input_tokens":1000,"cache_write_input_tokens":0,"output_tokens":300,"reasoning_output_tokens":100,"total_tokens":3300}}}"#,
            "\n",
        ),
    )
    .expect("codex session");

    let output = tokensave_command(&home)
        .args(["cost", "all", "--by-agent", "--export", "csv"])
        .output()
        .expect("run tokensave cost csv");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let codex = stdout
        .lines()
        .find(|line| line.starts_with("codex,"))
        .unwrap_or_else(|| panic!("no codex row: {stdout}"));
    // agent,cost_usd,credits,input,output,cache_write,cache_read,turns,unpriced
    let cells: Vec<&str> = codex.split(',').collect();
    assert_eq!(
        cells[3..],
        ["2000", "300", "0", "1000", "1", "0"],
        "{codex}"
    );
    assert!(
        cells[1].parse::<f64>().is_ok_and(|cost| cost > 0.0),
        "{codex}"
    );

    let text = tokensave_command(&home)
        .args(["cost", "all"])
        .output()
        .expect("run tokensave cost");
    let stdout = String::from_utf8(text.stdout).expect("utf8 stdout");
    assert!(stdout.contains("Codex complete"), "{stdout}");
}
