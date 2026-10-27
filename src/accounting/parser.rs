//! JSONL session parser for Claude Code transcripts.
//!
//! Reads `~/.claude/projects/**/*.jsonl`, extracts assistant turns with
//! model/usage/tool data, and inserts them into the `turns` table via
//! `GlobalDb`. Uses offset tracking for incremental re-parsing.

use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::accounting::classifier::{self, BashSignals};
use crate::accounting::pricing;
use crate::accounting::turn_merge::{ClaudeLine, ToolCall};
use crate::context::read_modes::estimate_tokens;
use crate::global_db::GlobalDb;
use crate::types::{AgentRun, CostTurn, TurnDetail};

/// Whether a data source contributes complete, partial, or no coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CoverageState {
    Complete,
    Partial,
    Absent,
}

/// Coverage summary for one accounting data source.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceCoverage {
    pub agent: &'static str,
    pub state: CoverageState,
    pub sessions: u64,
}

/// Find all JSONL session files under `<home>/.claude/projects/`, sorted.
///
/// The boolean indicates whether discovery encountered an I/O error.
pub(crate) fn find_session_files(home: &Path) -> (Vec<PathBuf>, bool) {
    let projects_dir = home.join(".claude").join("projects");
    match fs::metadata(&projects_dir) {
        Ok(metadata) if metadata.is_dir() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), false);
        }
        Ok(_) | Err(_) => return (Vec::new(), true),
    }

    let mut files = Vec::new();
    let had_errors = collect_jsonl_files(&projects_dir, &mut files, 0);
    files.sort();
    (files, had_errors)
}

/// Recursively collect .jsonl files, with a depth limit to avoid runaway traversal.
fn collect_jsonl_files(dir: &Path, out: &mut Vec<PathBuf>, depth: u8) -> bool {
    if depth > 5 {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return true;
    };
    let mut had_errors = false;
    for entry in entries {
        let Ok(entry) = entry else {
            had_errors = true;
            continue;
        };
        let path = entry.path();
        if path.is_dir() {
            had_errors |= collect_jsonl_files(&path, out, depth + 1);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    had_errors
}

/// Extract project hash and session ID from a JSONL file path.
/// Path pattern: `~/.claude/projects/<project-hash>/<session-id>.jsonl`
/// or `~/.claude/projects/<project-hash>/<session-id>/subagents/<agent>.jsonl`
fn extract_path_parts(path: &Path) -> (String, String) {
    let components: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    // Find "projects" in the path and take the next component as project_hash
    let projects_idx = components.iter().position(|c| *c == "projects");
    let project_hash = projects_idx
        .and_then(|i| components.get(i + 1))
        .unwrap_or(&"unknown")
        .to_string();

    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    (project_hash, session_id)
}

/// Parse a single JSONL line into a [`ClaudeLine`], if it's an assistant
/// message with usage data.
fn parse_line(line: &str, project_hash: &str, session_id: &str) -> Option<ClaudeLine> {
    let v: Value = serde_json::from_str(line).ok()?;

    // Only process assistant messages
    if v.get("type")?.as_str()? != "assistant" {
        return None;
    }

    let msg = v.get("message")?;
    let message_id = msg.get("id")?.as_str()?;
    let model = msg.get("model")?.as_str()?;

    let usage = msg.get("usage")?;
    let input_tokens = usage.get("input_tokens")?.as_u64().unwrap_or(0);
    let output_tokens = usage.get("output_tokens")?.as_u64().unwrap_or(0);
    let cache_write_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let cache_read_tokens = usage
        .get("cache_read_input_tokens")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    // Parse timestamp from the outer object (ISO 8601)
    let timestamp = parse_timestamp(v.get("timestamp")?.as_str()?)?;

    // Extract tool names and bash commands for classification
    let content = msg.get("content").and_then(|c| c.as_array());
    let mut tool_names_vec: Vec<String> = Vec::new();
    let mut bash_commands: Vec<String> = Vec::new();
    let mut tool_use_ids: Vec<String> = Vec::new();
    let mut calls: Vec<ToolCall> = Vec::new();

    if let Some(blocks) = content {
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                let id = block.get("id").and_then(|i| i.as_str());
                if let Some(id) = id {
                    tool_use_ids.push(id.to_string());
                }
                if let Some(name) = block.get("name").and_then(|n| n.as_str()) {
                    tool_names_vec.push(name.to_string());
                    // A later line is merged by call id, and the ids are
                    // stored comma-joined: an empty id, or one holding a
                    // comma, is never found again, so its call would be
                    // appended on every read of the line.
                    if let Some(id) = id.filter(|id| !id.is_empty() && !id.contains(',')) {
                        calls.push(ToolCall {
                            id: id.to_string(),
                            name: name.to_string(),
                        });
                    }
                    if name == "Bash" {
                        if let Some(cmd) = block
                            .get("input")
                            .and_then(|i| i.get("command"))
                            .and_then(|c| c.as_str())
                        {
                            bash_commands.push(cmd.to_string());
                        }
                    }
                }
            }
        }
    }

    // Classify
    let tool_refs: Vec<&str> = tool_names_vec
        .iter()
        .map(std::string::String::as_str)
        .collect();
    let bash_refs: Vec<&str> = bash_commands
        .iter()
        .map(std::string::String::as_str)
        .collect();
    let bash = BashSignals::of(&bash_refs);
    let category = classifier::classify_with(&tool_refs, bash);

    // Compute cost
    let cost_usd = pricing::cost_of_turn(
        model,
        input_tokens,
        output_tokens,
        cache_write_tokens,
        cache_read_tokens,
    );

    let turn = CostTurn {
        message_id: message_id.to_string(),
        project_hash: project_hash.to_string(),
        session_id: session_id.to_string(),
        model: model.to_string(),
        timestamp,
        input_tokens,
        output_tokens,
        cache_write_tokens,
        cache_read_tokens,
        cost_usd,
        category: category.as_str().to_string(),
        tool_names: tool_names_vec.join(","),
        agent: "claude".to_string(),
        credits: None,
        detail: turn_detail(&v, usage),
    };
    Some(ClaudeLine {
        turn,
        calls,
        bash,
        tool_use_ids,
    })
}

/// The [`TurnDetail`] of one assistant line: the outer object `v` and its
/// `message.usage`. A key that is absent or `null` leaves its field `None`.
fn turn_detail(v: &Value, usage: &Value) -> TurnDetail {
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_string);
    TurnDetail {
        // Equal whenever both are set; either one may be null on its own.
        effort: text(v.get("perTurnEffort")).or_else(|| text(v.get("effort"))),
        requested_model: text(v.get("requestedModel")),
        thinking_tokens: usage
            .get("output_tokens_details")
            .and_then(|d| d.get("thinking_tokens"))
            .and_then(Value::as_u64),
        // Per thinking block, and the message's first line wins; later blocks
        // of a message measured at most 22 ms (0.002% of all thinking time).
        thinking_ms: v.get("thinkingDurationMs").and_then(Value::as_u64),
        speed: text(usage.get("speed")),
        service_tier: text(usage.get("service_tier")),
        git_branch: text(v.get("gitBranch")),
        run_id: text(v.get("agentId")),
    }
}

/// Longest `description` kept for a subagent run: a task label, not a prompt.
const RUN_DESCRIPTION_MAX_CHARS: usize = 200;

/// The [`AgentRun`] a subagent's `.meta.json` describes, or `None` when the
/// file is unreadable or has no `agentType`.
///
/// `transcript` is the run's `.jsonl` beside it, at
/// `projects/<project>/<session>/subagents/agent-<id>.jsonl`: the run id comes
/// from its file name and the parent session from the folder above
/// `subagents`.
fn parse_agent_run(meta: &str, transcript: &Path, project_hash: &str) -> Option<AgentRun> {
    let v: Value = serde_json::from_str(meta).ok()?;
    let text = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    let run_id = transcript
        .file_stem()?
        .to_str()?
        .strip_prefix("agent-")?
        .to_string();
    let subagents = transcript.parent()?;
    if subagents.file_name()? != "subagents" {
        return None;
    }
    let session_id = subagents.parent()?.file_name()?.to_str()?.to_string();
    Some(AgentRun {
        run_id,
        session_id,
        project_hash: project_hash.to_string(),
        role: text("agentType")?,
        description: text("description")
            .map(|d| d.chars().take(RUN_DESCRIPTION_MAX_CHARS).collect()),
        spawn_depth: v.get("spawnDepth").and_then(Value::as_u64),
        parent_run_id: text("parentAgentId"),
        tool_use_id: text("toolUseId"),
        model: text("model"),
        effort: text("effort"),
        finished: v.get("finished").and_then(Value::as_bool),
        worktree_branch: text("worktreeBranch"),
    })
}

/// Records the run behind a subagent transcript from its `.meta.json`, when
/// that file changed since the last ingest (`finished` is written late).
/// Returns `true` when reading or recording it failed, for the source's
/// coverage; the next ingest then tries again.
async fn ingest_agent_run(gdb: &GlobalDb, transcript: &Path, project_hash: &str) -> bool {
    let meta_path = transcript.with_extension("meta.json");
    let Ok(meta) = fs::metadata(&meta_path) else {
        // Older Claude Code builds wrote no `.meta.json`: nothing to record.
        return false;
    };
    // Length and mtime in nanoseconds: the rewrite that sets `finished` can
    // land in the same second as an earlier read, and whole seconds would
    // hide it for good.
    let len = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64);
    let key = meta_path.to_string_lossy().to_string();
    if gdb.get_parse_offset(&key).await == Some((len, mtime)) {
        return false;
    }
    let Ok(text) = fs::read_to_string(&meta_path) else {
        return true;
    };
    if let Some(run) = parse_agent_run(&text, transcript, project_hash) {
        if !gdb.upsert_agent_run(&run).await {
            return true;
        }
    }
    gdb.set_parse_offset(&key, len, mtime).await;
    false
}

/// The tool results carried by one transcript line, as
/// `(tool_use_id, estimated tokens)` pairs.
///
/// A result does not travel with the `tool_use` that asked for it: it arrives
/// in the *following* user message, so the caller keeps the id-to-turn mapping
/// and attributes the size back (#474). Only user messages carry results;
/// every other line yields nothing.
///
/// The size is measured on the result text — the bytes a `Read` or a `Grep`
/// actually put into the conversation, which is the quantity a graph query
/// could have served more cheaply. Content arrives either as a plain string or
/// as a list of blocks, and both forms are measured.
fn parse_tool_results(line: &str) -> Vec<(String, u64)> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    if v.get("type").and_then(|t| t.as_str()) != Some("user") {
        return Vec::new();
    }
    let Some(blocks) = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for block in blocks {
        if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
            continue;
        }
        let Some(id) = block.get("tool_use_id").and_then(|i| i.as_str()) else {
            continue;
        };
        let tokens = block.get("content").map_or(0, measure_result_content);
        if tokens > 0 {
            out.push((id.to_string(), tokens));
        }
    }
    out
}

/// Estimated tokens of one `tool_result` block's content.
fn measure_result_content(content: &Value) -> u64 {
    match content {
        Value::String(text) => u64::from(estimate_tokens(text)),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| {
                block
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map_or(0, |text| u64::from(estimate_tokens(text)))
            })
            .sum(),
        _ => 0,
    }
}

/// Parse an ISO 8601 timestamp to unix epoch seconds.
pub(crate) fn parse_timestamp(ts: &str) -> Option<u64> {
    // Handle "2026-04-14T10:32:15.039Z" format
    // Simple parsing without pulling in chrono: split on known positions
    if ts.len() < 19 {
        return None;
    }
    let year: i64 = ts.get(0..4)?.parse().ok()?;
    let month: u64 = ts.get(5..7)?.parse().ok()?;
    let day: u64 = ts.get(8..10)?.parse().ok()?;
    let hour: u64 = ts.get(11..13)?.parse().ok()?;
    let min: u64 = ts.get(14..16)?.parse().ok()?;
    let sec: u64 = ts.get(17..19)?.parse().ok()?;

    let bytes = ts.as_bytes();
    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
        || year < 1970
        || !(1..=12).contains(&month)
        || hour >= 24
        || min >= 60
        || sec >= 60
    {
        return None;
    }

    let month_days = [0u64, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let max_day = month_days[month as usize] + u64::from(month == 2 && is_leap(year));
    if day == 0 || day > max_day {
        return None;
    }

    // Days from epoch using a simple formula (good enough for 2000-2100)
    let mut days: i64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += month_days[m as usize] as i64;
    }
    if month > 2 && is_leap(year) {
        days += 1;
    }
    days += (day as i64) - 1;

    Some((days as u64) * 86400 + hour * 3600 + min * 60 + sec)
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

/// Stats returned by the `ingest` function.
pub struct IngestStats {
    /// Number of new Claude turns inserted (receipt-only; does not count historical Droid scans).
    pub turns_inserted: u64,
    /// Number of Droid turns inserted or updated.
    pub turns_updated: u64,
    /// Total cost of newly-inserted Claude turns.
    pub cost_usd: f64,
    /// Total input + output tokens of newly-inserted Claude turns.
    pub tokens_consumed: u64,
    /// Per-source coverage information.
    pub coverage: Vec<SourceCoverage>,
}

impl IngestStats {
    /// Returns an empty stats value with no coverage information.
    pub fn empty() -> Self {
        Self {
            turns_inserted: 0,
            turns_updated: 0,
            cost_usd: 0.0,
            tokens_consumed: 0,
            coverage: Vec::new(),
        }
    }
}

/// Ingest only Claude Code session files from `home`, skipping any Droid scan.
///
/// Shared by the Claude Stop hook (via [`ingest_claude_only`]) and the full
/// coordinated ingest (via [`ingest_from_home`]).  Keeping this as a private
/// helper avoids duplicating the offset-tracking loop.
async fn ingest_claude_from_home(gdb: &GlobalDb, home: &Path) -> IngestStats {
    if gdb.newer_schema().is_some() {
        return IngestStats::empty();
    }
    let (files, mut had_errors) = find_session_files(home);
    let claude_sessions = files.len() as u64;

    let mut total_inserted = 0u64;
    let mut total_cost = 0.0f64;
    let mut total_tokens = 0u64;

    for file_path in &files {
        let path_str = file_path.to_string_lossy().to_string();

        // Before the unchanged-transcript skip below: a run's `.meta.json`
        // can change (`finished`) after its transcript's last line.
        if file_path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "subagents")
        {
            let (project_hash, _) = extract_path_parts(file_path);
            had_errors |= ingest_agent_run(gdb, file_path, &project_hash).await;
        }

        let Ok(meta) = fs::metadata(file_path) else {
            had_errors = true;
            continue;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());

        let (prev_offset, prev_mtime) = gdb.get_parse_offset(&path_str).await.unwrap_or((0, 0));

        // A file shorter than what was read, or older, was replaced: read it
        // from its start.
        let len = meta.len();
        let seek_to = if len >= prev_offset
            && (mtime == prev_mtime || (prev_mtime > 0 && mtime > prev_mtime))
        {
            prev_offset
        } else {
            0
        };

        // Skipped only with nothing past the offset: the mtime is in whole
        // seconds, and lines appended in the second of the last read leave it
        // unchanged (#256).
        if seek_to == len && mtime == prev_mtime {
            continue;
        }

        let (project_hash, session_id) = extract_path_parts(file_path);

        let Ok(f) = fs::File::open(file_path) else {
            had_errors = true;
            continue;
        };
        let mut reader = BufReader::new(f);

        if seek_to > 0 && reader.seek(SeekFrom::Start(seek_to)).is_err() {
            had_errors = true;
            continue;
        }

        // `tool_use_id` -> the message that issued it, so a result found in a
        // later line can be attributed back (#474). Per session file, and
        // entries are removed as they are consumed, so it holds only the calls
        // still awaiting a result.
        let mut issuing_turn: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        let mut line = String::new();
        let mut current_offset = seek_to;
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
                // A last line with no newline is still being written; the
                // offset stays before it, and the next ingest reads it whole.
                Ok(_) if !line.ends_with('\n') => break,
                Ok(n) => {
                    current_offset += n as u64;
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Some(line) = parse_line(trimmed, &project_hash, &session_id) {
                        let turn = &line.turn;
                        // Remember which turn issued each tool call, so the
                        // result blocks in a later user message can be sized
                        // and added back to it (#474).
                        for id in &line.tool_use_ids {
                            issuing_turn.insert(id.clone(), turn.message_id.clone());
                        }
                        if gdb.insert_claude_turn(&line).await {
                            total_inserted += 1;
                            total_cost += turn.cost_usd;
                            total_tokens += turn.input_tokens + turn.output_tokens;
                        } else {
                            // A later line of a message already recorded:
                            // its calls and final usage join the turn (#250).
                            gdb.merge_claude_line(&line).await;
                        }
                    } else {
                        for (tool_use_id, tokens) in parse_tool_results(trimmed) {
                            // An id with no remembered turn is what a resumed
                            // parse sees when the assistant line fell before
                            // the stored offset; there is nothing to add it to.
                            if let Some(message_id) = issuing_turn.remove(&tool_use_id) {
                                gdb.add_tool_result_tokens(&message_id, &tool_use_id, tokens)
                                    .await;
                            }
                        }
                    }
                }
            }
        }

        if !read_failed {
            gdb.set_parse_offset(&path_str, current_offset, mtime).await;
        }
    }

    let claude_state = if had_errors {
        CoverageState::Partial
    } else if files.is_empty() {
        CoverageState::Absent
    } else {
        CoverageState::Complete
    };

    IngestStats {
        turns_inserted: total_inserted,
        turns_updated: 0,
        cost_usd: total_cost,
        tokens_consumed: total_tokens,
        coverage: vec![SourceCoverage {
            agent: "claude",
            state: claude_state,
            sessions: claude_sessions,
        }],
    }
}

/// Ingest Claude sessions from `home` and Droid sessions from
/// `home/.factory/sessions`.  Returns combined [`IngestStats`].
pub(crate) async fn ingest_from_home(gdb: &GlobalDb, home: &Path) -> IngestStats {
    // A database newer than this binary is read only (inkcast #248).
    if gdb.newer_schema().is_some() {
        return IngestStats::empty();
    }
    let mut stats = ingest_claude_from_home(gdb, home).await;

    // Droid ingestion from ~/.factory/sessions
    let droid_root = home.join(".factory").join("sessions");
    let droid = crate::accounting::droid::ingest_dir(gdb, &droid_root).await;

    stats.turns_updated = droid.rows_changed;
    stats.coverage.push(droid.coverage);

    let pi = ingest_pi_from_home(gdb, home).await;
    stats.turns_inserted += pi.turns_inserted;
    stats.cost_usd += pi.cost_usd;
    stats.tokens_consumed += pi.tokens_consumed;
    stats.coverage.extend(pi.coverage);

    stats
}

/// Ingest Pi sessions from `home/.pi/agent/sessions`.
///
/// Pi stores one JSONL file per session and prices every message itself, so
/// this source contributes its own cost rather than a computed one.
async fn ingest_pi_from_home(gdb: &GlobalDb, home: &Path) -> IngestStats {
    if gdb.newer_schema().is_some() {
        return IngestStats::empty();
    }
    let sessions = home.join(".pi").join("agent").join("sessions");
    let pi = crate::accounting::pi::ingest_dir(gdb, &sessions).await;
    IngestStats {
        turns_inserted: pi.turns_inserted,
        turns_updated: 0,
        cost_usd: pi.cost_usd,
        tokens_consumed: pi.tokens_consumed,
        coverage: vec![pi.coverage],
    }
}

/// Ingest only Claude Code session files, skipping any Droid scan.
///
/// Use this from the Claude Stop hook to keep the hot path free of Droid
/// filesystem and JSON I/O.  The general [`ingest`] function performs the full
/// Claude + Droid scan and is reserved for cost/monitor/status callers.
pub async fn ingest_claude_only(gdb: &GlobalDb) -> IngestStats {
    let Some(home) = crate::agents::home_dir() else {
        return IngestStats::empty();
    };
    ingest_claude_from_home(gdb, &home).await
}

/// Ingest only Pi session files, skipping the Claude and Droid scans.
///
/// Use this from the Pi `agent_settled` hook, which runs once per session end
/// and must not pay for the other sources' filesystem work.
pub async fn ingest_pi_only(gdb: &GlobalDb) -> IngestStats {
    let Some(home) = crate::agents::home_dir() else {
        return IngestStats::empty();
    };
    ingest_pi_from_home(gdb, &home).await
}

/// Ingest all session files from every supported agent into the global DB.
/// Uses offset tracking to only parse new lines since the last run.
pub async fn ingest(gdb: &GlobalDb) -> IngestStats {
    let Some(home) = crate::agents::home_dir() else {
        return IngestStats::empty();
    };
    ingest_from_home(gdb, &home).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn open_test_db(tmp: &TempDir) -> GlobalDb {
        let db_path = tmp.path().join(".tokensave").join("global.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        GlobalDb::open_at(&db_path).await.unwrap()
    }

    /// `ingest_claude_from_home` must not populate Droid coverage or `turns_updated`.
    #[tokio::test]
    async fn claude_only_ingest_has_no_droid_coverage() {
        let home_tmp = TempDir::new().unwrap();
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;

        let stats = ingest_claude_from_home(&gdb, home_tmp.path()).await;

        assert_eq!(
            stats.turns_updated, 0,
            "claude-only must not update Droid rows"
        );
        assert!(
            !stats.coverage.iter().any(|c| c.agent == "droid"),
            "claude-only must not include droid coverage"
        );
        assert_eq!(
            stats.coverage.len(),
            1,
            "exactly one coverage entry (claude)"
        );
        assert_eq!(stats.coverage[0].agent, "claude");
    }

    #[test]
    fn test_parse_timestamp() {
        // 2026-01-01T00:00:00Z
        let ts = parse_timestamp("2026-01-01T00:00:00.000Z");
        assert!(ts.is_some());
        let epoch = ts.unwrap();
        // 2026-01-01 = 56 years from 1970, roughly 20454 days
        assert!(epoch > 1_700_000_000);
        assert!(epoch < 1_800_000_000);
    }

    #[test]
    fn invalid_timestamp_components_return_none() {
        assert_eq!(parse_timestamp("2026-14-01T00:00:00Z"), None);
        assert_eq!(parse_timestamp("2026-02-30T00:00:00Z"), None);
        assert_eq!(parse_timestamp("2026-01-01T24:00:00Z"), None);
        assert_eq!(parse_timestamp("2026-01-01T00:60:00Z"), None);
        assert_eq!(parse_timestamp("1969-12-31T23:59:59Z"), None);
        assert_eq!(parse_timestamp("2026/01/01 00:00:00Z"), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn claude_file_read_error_marks_coverage_partial() {
        let home_tmp = TempDir::new().unwrap();
        let db_tmp = TempDir::new().unwrap();
        let projects = home_tmp
            .path()
            .join(".claude")
            .join("projects")
            .join("project");
        std::fs::create_dir_all(&projects).unwrap();
        std::os::unix::fs::symlink(
            projects.join("missing-target"),
            projects.join("broken.jsonl"),
        )
        .unwrap();
        let gdb = open_test_db(&db_tmp).await;

        let stats = ingest_claude_from_home(&gdb, home_tmp.path()).await;

        assert_eq!(stats.coverage[0].state, CoverageState::Partial);
        assert_eq!(stats.coverage[0].sessions, 1);
    }

    #[test]
    fn test_parse_timestamp_invalid() {
        assert!(parse_timestamp("bad").is_none());
        assert!(parse_timestamp("").is_none());
    }

    #[test]
    fn test_extract_path_parts() {
        let path =
            PathBuf::from("/Users/test/.claude/projects/-Users-test-Code/abc123-session.jsonl");
        let (project, session) = extract_path_parts(&path);
        assert_eq!(project, "-Users-test-Code");
        assert_eq!(session, "abc123-session");
    }

    #[test]
    fn test_parse_line_assistant() {
        let line = r#"{"type":"assistant","message":{"id":"msg_01abc","model":"claude-opus-4-6","role":"assistant","usage":{"input_tokens":1000,"output_tokens":200,"cache_creation_input_tokens":500,"cache_read_input_tokens":800},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"test.rs"}}]},"timestamp":"2026-04-14T10:00:00.000Z"}"#;
        let turn = parse_line(line, "proj", "sess");
        assert!(turn.is_some());
        let t = turn.unwrap().turn;
        assert_eq!(t.message_id, "msg_01abc");
        assert_eq!(t.model, "claude-opus-4-6");
        assert_eq!(t.input_tokens, 1000);
        assert_eq!(t.output_tokens, 200);
        assert_eq!(t.cache_write_tokens, 500);
        assert_eq!(t.cache_read_tokens, 800);
        assert_eq!(t.category, "coding");
        assert_eq!(t.tool_names, "Edit");
        assert!(t.cost_usd > 0.0);
    }

    #[test]
    fn test_parse_line_user_skipped() {
        let line = r#"{"type":"user","message":{"content":"hello"},"timestamp":"2026-04-14T10:00:00.000Z"}"#;
        assert!(parse_line(line, "proj", "sess").is_none());
    }

    #[test]
    fn test_parse_line_malformed() {
        assert!(parse_line("not json at all", "proj", "sess").is_none());
        assert!(parse_line("{}", "proj", "sess").is_none());
    }

    /// The ids a turn issued come back with it, since the results that will
    /// be attributed to it are identified by nothing else (#474).
    #[test]
    fn assistant_turn_reports_the_tool_calls_it_issued() {
        let line = r#"{"type":"assistant","message":{"id":"msg_01abc","model":"claude-opus-4-6","role":"assistant","usage":{"input_tokens":3,"output_tokens":200},"content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{}},{"type":"tool_use","id":"toolu_2","name":"Grep","input":{}}]},"timestamp":"2026-04-14T10:00:00.000Z"}"#;
        let line = parse_line(line, "proj", "sess").unwrap();
        assert_eq!(line.turn.tool_names, "Read,Grep");
        assert_eq!(line.tool_use_ids, ["toolu_1", "toolu_2"]);
        assert_eq!(line.call_ids(), "toolu_1,toolu_2");
    }

    /// A result block is sized by its text, whether the content is a plain
    /// string or a list of blocks.
    #[test]
    fn tool_results_are_measured_by_their_text() {
        let string_form = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"0123456789012345678901234567890123456789"}]}}"#;
        let results = parse_tool_results(string_form);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "toolu_1");
        assert_eq!(results[0].1, 10, "40 characters at ~4 per token");

        let block_form = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_2","content":[{"type":"text","text":"01234567"},{"type":"text","text":"89AB"}]}]}}"#;
        let results = parse_tool_results(block_form);
        assert_eq!(results, [("toolu_2".to_string(), 3)]);
    }

    /// Everything that is not a tool result contributes nothing — an ordinary
    /// user message most of all, which is text the user typed rather than
    /// anything a tool injected.
    #[test]
    fn only_tool_results_are_measured() {
        let typed = r#"{"type":"user","message":{"content":"a long message the user typed out"}}"#;
        assert!(parse_tool_results(typed).is_empty());

        let assistant =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}"#;
        assert!(parse_tool_results(assistant).is_empty());

        assert!(parse_tool_results("not json").is_empty());
        assert!(parse_tool_results("{}").is_empty());
    }

    // Shapes measured on real Claude Code transcripts (2026-10-27): key names
    // and value kinds only, sample values invented.
    const MAIN_LINE: &str = r#"{"type":"assistant","sessionId":"sess-1","isSidechain":false,"gitBranch":"main","effort":"high","perTurnEffort":"high","requestedModel":"claude-opus-5-5[1m]","thinkingDurationMs":1234,"message":{"id":"msg_main","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":200,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"service_tier":"standard","speed":"standard","output_tokens_details":{"thinking_tokens":150}},"content":[]},"timestamp":"2026-10-27T04:00:00.000Z"}"#;
    const SUBAGENT_LINE: &str = r#"{"type":"assistant","sessionId":"sess-1","isSidechain":true,"agentId":"a1b2c3","gitBranch":"fix/thing","effort":"xhigh","perTurnEffort":null,"message":{"id":"msg_sub","model":"claude-opus-5-5","usage":{"input_tokens":3,"output_tokens":40,"service_tier":"standard","speed":null}},"timestamp":"2026-10-27T04:01:00.000Z"}"#;

    #[test]
    fn a_main_session_turn_records_who_ran_it_and_how() {
        let turn = parse_line(MAIN_LINE, "proj", "sess-1").unwrap().turn;
        assert_eq!(
            turn.detail,
            TurnDetail {
                effort: Some("high".into()),
                requested_model: Some("claude-opus-5-5[1m]".into()),
                thinking_tokens: Some(150),
                thinking_ms: Some(1234),
                speed: Some("standard".into()),
                service_tier: Some("standard".into()),
                git_branch: Some("main".into()),
                run_id: None,
            }
        );
    }

    /// `perTurnEffort` can be null while `effort` is set; a null `speed` is
    /// no speed, not the string "null".
    #[test]
    fn a_subagent_turn_carries_its_run_and_falls_back_to_effort() {
        let turn = parse_line(SUBAGENT_LINE, "proj", "agent-a1b2c3")
            .unwrap()
            .turn;
        assert_eq!(turn.detail.run_id.as_deref(), Some("a1b2c3"));
        assert_eq!(turn.detail.effort.as_deref(), Some("xhigh"));
        assert_eq!(turn.detail.speed, None);
        assert_eq!(turn.detail.thinking_tokens, None);
        assert_eq!(turn.detail.git_branch.as_deref(), Some("fix/thing"));
    }

    #[test]
    fn a_line_without_the_fields_leaves_them_unknown() {
        let line = r#"{"type":"assistant","message":{"id":"msg_old","model":"claude-opus-4-6","usage":{"input_tokens":1,"output_tokens":2}},"timestamp":"2026-04-14T10:00:00.000Z"}"#;
        let turn = parse_line(line, "proj", "sess").unwrap().turn;
        assert_eq!(turn.detail, TurnDetail::default());
    }

    fn transcript() -> PathBuf {
        PathBuf::from("/h/.claude/projects/-proj/sess-1/subagents/agent-a1b2c3.jsonl")
    }

    #[test]
    fn a_meta_file_describes_its_run() {
        let meta = r#"{"agentType":"ob3","description":"Review the change","toolUseId":"toolu_9","spawnDepth":1,"parentAgentId":"p0","model":"claude-opus-5-5","effort":"max","finished":true,"worktreeBranch":"review/9","requestShape":"x","isBuiltIn":false}"#;
        let run = parse_agent_run(meta, &transcript(), "-proj").unwrap();
        assert_eq!(
            run,
            AgentRun {
                run_id: "a1b2c3".into(),
                session_id: "sess-1".into(),
                project_hash: "-proj".into(),
                role: "ob3".into(),
                description: Some("Review the change".into()),
                spawn_depth: Some(1),
                parent_run_id: Some("p0".into()),
                tool_use_id: Some("toolu_9".into()),
                model: Some("claude-opus-5-5".into()),
                effort: Some("max".into()),
                finished: Some(true),
                worktree_branch: Some("review/9".into()),
            }
        );
    }

    /// Most meta files carry no model, effort or finished flag.
    #[test]
    fn a_sparse_meta_file_leaves_the_rest_unknown() {
        let meta = r#"{"agentType":"Explore","description":"Find it","toolUseId":"toolu_1","spawnDepth":1}"#;
        let run = parse_agent_run(meta, &transcript(), "-proj").unwrap();
        assert_eq!(run.role, "Explore");
        assert_eq!((run.model, run.effort, run.finished), (None, None, None));
    }

    #[test]
    fn a_meta_file_is_refused_without_a_role_or_outside_subagents() {
        assert!(parse_agent_run(r#"{"spawnDepth":1}"#, &transcript(), "-proj").is_none());
        let elsewhere = PathBuf::from("/h/.claude/projects/-proj/sess-1/agent-a1b2c3.jsonl");
        assert!(parse_agent_run(r#"{"agentType":"ob3"}"#, &elsewhere, "-proj").is_none());
        assert!(parse_agent_run("not json", &transcript(), "-proj").is_none());
    }

    #[test]
    fn a_long_description_is_cut_to_a_label() {
        let long = "é".repeat(RUN_DESCRIPTION_MAX_CHARS + 50);
        let meta = format!(r#"{{"agentType":"ob3","description":"{long}"}}"#);
        let run = parse_agent_run(&meta, &transcript(), "-proj").unwrap();
        assert_eq!(
            run.description.unwrap().chars().count(),
            RUN_DESCRIPTION_MAX_CHARS
        );
    }

    /// End to end: a session with one subagent is ingested into `turns` and
    /// `agent_runs`, and a later `finished` in the meta file reaches the run.
    #[tokio::test]
    async fn a_session_and_its_subagent_run_are_ingested() {
        let home = TempDir::new().unwrap();
        let db_tmp = TempDir::new().unwrap();
        let session = home
            .path()
            .join(".claude")
            .join("projects")
            .join("-proj")
            .join("sess-1");
        let subagents = session.join("subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        std::fs::write(session.with_extension("jsonl"), format!("{MAIN_LINE}\n")).unwrap();
        std::fs::write(
            subagents.join("agent-a1b2c3.jsonl"),
            format!("{SUBAGENT_LINE}\n"),
        )
        .unwrap();
        let meta_path = subagents.join("agent-a1b2c3.meta.json");
        std::fs::write(&meta_path, r#"{"agentType":"ob3","spawnDepth":1}"#).unwrap();
        let gdb = open_test_db(&db_tmp).await;

        let stats = ingest_claude_from_home(&gdb, home.path()).await;

        assert_eq!(stats.turns_inserted, 2);
        assert_eq!(stats.coverage[0].state, CoverageState::Complete);
        let main = gdb.turn_detail("msg_main").await.unwrap();
        assert_eq!(main.effort.as_deref(), Some("high"));
        assert_eq!(main.run_id, None);
        let sub = gdb.turn_detail("msg_sub").await.unwrap();
        assert_eq!(sub.run_id.as_deref(), Some("a1b2c3"));
        let run = gdb.agent_run("a1b2c3").await.unwrap();
        assert_eq!(
            (run.session_id.as_str(), run.role.as_str(), run.finished),
            ("sess-1", "ob3", None)
        );

        std::fs::write(
            &meta_path,
            r#"{"agentType":"ob3","spawnDepth":1,"finished":true}"#,
        )
        .unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&meta_path)
            .unwrap()
            .set_modified(later)
            .unwrap();
        ingest_claude_from_home(&gdb, home.path()).await;

        assert_eq!(gdb.agent_run("a1b2c3").await.unwrap().finished, Some(true));
    }

    /// A subagent tree with one run whose `.meta.json` is `meta`; returns
    /// the home directory and the meta file's path.
    fn subagent_home(meta: &str) -> (TempDir, PathBuf) {
        let home = TempDir::new().unwrap();
        let subagents = home
            .path()
            .join(".claude")
            .join("projects")
            .join("-proj")
            .join("sess-1")
            .join("subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        std::fs::write(
            subagents.join("agent-a1b2c3.jsonl"),
            format!("{SUBAGENT_LINE}\n"),
        )
        .unwrap();
        let meta_path = subagents.join("agent-a1b2c3.meta.json");
        std::fs::write(&meta_path, meta).unwrap();
        (home, meta_path)
    }

    fn set_mtime(path: &Path, at: std::time::SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    /// Claude Code rewrites the meta file to set `finished`; a rewrite in the
    /// same second as the last read, to the same length, is still read.
    #[tokio::test]
    async fn a_meta_rewrite_within_the_same_second_is_read() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let (home, meta_path) = subagent_home(r#"{"agentType":"ob3","finished":false}"#);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let second = std::time::UNIX_EPOCH + std::time::Duration::from_secs(now.as_secs());
        set_mtime(&meta_path, second + std::time::Duration::from_millis(100));
        ingest_claude_from_home(&gdb, home.path()).await;
        assert_eq!(gdb.agent_run("a1b2c3").await.unwrap().finished, Some(false));

        std::fs::write(&meta_path, r#"{"agentType":"ob3", "finished":true}"#).unwrap();
        set_mtime(&meta_path, second + std::time::Duration::from_millis(900));
        ingest_claude_from_home(&gdb, home.path()).await;

        assert_eq!(gdb.agent_run("a1b2c3").await.unwrap().finished, Some(true));
    }

    /// A run that could not be recorded is not marked as read: the ingest
    /// reports partial coverage and the next one records it.
    #[tokio::test]
    async fn a_run_that_failed_to_record_is_retried() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let (home, _) = subagent_home(r#"{"agentType":"ob3"}"#);
        let db_path = db_tmp.path().join(".tokensave").join("global.db");
        let side = libsql::Builder::new_local(&db_path).build().await.unwrap();
        let side = side.connect().unwrap();
        side.execute("ALTER TABLE agent_runs RENAME TO agent_runs_away", ())
            .await
            .unwrap();

        let stats = ingest_claude_from_home(&gdb, home.path()).await;
        assert_eq!(stats.coverage[0].state, CoverageState::Partial);

        side.execute("ALTER TABLE agent_runs_away RENAME TO agent_runs", ())
            .await
            .unwrap();
        ingest_claude_from_home(&gdb, home.path()).await;
        assert_eq!(gdb.agent_run("a1b2c3").await.unwrap().role, "ob3");
    }

    /// A subagent message's first line is written while it streams, without
    /// `speed` or thinking tokens; its last line has them. The turn keeps
    /// them, with the last line's output tokens (#250). The ingest stats
    /// still count the turn as its first line inserted it.
    #[tokio::test]
    async fn a_later_line_of_a_message_fills_its_usage_detail() {
        let streaming = r#"{"type":"assistant","sessionId":"sess-1","agentId":"a1b2c3","message":{"id":"msg_s","model":"claude-opus-5-5","usage":{"input_tokens":2,"output_tokens":16,"service_tier":"standard"},"content":[]},"timestamp":"2026-10-27T04:00:00.000Z"}"#;
        let finished = r#"{"type":"assistant","sessionId":"sess-1","agentId":"a1b2c3","message":{"id":"msg_s","model":"claude-opus-5-5","usage":{"input_tokens":2,"output_tokens":559,"service_tier":"standard","speed":"standard","output_tokens_details":{"thinking_tokens":648}},"content":[]},"timestamp":"2026-10-27T04:00:01.000Z"}"#;
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let (home, meta_path) = subagent_home(r#"{"agentType":"ob3"}"#);
        std::fs::write(
            meta_path.with_extension("").with_extension("jsonl"),
            format!("{streaming}\n{finished}\n"),
        )
        .unwrap();

        let stats = ingest_claude_from_home(&gdb, home.path()).await;

        assert_eq!((stats.turns_inserted, stats.tokens_consumed), (1, 18));
        let detail = gdb.turn_detail("msg_s").await.unwrap();
        assert_eq!(detail.speed.as_deref(), Some("standard"));
        assert_eq!(detail.thinking_tokens, Some(648));
        assert_eq!(detail.run_id.as_deref(), Some("a1b2c3"));
        assert_eq!(stored_turn(&db_tmp, "msg_s").await.output_tokens, 559);
    }

    /// One assistant line of message `id`, holding the content block `block`.
    fn assistant_line(id: &str, output_tokens: u64, stop: &str, block: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"id":"{id}","model":"claude-opus-4-6","stop_reason":{stop},"usage":{{"input_tokens":3,"output_tokens":{output_tokens},"cache_read_input_tokens":20}},"content":[{block}]}},"timestamp":"2026-10-27T04:00:00.000Z"}}"#
        )
    }

    const THINKING: &str = r#"{"type":"thinking","thinking":""}"#;
    const TEXT: &str = r#"{"type":"text","text":"x"}"#;

    fn tool_use(id: &str, name: &str, command: &str) -> String {
        format!(
            r#"{{"type":"tool_use","id":"{id}","name":"{name}","input":{{"command":"{command}"}}}}"#
        )
    }

    /// A main-session transcript holding `lines`; returns the home directory
    /// and the transcript's path.
    fn session_home(lines: &[String]) -> (TempDir, PathBuf) {
        let home = TempDir::new().unwrap();
        let project = home.path().join(".claude").join("projects").join("-proj");
        std::fs::create_dir_all(&project).unwrap();
        let transcript = project.join("sess-1.jsonl");
        std::fs::write(&transcript, lines.join("\n") + "\n").unwrap();
        (home, transcript)
    }

    /// The columns of a recorded turn that its later lines change, read
    /// through a second connection to the test database.
    async fn stored_turn(db_tmp: &TempDir, message_id: &str) -> StoredTurn {
        let db_path = db_tmp.path().join(".tokensave").join("global.db");
        let db = libsql::Builder::new_local(&db_path).build().await.unwrap();
        let conn = db.connect().unwrap();
        let mut rows = conn
            .query(
                "SELECT tool_names, category, output_tokens, cost_usd, tool_use_ids,
                        bash_signals, tool_result_tokens
                 FROM turns WHERE message_id = ?1",
                libsql::params![message_id.to_string()],
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        StoredTurn {
            tool_names: row.get(0).unwrap(),
            category: row.get(1).unwrap(),
            output_tokens: row.get(2).unwrap(),
            cost_usd: row.get(3).unwrap(),
            tool_use_ids: row.get(4).ok(),
            bash_signals: row.get(5).ok(),
            tool_result_tokens: row.get(6).unwrap(),
        }
    }

    #[derive(Debug, PartialEq)]
    struct StoredTurn {
        tool_names: String,
        category: String,
        output_tokens: i64,
        cost_usd: f64,
        tool_use_ids: Option<String>,
        bash_signals: Option<i64>,
        tool_result_tokens: i64,
    }

    /// Claude Code writes a message one block per line; a tool call on its
    /// third line is the turn's (#250).
    #[tokio::test]
    async fn a_tool_call_after_the_first_line_is_the_turns() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let (home, _) = session_home(&[
            assistant_line("msg_m", 8, "null", THINKING),
            assistant_line("msg_m", 8, "null", TEXT),
            assistant_line(
                "msg_m",
                8,
                r#""tool_use""#,
                &tool_use("toolu_e", "Edit", ""),
            ),
        ]);

        let stats = ingest_claude_from_home(&gdb, home.path()).await;

        assert_eq!(stats.turns_inserted, 1);
        let turn = stored_turn(&db_tmp, "msg_m").await;
        assert_eq!(
            (turn.tool_names.as_str(), turn.category.as_str()),
            ("Edit", "coding")
        );
        assert_eq!(turn.tool_use_ids.as_deref(), Some("toolu_e"));
    }

    /// A subagent message's first line is written while it streams; the
    /// turn holds the final line's output tokens, and is priced by them.
    #[tokio::test]
    async fn a_streamed_message_keeps_its_final_output_tokens() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let (home, _) = session_home(&[
            assistant_line("msg_s", 8, "null", THINKING),
            assistant_line("msg_s", 1200, r#""end_turn""#, TEXT),
        ]);

        ingest_claude_from_home(&gdb, home.path()).await;

        let turn = stored_turn(&db_tmp, "msg_s").await;
        assert_eq!(turn.output_tokens, 1200);
        let priced = pricing::cost_of_turn("claude-opus-4-6", 3, 1200, 0, 20);
        assert!(priced > 0.0);
        assert!((turn.cost_usd - priced).abs() < f64::EPSILON);
    }

    /// A file read again from its start (its offset lost) changes no turn:
    /// no output counted twice, no tool name repeated.
    #[tokio::test]
    async fn a_transcript_read_twice_changes_no_turn() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let (home, _) = session_home(&[
            assistant_line("msg_r", 8, "null", &tool_use("toolu_a", "Read", "")),
            assistant_line("msg_r", 8, "null", &tool_use("toolu_b", "Read", "")),
            assistant_line(
                "msg_r",
                900,
                r#""tool_use""#,
                &tool_use("toolu_c", "Bash", "git status"),
            ),
        ]);
        ingest_claude_from_home(&gdb, home.path()).await;
        let once = stored_turn(&db_tmp, "msg_r").await;
        assert_eq!(once.tool_names, "Read,Read,Bash");
        assert_eq!(once.category, "git_ops");

        let db_path = db_tmp.path().join(".tokensave").join("global.db");
        let side = libsql::Builder::new_local(&db_path).build().await.unwrap();
        side.connect()
            .unwrap()
            .execute("DELETE FROM parse_offsets", ())
            .await
            .unwrap();
        let stats = ingest_claude_from_home(&gdb, home.path()).await;

        assert_eq!(stats.turns_inserted, 0);
        assert_eq!(stored_turn(&db_tmp, "msg_r").await, once);
    }

    /// A parse that stops between two lines of a message and resumes later
    /// records the turn a single pass records.
    #[tokio::test]
    async fn a_message_split_by_a_resumed_parse_is_recorded_whole() {
        let lines = [
            assistant_line(
                "msg_p",
                8,
                "null",
                &tool_use("toolu_a", "Bash", "fix the import"),
            ),
            assistant_line("msg_p", 8, "null", TEXT),
            assistant_line(
                "msg_p",
                640,
                r#""tool_use""#,
                &tool_use("toolu_b", "Edit", ""),
            ),
        ];

        let single_tmp = TempDir::new().unwrap();
        let single = open_test_db(&single_tmp).await;
        let (single_home, _) = session_home(&lines);
        ingest_claude_from_home(&single, single_home.path()).await;

        let split_tmp = TempDir::new().unwrap();
        let split = open_test_db(&split_tmp).await;
        let (split_home, transcript) = session_home(&lines[..2]);
        let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        set_mtime(&transcript, earlier);
        ingest_claude_from_home(&split, split_home.path()).await;
        std::fs::write(&transcript, lines.join("\n") + "\n").unwrap();
        ingest_claude_from_home(&split, split_home.path()).await;

        let whole = stored_turn(&single_tmp, "msg_p").await;
        assert_eq!(whole.category, "debugging");
        assert_eq!(whole.output_tokens, 640);
        assert_eq!(stored_turn(&split_tmp, "msg_p").await, whole);
    }

    /// Tool results are attributed to the turn whose line issued the call,
    /// whichever line of the message that was (#474 with #250).
    #[tokio::test]
    async fn a_result_reaches_a_call_from_a_later_line() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_g","content":"0123456789012345678901234567890123456789"}]}}"#;
        let (home, _) = session_home(&[
            assistant_line("msg_t", 8, "null", TEXT),
            assistant_line(
                "msg_t",
                8,
                r#""tool_use""#,
                &tool_use("toolu_g", "Grep", ""),
            ),
            result.to_string(),
        ]);

        ingest_claude_from_home(&gdb, home.path()).await;

        let turn = stored_turn(&db_tmp, "msg_t").await;
        assert_eq!(
            (turn.tool_names.as_str(), turn.tool_result_tokens),
            ("Grep", 10)
        );
    }

    /// Ingests `lines`, clears the offsets, ingests again; returns the turn
    /// `message_id` after each pass.
    async fn turn_after_two_reads(lines: &[String], message_id: &str) -> (StoredTurn, StoredTurn) {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let (home, _) = session_home(lines);
        ingest_claude_from_home(&gdb, home.path()).await;
        let once = stored_turn(&db_tmp, message_id).await;
        let db_path = db_tmp.path().join(".tokensave").join("global.db");
        let side = libsql::Builder::new_local(&db_path).build().await.unwrap();
        side.connect()
            .unwrap()
            .execute("DELETE FROM parse_offsets", ())
            .await
            .unwrap();
        ingest_claude_from_home(&gdb, home.path()).await;
        (once, stored_turn(&db_tmp, message_id).await)
    }

    /// A call id that is empty, or holds a comma, cannot be found again in
    /// the stored list: reading its line again must still add nothing.
    #[tokio::test]
    async fn a_call_with_an_unusable_id_is_not_appended_on_every_read() {
        for id in ["", "toolu_a,b"] {
            let (once, twice) = turn_after_two_reads(
                &[
                    assistant_line("msg_u", 8, "null", TEXT),
                    assistant_line("msg_u", 8, r#""tool_use""#, &tool_use(id, "Read", "")),
                ],
                "msg_u",
            )
            .await;
            assert_eq!(twice, once, "{id:?}");
        }
    }

    fn tool_result(id: &str) -> String {
        format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":"0123456789012345678901234567890123456789"}}]}}}}"#
        )
    }

    /// A transcript read again counts each tool result once (#255), whether
    /// its call was on the first line of its message or a later one, and
    /// with the results in separate user messages.
    #[tokio::test]
    async fn results_are_counted_once_however_often_the_file_is_read() {
        let (once, twice) = turn_after_two_reads(
            &[
                assistant_line("msg_c", 8, "null", &tool_use("toolu_g", "Grep", "")),
                assistant_line(
                    "msg_c",
                    8,
                    r#""tool_use""#,
                    &tool_use("toolu_r", "Read", ""),
                ),
                tool_result("toolu_g"),
                tool_result("toolu_r"),
            ],
            "msg_c",
        )
        .await;
        assert_eq!(once.tool_result_tokens, 20);
        assert_eq!(twice, once);
    }

    /// A turn recorded before `result_ids` is added to while its size is 0,
    /// and left alone once it holds a size whose results are unknown.
    #[tokio::test]
    async fn a_turn_recorded_before_result_ids_is_added_to_only_while_empty() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let first = assistant_line("msg_l", 8, "null", &tool_use("toolu_a", "Read", ""));
        let legacy = parse_line(&first, "proj", "sess").unwrap().turn;
        assert!(gdb.insert_turn(&legacy).await);

        assert!(gdb.add_tool_result_tokens("msg_l", "toolu_a", 10).await);
        assert!(!gdb.add_tool_result_tokens("msg_l", "toolu_a", 10).await);
        assert!(gdb.add_tool_result_tokens("msg_l", "toolu_b", 5).await);
        assert_eq!(stored_turn(&db_tmp, "msg_l").await.tool_result_tokens, 15);

        let other = assistant_line("msg_k", 8, "null", &tool_use("toolu_c", "Read", ""));
        assert!(
            gdb.insert_turn(&parse_line(&other, "proj", "sess").unwrap().turn)
                .await
        );
        let db_path = db_tmp.path().join(".tokensave").join("global.db");
        let side = libsql::Builder::new_local(&db_path).build().await.unwrap();
        side.connect()
            .unwrap()
            .execute(
                "UPDATE turns SET tool_result_tokens = 7 WHERE message_id = 'msg_k'",
                (),
            )
            .await
            .unwrap();
        assert!(!gdb.add_tool_result_tokens("msg_k", "toolu_c", 10).await);
        assert_eq!(stored_turn(&db_tmp, "msg_k").await.tool_result_tokens, 7);
    }

    #[tokio::test]
    async fn a_result_with_an_unusable_id_is_not_counted() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let line = assistant_line("msg_v", 8, "null", TEXT);
        assert!(
            gdb.insert_claude_turn(&parse_line(&line, "proj", "sess").unwrap())
                .await
        );
        assert!(!gdb.add_tool_result_tokens("msg_v", "", 10).await);
        assert!(!gdb.add_tool_result_tokens("msg_v", "toolu_a,b", 10).await);
        assert_eq!(stored_turn(&db_tmp, "msg_v").await.tool_result_tokens, 0);
    }

    /// The message ids of the recorded turns, in order.
    async fn recorded(db_tmp: &TempDir) -> Vec<String> {
        let db_path = db_tmp.path().join(".tokensave").join("global.db");
        let db = libsql::Builder::new_local(&db_path).build().await.unwrap();
        let conn = db.connect().unwrap();
        let mut rows = conn
            .query("SELECT message_id FROM turns ORDER BY message_id", ())
            .await
            .unwrap();
        let mut ids = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            ids.push(row.get(0).unwrap());
        }
        ids
    }

    /// The start of the current second, so two writes can share it.
    fn this_second() -> std::time::SystemTime {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(now.as_secs())
    }

    fn append(path: &Path, text: &str) {
        use std::io::Write;
        std::fs::File::options()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    /// Lines appended in the same second as the last read are read by the
    /// next ingest, though the file's mtime in seconds has not changed (#256).
    #[tokio::test]
    async fn lines_written_in_the_same_second_as_a_read_are_read() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let (home, transcript) = session_home(&[assistant_line("msg_a", 8, "null", TEXT)]);
        let second = this_second();
        set_mtime(&transcript, second + std::time::Duration::from_millis(100));
        ingest_claude_from_home(&gdb, home.path()).await;

        append(
            &transcript,
            &(assistant_line("msg_b", 8, r#""end_turn""#, TEXT) + "\n"),
        );
        set_mtime(&transcript, second + std::time::Duration::from_millis(900));
        ingest_claude_from_home(&gdb, home.path()).await;

        assert_eq!(recorded(&db_tmp).await, ["msg_a", "msg_b"]);
    }

    /// A last line with no newline is still being written: it is left for
    /// the next ingest, which reads it whole once it is complete (#256).
    #[tokio::test]
    async fn a_line_still_being_written_is_read_once_complete() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let first = assistant_line("msg_a", 8, "null", TEXT) + "\n";
        let second = assistant_line("msg_b", 1200, r#""end_turn""#, TEXT) + "\n";
        let (head, tail) = second.split_at(second.len() / 2);
        let (home, transcript) = session_home(&[]);
        std::fs::write(&transcript, format!("{first}{head}")).unwrap();
        set_mtime(
            &transcript,
            std::time::SystemTime::now() - std::time::Duration::from_secs(60),
        );
        ingest_claude_from_home(&gdb, home.path()).await;

        assert_eq!(recorded(&db_tmp).await, ["msg_a"]);
        let key = transcript.to_string_lossy().to_string();
        assert_eq!(
            gdb.get_parse_offset(&key).await.map(|(offset, _)| offset),
            Some(first.len() as u64)
        );

        append(&transcript, tail);
        ingest_claude_from_home(&gdb, home.path()).await;

        assert_eq!(recorded(&db_tmp).await, ["msg_a", "msg_b"]);
        assert_eq!(stored_turn(&db_tmp, "msg_b").await.output_tokens, 1200);
    }

    /// A file shorter than the offset stored for it was replaced: it is read
    /// from its start, even when its mtime is later (#256).
    #[tokio::test]
    async fn a_file_shorter_than_its_offset_is_read_from_the_start() {
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let (home, transcript) = session_home(&[
            assistant_line("msg_a", 8, "null", TEXT),
            assistant_line("msg_b", 8, "null", TEXT),
        ]);
        set_mtime(
            &transcript,
            std::time::SystemTime::now() - std::time::Duration::from_secs(60),
        );
        ingest_claude_from_home(&gdb, home.path()).await;

        std::fs::write(
            &transcript,
            assistant_line("msg_c", 8, r#""end_turn""#, TEXT) + "\n",
        )
        .unwrap();
        set_mtime(&transcript, std::time::SystemTime::now());
        ingest_claude_from_home(&gdb, home.path()).await;

        assert_eq!(recorded(&db_tmp).await, ["msg_a", "msg_b", "msg_c"]);
    }

    /// A file with nothing past its stored offset, and the same mtime, is
    /// not opened: an unreadable one does not make the coverage partial.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_unchanged_file_is_not_opened() {
        use std::os::unix::fs::PermissionsExt;
        let db_tmp = TempDir::new().unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let (home, transcript) = session_home(&[assistant_line("msg_a", 8, "null", TEXT)]);
        ingest_claude_from_home(&gdb, home.path()).await;

        std::fs::set_permissions(&transcript, std::fs::Permissions::from_mode(0o000)).unwrap();
        let stats = ingest_claude_from_home(&gdb, home.path()).await;
        std::fs::set_permissions(&transcript, std::fs::Permissions::from_mode(0o600)).unwrap();

        assert_eq!(stats.coverage[0].state, CoverageState::Complete);
    }

    /// A database newer than this binary is not ingested into, by any
    /// source (inkcast #248).
    #[tokio::test]
    async fn a_database_newer_than_the_binary_is_not_ingested_into() {
        let db_tmp = TempDir::new().unwrap();
        drop(open_test_db(&db_tmp).await);
        let db_path = db_tmp.path().join(".tokensave").join("global.db");
        let side = libsql::Builder::new_local(&db_path).build().await.unwrap();
        side.connect()
            .unwrap()
            .execute_batch(&format!(
                "PRAGMA user_version = {}",
                crate::global_db::SCHEMA_VERSION + 1
            ))
            .await
            .unwrap();
        let gdb = open_test_db(&db_tmp).await;
        let (home, _) = session_home(&[assistant_line("msg_a", 8, "null", TEXT)]);

        let claude = ingest_claude_from_home(&gdb, home.path()).await;
        let all = ingest_from_home(&gdb, home.path()).await;
        let pi = ingest_pi_from_home(&gdb, home.path()).await;

        for stats in [claude, all, pi] {
            assert_eq!(stats.turns_inserted, 0);
            assert!(stats.coverage.is_empty());
        }
        assert!(recorded(&db_tmp).await.is_empty());
    }
}
