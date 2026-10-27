//! Codex CLI session accounting parser.
//!
//! Reads `$CODEX_HOME/sessions/**/rollout-*.jsonl` (`~/.codex` by default),
//! one file per thread, and turns each `token_usage_record` line into a row
//! of the `turns` table. A record is one model response: its `response_id`
//! is unique across sessions, and its `usage` holds that response's tokens
//! only, so no cumulative difference is taken (`token_count` events are not
//! read: their totals disagree with the records in some files).
//!
//! Codex counts cached input inside `input_tokens` and reasoning inside
//! `output_tokens`, so the stored input is the uncached part and the
//! reasoning is kept apart as `thinking_tokens`, already within the output.
//!
//! The model, effort and working directory are not on the record but on the
//! `turn_context` line before it, and the git branch on the file's first
//! `session_meta`. A resumed read therefore scans the lines before its
//! offset for those two kinds only, and processes everything after it; the
//! turns it stores are the ones a full read stores.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;
use walkdir::WalkDir;

use crate::accounting::parser::{parse_timestamp, CoverageState, SourceCoverage};
use crate::accounting::{classifier, pricing};
use crate::global_db::GlobalDb;
use crate::types::{AgentRun, CostTurn, TurnDetail};

/// What one ingestion run of the Codex sessions directory produced.
#[derive(Debug)]
pub(crate) struct CodexIngestResult {
    pub coverage: SourceCoverage,
    pub turns_inserted: u64,
    /// Cost of the inserted turns whose model has a price.
    pub cost_usd: f64,
    /// Inserted turns whose model has no price (inkcast #252).
    pub unpriced_turns: u64,
    pub tokens_consumed: u64,
    /// Complete lines that are not JSON; skipped, and passed by the offset.
    pub lines_unparseable: u64,
    /// Compressed session files (`.jsonl.zst`), which are not read.
    pub compressed_skipped: u64,
}

/// The facts a record takes from the lines before it.
#[derive(Debug, Default, Clone, PartialEq)]
struct Context {
    /// Set by the first `session_meta` only: a forked thread's file carries
    /// the meta of the thread it was forked from after its own.
    meta_seen: bool,
    git_branch: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

/// The kind of a line, read from its top-level `type`.
fn line_type(value: &Value) -> Option<&str> {
    value.get("type").and_then(Value::as_str)
}

/// Updates `context` from a `session_meta` or `turn_context` line, and
/// returns the subagent run a `session_meta` line describes.
fn apply_context(context: &mut Context, value: &Value) -> Option<AgentRun> {
    let payload = value.get("payload")?;
    let text = |pointer: &str| {
        payload
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    match line_type(value)? {
        "session_meta" if !context.meta_seen => {
            context.meta_seen = true;
            context.git_branch = text("/git/branch");
            if context.cwd.is_none() {
                context.cwd = text("/cwd");
            }
            agent_run_of(payload, &project_hash_of(context.cwd.as_deref()))
        }
        "turn_context" => {
            context.model = text("/model");
            context.effort = text("/effort");
            if let Some(cwd) = text("/cwd") {
                context.cwd = Some(cwd);
            }
            None
        }
        _ => None,
    }
}

/// The subagent run a thread's `session_meta` describes, or `None` for a
/// main thread. A subagent thread has its own `id` and the parent session's
/// `session_id`; `parent_thread_id` names the thread that spawned it, which
/// is a run of its own only when it is not the main thread.
fn agent_run_of(meta: &Value, project_hash: &str) -> Option<AgentRun> {
    let subagent = meta.pointer("/source/subagent")?;
    let run_id = meta.get("id")?.as_str()?.to_string();
    let session_id = meta.get("session_id")?.as_str()?.to_string();
    let spawn = subagent.get("thread_spawn");
    let role = spawn
        .and_then(|spawn| spawn.get("agent_role"))
        .and_then(Value::as_str)
        .or_else(|| subagent.get("other").and_then(Value::as_str))
        .unwrap_or("subagent")
        .to_string();
    let parent_run_id = meta
        .get("parent_thread_id")
        .and_then(Value::as_str)
        .filter(|parent| *parent != session_id)
        .map(str::to_string);
    Some(AgentRun {
        run_id,
        session_id,
        project_hash: project_hash.to_string(),
        role,
        description: None,
        spawn_depth: spawn
            .and_then(|spawn| spawn.get("depth"))
            .and_then(Value::as_u64),
        parent_run_id,
        tool_use_id: None,
        model: None,
        effort: None,
        finished: None,
        worktree_branch: None,
    })
}

/// The project bucket of a working directory: Claude Code's project folder
/// name for it (every character that is not ASCII alphanumeric becomes `-`),
/// so a repository's Codex and Claude Code turns share a bucket.
fn project_hash_of(cwd: Option<&str>) -> String {
    cwd.map_or_else(
        || "unknown".to_string(),
        |cwd| {
            cwd.chars()
                .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
                .collect()
        },
    )
}

/// The turn a `token_usage_record` line describes, under `context`.
fn parse_record(value: &Value, context: &Context) -> Option<CostTurn> {
    let payload = value.get("payload")?;
    let response_id = payload.get("response_id")?.as_str()?;
    let session_id = payload.get("session_id")?.as_str()?;
    let thread_id = payload.get("thread_id").and_then(Value::as_str);
    let usage = payload.get("usage")?;
    let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);

    let cached = count("cached_input_tokens");
    let input_tokens = count("input_tokens").saturating_sub(cached);
    let output_tokens = count("output_tokens");
    let cache_write_tokens = count("cache_write_input_tokens");
    let reasoning = usage.get("reasoning_output_tokens").and_then(Value::as_u64);

    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_timestamp)?;
    let model = context
        .model
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let cost_usd = pricing::cost_of_turn(
        &model,
        input_tokens,
        output_tokens,
        cache_write_tokens,
        cached,
    );

    Some(CostTurn {
        message_id: format!("codex:{response_id}"),
        project_hash: project_hash_of(context.cwd.as_deref()),
        session_id: session_id.to_string(),
        model,
        timestamp,
        input_tokens,
        output_tokens,
        cache_write_tokens,
        cache_read_tokens: cached,
        cost_usd,
        // The calls are on other lines of the response; they are not
        // attributed to it, and the turn counts as conversation.
        category: classifier::classify(&[], &[]).as_str().to_string(),
        tool_names: String::new(),
        agent: "codex".to_string(),
        credits: None,
        detail: TurnDetail {
            effort: context.effort.clone(),
            thinking_tokens: reasoning,
            git_branch: context.git_branch.clone(),
            // A subagent thread's records name the parent session.
            run_id: thread_id
                .filter(|thread| *thread != session_id)
                .map(str::to_string),
            ..TurnDetail::default()
        },
    })
}

/// Every session file below `root`, sorted, and whether the walk failed
/// anywhere. Symlinks are not followed.
fn find_session_files(root: &Path) -> (Vec<PathBuf>, bool) {
    if !root.is_dir() {
        return (Vec::new(), false);
    }
    let mut files = Vec::new();
    let mut had_errors = false;
    for entry in WalkDir::new(root).follow_links(false) {
        match entry {
            Err(_) => had_errors = true,
            Ok(entry) if entry.file_type().is_file() => files.push(entry.into_path()),
            Ok(_) => {}
        }
    }
    files.sort();
    (files, had_errors)
}

fn is_rollout(path: &Path, extension: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(extension))
}

/// Ingests every Codex session file below `root` (`$CODEX_HOME/sessions`).
///
/// Coverage is `Absent` when there is nothing to read, `Partial` when a walk
/// or read failed or a compressed file was left unread, and `Complete`
/// otherwise. A line that is not JSON is skipped and counted; a last line
/// with no newline is still being written, and is read whole next time.
pub(crate) async fn ingest_dir(gdb: &GlobalDb, root: &Path) -> CodexIngestResult {
    let (files, mut had_errors) = find_session_files(root);
    let mut sessions = 0u64;
    let mut turns_inserted = 0u64;
    let mut cost_usd = 0.0f64;
    let mut unpriced_turns = 0u64;
    let mut tokens_consumed = 0u64;
    let mut lines_unparseable = 0u64;
    let mut compressed_skipped = 0u64;

    for file_path in &files {
        if is_rollout(file_path, ".jsonl.zst") {
            compressed_skipped += 1;
            continue;
        }
        if !is_rollout(file_path, ".jsonl") {
            continue;
        }
        sessions += 1;
        let path_str = file_path.to_string_lossy().to_string();
        let Ok(metadata) = fs::metadata(file_path) else {
            had_errors = true;
            continue;
        };
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_secs());
        let (previous_offset, previous_mtime) =
            gdb.get_parse_offset(&path_str).await.unwrap_or((0, 0));

        // As for Claude Code: a file shorter than what was read, or older,
        // was replaced and is read from its start; one with nothing past the
        // offset is skipped, whatever its mtime's second (#256).
        let len = metadata.len();
        let seek_to = if len >= previous_offset
            && (mtime == previous_mtime || (previous_mtime > 0 && mtime > previous_mtime))
        {
            previous_offset
        } else {
            0
        };
        if seek_to == len && mtime == previous_mtime {
            continue;
        }

        let Ok(file) = fs::File::open(file_path) else {
            had_errors = true;
            continue;
        };
        let mut reader = BufReader::new(file);
        let mut context = Context::default();
        let mut offset = 0u64;
        let mut line = String::new();
        let mut read_failed = false;

        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Err(_) => {
                    had_errors = true;
                    read_failed = true;
                    break;
                }
                Ok(_) if !line.ends_with('\n') => break,
                Ok(bytes) => {
                    let already_read = offset < seek_to;
                    offset += bytes as u64;
                    let trimmed = line.trim();
                    // Before the offset only the context lines matter; the
                    // substring test spares parsing every other line.
                    if trimmed.is_empty()
                        || (already_read
                            && !trimmed.contains("\"session_meta\"")
                            && !trimmed.contains("\"turn_context\""))
                    {
                        continue;
                    }
                    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                        if !already_read {
                            lines_unparseable += 1;
                        }
                        continue;
                    };
                    if line_type(&value) == Some("token_usage_record") {
                        if already_read {
                            continue;
                        }
                        let Some(turn) = parse_record(&value, &context) else {
                            continue;
                        };
                        if gdb.insert_turn(&turn).await {
                            turns_inserted += 1;
                            match turn.cost_usd {
                                Some(cost) => cost_usd += cost,
                                None => unpriced_turns += 1,
                            }
                            tokens_consumed += turn.input_tokens + turn.output_tokens;
                        }
                    } else if let Some(run) = apply_context(&mut context, &value) {
                        if !already_read {
                            had_errors |= !gdb.upsert_agent_run(&run).await;
                        }
                    }
                }
            }
        }

        if !read_failed {
            gdb.set_parse_offset(&path_str, offset, mtime).await;
        }
    }

    let state = if had_errors || compressed_skipped > 0 {
        CoverageState::Partial
    } else if sessions == 0 {
        CoverageState::Absent
    } else {
        CoverageState::Complete
    };
    CodexIngestResult {
        coverage: SourceCoverage {
            agent: "codex",
            state,
            sessions,
        },
        turns_inserted,
        cost_usd,
        unpriced_turns,
        tokens_consumed,
        lines_unparseable,
        compressed_skipped,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A model the embedded price table prices, so the test does not depend
    /// on a fetched `pricing.json`.
    const PRICED: &str = "claude-opus-4-6";
    const UNPRICED: &str = "codex-unpriced-zzz-2099";

    async fn open_test_db(tmp: &TempDir) -> GlobalDb {
        let db_path = tmp.path().join(".tokensave").join("global.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        GlobalDb::open_at(&db_path).await.unwrap()
    }

    fn session_meta(id: &str, session_id: &str, source: &str, branch: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-10-20T10:00:00.000Z","ordinal":0,"type":"session_meta","payload":{{"id":"{id}","session_id":"{session_id}","cwd":"/work/repo.one","git":{{"branch":"{branch}","commit_hash":"c0ffee","repository_url":"https://example.invalid/repo.git"}},"cli_version":"0.155.1","model_provider":"openai","originator":"codex_exec","history_mode":"full","context_window":{{"window_id":"w1"}},"source":{source}}}}}"#
        )
    }

    fn turn_context(model: &str, effort: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-10-20T10:00:01.000Z","ordinal":1,"type":"turn_context","payload":{{"cwd":"/work/repo.one","model":"{model}","effort":"{effort}","approval_policy":"never","collaboration_mode":{{"mode":"default","settings":{{"model":"{model}","reasoning_effort":"{effort}"}}}}}}}}"#
        )
    }

    fn record(
        response_id: &str,
        session_id: &str,
        thread_id: &str,
        input: u64,
        cached: u64,
    ) -> String {
        let usage = format!(
            r#"{{"input_tokens":{input},"cached_input_tokens":{cached},"cache_write_input_tokens":0,"output_tokens":200,"reasoning_output_tokens":50,"total_tokens":{}}}"#,
            input + 200
        );
        format!(
            r#"{{"timestamp":"2026-10-20T10:00:05.000Z","ordinal":2,"type":"token_usage_record","payload":{{"response_id":"{response_id}","root_turn_id":"rt","session_id":"{session_id}","thread_id":"{thread_id}","turn_id":"t","usage":{usage},"turn_token_usage":{usage},"thread_token_usage":{usage}}}}}"#
        )
    }

    /// A `token_count` event, whose totals the parser must not read.
    const TOKEN_COUNT: &str = r#"{"timestamp":"2026-10-20T10:00:06.000Z","ordinal":3,"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":999999,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":999999,"reasoning_output_tokens":0,"total_tokens":1999998},"total_token_usage":{"input_tokens":999999,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":999999,"reasoning_output_tokens":0,"total_tokens":1999998},"model_context_window":272000}}}"#;

    fn write_session(root: &Path, name: &str, lines: &[String]) -> PathBuf {
        let dir = root.join("2026").join("10").join("20");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    fn main_session(response_ids: &[&str], model: &str) -> Vec<String> {
        let mut lines = vec![
            session_meta("T1", "T1", r#""exec""#, "main"),
            turn_context(model, "high"),
        ];
        lines.extend(
            response_ids
                .iter()
                .map(|id| record(id, "T1", "T1", 1000, 600)),
        );
        lines.push(TOKEN_COUNT.to_string());
        lines
    }

    /// Every column of every Codex turn, and every run, through a second
    /// connection.
    async fn stored(db_tmp: &TempDir) -> Vec<String> {
        let db_path = db_tmp.path().join(".tokensave").join("global.db");
        let db = libsql::Builder::new_local(&db_path).build().await.unwrap();
        let conn = db.connect().unwrap();
        let mut out = Vec::new();
        for sql in [
            "SELECT * FROM turns WHERE agent = 'codex' ORDER BY message_id",
            "SELECT * FROM agent_runs ORDER BY run_id",
        ] {
            let mut rows = conn.query(sql, ()).await.unwrap();
            while let Some(row) = rows.next().await.unwrap() {
                let columns: Vec<String> = (0..row.column_count())
                    .map(|i| format!("{:?}", row.get_value(i).unwrap()))
                    .collect();
                out.push(columns.join("|"));
            }
        }
        out
    }

    /// The stored cost of a turn, through a second connection.
    async fn stored_cost(db_tmp: &TempDir, message_id: &str) -> Option<f64> {
        let db_path = db_tmp.path().join(".tokensave").join("global.db");
        let db = libsql::Builder::new_local(&db_path).build().await.unwrap();
        let conn = db.connect().unwrap();
        let mut rows = conn
            .query(
                "SELECT cost_usd FROM turns WHERE message_id = ?1",
                libsql::params![message_id.to_string()],
            )
            .await
            .unwrap();
        rows.next()
            .await
            .unwrap()
            .unwrap()
            .get::<Option<f64>>(0)
            .unwrap()
    }

    #[test]
    fn a_working_directory_is_bucketed_as_claude_code_names_its_folder() {
        assert_eq!(
            project_hash_of(Some("/work/repo.one_x")),
            "-work-repo-one-x"
        );
        assert_eq!(project_hash_of(None), "unknown");
    }

    #[test]
    fn a_record_becomes_a_turn_under_the_context_before_it() {
        let mut context = Context::default();
        for line in [
            session_meta("T1", "T1", r#""exec""#, "main"),
            turn_context(PRICED, "xhigh"),
        ] {
            assert_eq!(
                apply_context(&mut context, &serde_json::from_str(&line).unwrap()),
                None
            );
        }
        let value = serde_json::from_str(&record("resp_1", "T1", "T1", 1000, 600)).unwrap();
        let turn = parse_record(&value, &context).unwrap();

        assert_eq!(turn.message_id, "codex:resp_1");
        assert_eq!(turn.agent, "codex");
        assert_eq!(turn.session_id, "T1");
        assert_eq!(turn.project_hash, "-work-repo-one");
        assert_eq!(turn.model, PRICED);
        // Codex counts cached input inside the input: only the rest is new.
        assert_eq!((turn.input_tokens, turn.cache_read_tokens), (400, 600));
        assert_eq!((turn.output_tokens, turn.cache_write_tokens), (200, 0));
        assert_eq!(turn.detail.thinking_tokens, Some(50));
        assert_eq!(turn.detail.effort.as_deref(), Some("xhigh"));
        assert_eq!(turn.detail.git_branch.as_deref(), Some("main"));
        assert_eq!(turn.detail.run_id, None);
        assert_eq!(
            turn.cost_usd,
            pricing::cost_of_turn(PRICED, 400, 200, 0, 600)
        );
        assert!(turn.cost_usd.is_some_and(|cost| cost > 0.0));
    }

    #[tokio::test]
    async fn every_record_is_stored_once_and_token_counts_are_not_read() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let root = TempDir::new().unwrap();
        write_session(
            root.path(),
            "rollout-a.jsonl",
            &main_session(&["r1", "r2"], PRICED),
        );

        let first = ingest_dir(&gdb, root.path()).await;
        let second = ingest_dir(&gdb, root.path()).await;

        assert_eq!(first.turns_inserted, 2);
        assert_eq!(first.tokens_consumed, 2 * (400 + 200));
        assert_eq!(first.coverage.state, CoverageState::Complete);
        assert_eq!(second.turns_inserted, 0);
        assert_eq!(stored(&db_tmp).await.len(), 2);
    }

    /// A model with no price is stored unpriced, not free, and priced once
    /// it has one (inkcast #252).
    #[tokio::test]
    async fn a_turn_whose_model_has_no_price_is_priced_once_it_has_one() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let root = TempDir::new().unwrap();
        write_session(
            root.path(),
            "rollout-a.jsonl",
            &main_session(&["r1"], UNPRICED),
        );

        let stats = ingest_dir(&gdb, root.path()).await;
        assert_eq!((stats.turns_inserted, stats.unpriced_turns), (1, 1));
        // Nothing is added for a turn whose cost is unknown.
        assert_eq!(stats.cost_usd.to_bits(), 0.0f64.to_bits());
        assert_eq!(stored_cost(&db_tmp, "codex:r1").await, None);

        let price = pricing::ModelPricing {
            input_per_mtok: 1.25,
            output_per_mtok: 10.0,
            cache_write_per_mtok: 0.0,
            cache_read_per_mtok: 0.125,
        };
        let priced = gdb
            .price_unpriced_turns(|model| (model == UNPRICED).then_some(price))
            .await;
        assert_eq!(priced, Some(1));
        assert_eq!(
            stored_cost(&db_tmp, "codex:r1").await,
            Some(price.cost(400, 200, 0, 600))
        );
    }

    #[tokio::test]
    async fn a_subagent_thread_is_a_run_of_its_parent_session() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let root = TempDir::new().unwrap();
        let spawned = r#"{"subagent":{"thread_spawn":{"agent_nickname":"n","agent_path":"p","agent_role":null,"depth":2,"parent_thread_id":"T2"}}}"#;
        let mut lines = vec![
            r#"{"timestamp":"2026-10-20T10:00:00.000Z","ordinal":0,"type":"session_meta","payload":{"id":"T3","session_id":"T1","parent_thread_id":"T2","cwd":"/work/repo.one","source":"#.to_string()
                + spawned
                + "}}",
            turn_context(PRICED, "low"),
            record("r3", "T1", "T3", 10, 0),
        ];
        write_session(root.path(), "rollout-sub.jsonl", &lines);
        lines = vec![
            session_meta("T4", "T1", r#"{"subagent":{"other":"guardian"}}"#, "main")
                .replace(r#""cwd""#, r#""parent_thread_id":"T1","cwd""#),
            turn_context(PRICED, "low"),
            record("r4", "T1", "T4", 10, 0),
        ];
        write_session(root.path(), "rollout-guard.jsonl", &lines);

        ingest_dir(&gdb, root.path()).await;

        let spawned_run = gdb.agent_run("T3").await.unwrap();
        assert_eq!(spawned_run.session_id, "T1");
        assert_eq!(spawned_run.role, "subagent");
        assert_eq!(spawned_run.spawn_depth, Some(2));
        // Spawned by another subagent, which is a run of its own.
        assert_eq!(spawned_run.parent_run_id.as_deref(), Some("T2"));
        let guardian = gdb.agent_run("T4").await.unwrap();
        assert_eq!(guardian.role, "guardian");
        // Spawned by the main thread, which is the session, not a run.
        assert_eq!(guardian.parent_run_id, None);
        let rows = stored(&db_tmp).await;
        assert!(rows[0].starts_with(r#"Text("codex:r3")"#) && rows[0].contains(r#"Text("T3")"#));
        assert!(rows[1].starts_with(r#"Text("codex:r4")"#) && rows[1].contains(r#"Text("T4")"#));
    }

    /// A forked thread's file carries the meta of the thread it was forked
    /// from after its own; only the first is the thread's.
    #[tokio::test]
    async fn the_first_session_meta_is_the_threads() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let root = TempDir::new().unwrap();
        write_session(
            root.path(),
            "rollout-fork.jsonl",
            &[
                session_meta("T1", "T1", r#""exec""#, "fork-branch"),
                session_meta(
                    "T9",
                    "T8",
                    r#"{"subagent":{"other":"guardian"}}"#,
                    "source-branch",
                ),
                turn_context(PRICED, "high"),
                record("r1", "T1", "T1", 10, 0),
            ],
        );

        ingest_dir(&gdb, root.path()).await;

        assert_eq!(gdb.agent_run("T9").await, None);
        let rows = stored(&db_tmp).await;
        assert_eq!(rows.len(), 1);
        assert!(rows[0].contains(r#"Text("fork-branch")"#));
    }

    /// A read resumed at its offset stores exactly what one full read of the
    /// final file stores, the context before the offset included.
    #[tokio::test]
    async fn a_resumed_read_stores_what_a_full_read_stores() {
        let root = TempDir::new().unwrap();
        let path = write_session(
            root.path(),
            "rollout-a.jsonl",
            &main_session(&["r1"], PRICED),
        );
        let resumed_tmp = TempDir::new().unwrap();
        let resumed = open_test_db(&resumed_tmp).await;
        assert_eq!(ingest_dir(&resumed, root.path()).await.turns_inserted, 1);

        // After the offset: a turn under the context before it, then one
        // under a new model and effort.
        let mut lines = main_session(&["r1"], PRICED);
        lines.push(record("r2", "T1", "T1", 3000, 100));
        lines.push(turn_context(UNPRICED, "max"));
        lines.push(record("r3", "T1", "T1", 2000, 1500));
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        assert_eq!(ingest_dir(&resumed, root.path()).await.turns_inserted, 2);

        let full_tmp = TempDir::new().unwrap();
        let full = open_test_db(&full_tmp).await;
        assert_eq!(ingest_dir(&full, root.path()).await.turns_inserted, 3);

        let rows = stored(&resumed_tmp).await;
        assert_eq!(rows, stored(&full_tmp).await);
        let r2 = [
            PRICED,
            r#"Text("high")"#,
            r#"Text("main")"#,
            r#"Text("-work-repo-one")"#,
        ];
        assert!(
            r2.iter().all(|value| rows[1].contains(value)),
            "{}",
            rows[1]
        );
        assert!(rows[2].contains(UNPRICED) && rows[2].contains(r#"Text("max")"#));
    }

    /// A last line still being written is left for the next read, whole.
    #[tokio::test]
    async fn a_cut_off_last_line_is_read_whole_next_time() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let root = TempDir::new().unwrap();
        let mut lines = main_session(&["r1"], PRICED);
        let last = record("r2", "T1", "T1", 10, 0);
        let path = write_session(root.path(), "rollout-a.jsonl", &lines);
        let complete = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, complete.clone() + &last[..last.len() / 2]).unwrap();

        let first = ingest_dir(&gdb, root.path()).await;
        assert_eq!((first.turns_inserted, first.lines_unparseable), (1, 0));

        lines.push(last);
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let second = ingest_dir(&gdb, root.path()).await;
        assert_eq!((second.turns_inserted, second.lines_unparseable), (1, 0));
        assert_eq!(stored(&db_tmp).await.len(), 2);
    }

    #[tokio::test]
    async fn a_line_that_is_not_json_is_skipped_and_counted() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let root = TempDir::new().unwrap();
        let mut lines = main_session(&["r1"], PRICED);
        lines.insert(2, r#"{"type":"token_usage_record","payload":{"#.to_string());
        write_session(root.path(), "rollout-a.jsonl", &lines);

        let stats = ingest_dir(&gdb, root.path()).await;

        assert_eq!((stats.turns_inserted, stats.lines_unparseable), (1, 1));
        assert_eq!(stats.coverage.state, CoverageState::Complete);
        assert_eq!(ingest_dir(&gdb, root.path()).await.lines_unparseable, 0);
    }

    #[tokio::test]
    async fn a_compressed_session_is_left_unread_and_coverage_is_partial() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let root = TempDir::new().unwrap();
        write_session(
            root.path(),
            "rollout-a.jsonl",
            &main_session(&["r1"], PRICED),
        );
        write_session(
            root.path(),
            "rollout-b.jsonl.zst",
            &["not read".to_string()],
        );
        write_session(root.path(), "notes.jsonl", &main_session(&["r9"], PRICED));

        let stats = ingest_dir(&gdb, root.path()).await;

        assert_eq!(stats.compressed_skipped, 1);
        assert_eq!((stats.turns_inserted, stats.coverage.sessions), (1, 1));
        assert_eq!(stats.coverage.state, CoverageState::Partial);
    }

    #[tokio::test]
    async fn no_sessions_folder_is_absent() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let root = TempDir::new().unwrap();

        let stats = ingest_dir(&gdb, &root.path().join("sessions")).await;

        assert_eq!(stats.coverage.state, CoverageState::Absent);
        assert_eq!(stats.turns_inserted, 0);
    }
}
