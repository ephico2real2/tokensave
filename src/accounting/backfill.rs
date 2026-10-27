//! Fills the turn detail of Claude Code turns recorded before it was kept
//! (inkcast #249).
//!
//! Turns recorded before the detail columns existed hold `NULL` in all of
//! them, and an ingest never reads their lines again: the parse offsets skip
//! them. A backfill reads every transcript from its start into memory, and
//! fills only those columns, only where they are `NULL`. It never inserts a
//! turn, moves an offset, or touches a token count, cost, tool result,
//! category or tool name, so running it twice changes nothing the second
//! time.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::global_db::GlobalDb;
use crate::types::TurnDetail;

use super::parser::{extract_path_parts, find_session_files, parse_line};

/// The detail columns, in [`TurnDetail`] order.
pub const DETAIL_COLUMNS: [&str; 8] = [
    "effort",
    "requested_model",
    "thinking_tokens",
    "thinking_ms",
    "speed",
    "service_tier",
    "git_branch",
    "run_id",
];

/// What a backfill found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillReport {
    /// Whether it wrote nothing.
    pub dry_run: bool,
    /// Transcript files read, and of those the ones that could not be read
    /// to the end.
    pub files: u64,
    pub unreadable_files: u64,
    /// Claude Code turns in the database, and of those the ones whose
    /// message is still in a transcript.
    pub turns: u64,
    pub turns_on_disk: u64,
    /// Turns given at least one value: written, or with `dry_run`, to be.
    pub turns_updated: u64,
    /// Per column of [`DETAIL_COLUMNS`], the turns given a value.
    pub filled: [u64; 8],
}

/// Why a backfill did not run to the end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillError {
    /// The database's schema is newer than this binary's: it is read only.
    NewerSchema(u32),
    /// The recorded turns could not be read.
    ReadTurns,
    /// A write failed; the batches before it were kept.
    Write,
}

impl fmt::Display for BackfillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewerSchema(found) => f.write_str(&crate::global_db::newer_schema_notice(*found)),
            Self::ReadTurns => f.write_str("the recorded turns could not be read"),
            Self::Write => f.write_str(
                "writing the turn detail failed; the turns written before it keep their values",
            ),
        }
    }
}

/// Backfills from the Claude Code transcripts under `home/.claude/projects`.
pub async fn backfill_from_home(
    gdb: &GlobalDb,
    home: &Path,
    dry_run: bool,
) -> Result<BackfillReport, BackfillError> {
    if let Some(found) = gdb.newer_schema() {
        return Err(BackfillError::NewerSchema(found));
    }
    let stored = gdb
        .claude_turn_details()
        .await
        .ok_or(BackfillError::ReadTurns)?;
    let (read, files, unreadable_files) = transcript_details(home, &stored);

    let mut filled = [0; 8];
    let mut fills = Vec::new();
    for (message_id, found) in &read {
        let Some(have) = stored.get(message_id) else {
            continue;
        };
        let gains = gains(have, found);
        if gains.iter().any(|&gain| gain) {
            for (count, gain) in filled.iter_mut().zip(gains) {
                *count += u64::from(gain);
            }
            fills.push((message_id.clone(), found.clone()));
        }
    }
    // In message order, so a run that stops part-way has filled a prefix.
    fills.sort_by(|a, b| a.0.cmp(&b.0));

    let turns_updated = if dry_run {
        fills.len() as u64
    } else {
        gdb.fill_turn_details(&fills)
            .await
            .ok_or(BackfillError::Write)?
    };
    Ok(BackfillReport {
        dry_run,
        files,
        unreadable_files,
        turns: stored.len() as u64,
        turns_on_disk: read.len() as u64,
        turns_updated,
        filled,
    })
}

/// The detail each recorded message's transcript lines give it, as an
/// ingest from scratch would store it; with the files read and those that
/// could not be read to the end.
///
/// A turn is inserted from its message's first line, and later lines fill
/// only `thinking_tokens`, `speed` and `service_tier`
/// ([`super::turn_merge::merge_line`]); files are read in the ingest's order.
fn transcript_details(
    home: &Path,
    recorded: &HashMap<String, TurnDetail>,
) -> (HashMap<String, TurnDetail>, u64, u64) {
    let (files, _) = find_session_files(home);
    let mut unreadable = 0;
    let mut details: HashMap<String, TurnDetail> = HashMap::new();
    for file in &files {
        let Ok(f) = fs::File::open(file) else {
            unreadable += 1;
            continue;
        };
        let (project_hash, session_id) = extract_path_parts(file);
        let mut reader = BufReader::new(f);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Err(_) => {
                    unreadable += 1;
                    break;
                }
                // Still being written; the ingest leaves it too.
                Ok(_) if !line.ends_with('\n') => break,
                Ok(_) => {}
            }
            let Some(parsed) = parse_line(line.trim(), &project_hash, &session_id) else {
                continue;
            };
            let turn = parsed.turn;
            if !recorded.contains_key(&turn.message_id) {
                continue;
            }
            match details.get_mut(&turn.message_id) {
                None => {
                    details.insert(turn.message_id, turn.detail);
                }
                Some(first) => {
                    let later = turn.detail;
                    first.thinking_tokens = first.thinking_tokens.or(later.thinking_tokens);
                    first.speed = first.speed.take().or(later.speed);
                    first.service_tier = first.service_tier.take().or(later.service_tier);
                }
            }
        }
    }
    (details, files.len() as u64, unreadable)
}

/// Per column of [`DETAIL_COLUMNS`], whether `found` gives `have` a value it
/// lacks.
fn gains(have: &TurnDetail, found: &TurnDetail) -> [bool; 8] {
    [
        have.effort.is_none() && found.effort.is_some(),
        have.requested_model.is_none() && found.requested_model.is_some(),
        have.thinking_tokens.is_none() && found.thinking_tokens.is_some(),
        have.thinking_ms.is_none() && found.thinking_ms.is_some(),
        have.speed.is_none() && found.speed.is_some(),
        have.service_tier.is_none() && found.service_tier.is_some(),
        have.git_branch.is_none() && found.git_branch.is_some(),
        have.run_id.is_none() && found.run_id.is_some(),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A main session whose first message spans two lines (detail on the
    /// first, usage detail on the second), a second message with only a
    /// branch, and a subagent run's message.
    fn home_with_transcripts() -> TempDir {
        let home = TempDir::new().unwrap();
        let project = home.path().join(".claude").join("projects").join("-proj");
        let subagents = project.join("sess-1").join("subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        let main = [
            r#"{"type":"assistant","sessionId":"sess-1","gitBranch":"main","perTurnEffort":"max","requestedModel":"claude-opus-5-5[1m]","thinkingDurationMs":800,"message":{"id":"msg_a","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":8},"content":[{"type":"thinking","thinking":""}]},"timestamp":"2026-10-27T04:00:00.000Z"}"#,
            r#"{"type":"assistant","sessionId":"sess-1","gitBranch":"main","perTurnEffort":"max","message":{"id":"msg_a","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":90,"speed":"standard","service_tier":"standard","output_tokens_details":{"thinking_tokens":40}},"content":[{"type":"text","text":"x"}]},"timestamp":"2026-10-27T04:00:01.000Z"}"#,
            r#"{"type":"assistant","sessionId":"sess-1","gitBranch":"feat","message":{"id":"msg_b","model":"claude-opus-4-6","usage":{"input_tokens":2,"output_tokens":5},"content":[]},"timestamp":"2026-10-27T04:01:00.000Z"}"#,
        ];
        std::fs::write(project.join("sess-1.jsonl"), main.join("\n") + "\n").unwrap();
        let run = r#"{"type":"assistant","sessionId":"sess-1","agentId":"a1b2c3","gitBranch":"main","effort":"high","message":{"id":"msg_s","model":"claude-opus-4-6","usage":{"input_tokens":1,"output_tokens":4,"service_tier":"priority"},"content":[]},"timestamp":"2026-10-27T04:02:00.000Z"}"#;
        std::fs::write(subagents.join("agent-a1b2c3.jsonl"), format!("{run}\n")).unwrap();
        home
    }

    async fn open(tmp: &TempDir) -> GlobalDb {
        GlobalDb::open_at(&tmp.path().join("global.db"))
            .await
            .unwrap()
    }

    async fn side(tmp: &TempDir) -> libsql::Connection {
        let db = libsql::Builder::new_local(tmp.path().join("global.db"))
            .build()
            .await
            .unwrap();
        db.connect().unwrap()
    }

    /// Every row of `table`, each rendered as text, in a fixed order.
    async fn rows(tmp: &TempDir, table: &str, order: &str) -> Vec<String> {
        let conn = side(tmp).await;
        let mut rows = conn
            .query(&format!("SELECT * FROM {table} ORDER BY {order}"), ())
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            let values: Vec<String> = (0..row.column_count())
                .map(|i| format!("{:?}", row.get_value(i).unwrap()))
                .collect();
            out.push(values.join("|"));
        }
        out
    }

    /// A database ingested from the transcripts, with its detail columns
    /// emptied as they were before they were kept, and one turn whose
    /// transcript is gone.
    async fn legacy_database(home: &TempDir) -> (TempDir, GlobalDb) {
        let tmp = TempDir::new().unwrap();
        let gdb = open(&tmp).await;
        super::super::parser::ingest_claude_from_home(&gdb, home.path()).await;
        let mut gone = super::super::parser::parse_line(
            r#"{"type":"assistant","message":{"id":"msg_gone","model":"claude-opus-4-6","usage":{"input_tokens":1,"output_tokens":1},"content":[]},"timestamp":"2026-10-27T04:00:00.000Z"}"#,
            "-proj",
            "sess-0",
        )
        .unwrap()
        .turn;
        gone.detail = TurnDetail::default();
        assert!(gdb.insert_turn(&gone).await);
        side(&tmp)
            .await
            .execute_batch(
                "UPDATE turns SET effort = NULL, requested_model = NULL,
                     thinking_tokens = NULL, thinking_ms = NULL, speed = NULL,
                     service_tier = NULL, git_branch = NULL, run_id = NULL",
            )
            .await
            .unwrap();
        (tmp, gdb)
    }

    /// A backfilled database holds what an ingest from scratch stores, and
    /// changes no other column, offset or row count (inkcast #249).
    #[tokio::test]
    async fn a_backfill_stores_what_an_ingest_from_scratch_stores() {
        let home = home_with_transcripts();
        let fresh_tmp = TempDir::new().unwrap();
        let fresh = open(&fresh_tmp).await;
        super::super::parser::ingest_claude_from_home(&fresh, home.path()).await;
        let (tmp, gdb) = legacy_database(&home).await;
        let offsets = rows(&tmp, "parse_offsets", "file_path").await;

        let report = backfill_from_home(&gdb, home.path(), false).await.unwrap();

        assert_eq!(
            report,
            BackfillReport {
                dry_run: false,
                files: 2,
                unreadable_files: 0,
                turns: 4,
                turns_on_disk: 3,
                turns_updated: 3,
                filled: [2, 1, 1, 1, 1, 2, 3, 1],
            }
        );
        let mut expected = rows(&fresh_tmp, "turns", "message_id").await;
        let backfilled = rows(&tmp, "turns", "message_id").await;
        // The turn with no transcript keeps its unknown detail.
        let gone = backfilled
            .iter()
            .find(|row| row.contains("msg_gone"))
            .unwrap()
            .clone();
        expected.push(gone);
        expected.sort();
        assert_eq!(backfilled, expected);
        assert_eq!(rows(&tmp, "parse_offsets", "file_path").await, offsets);

        let again = backfill_from_home(&gdb, home.path(), false).await.unwrap();
        assert_eq!((again.turns_updated, again.filled), (0, [0; 8]));
        assert_eq!(rows(&tmp, "turns", "message_id").await, backfilled);
    }

    /// A stored value is never replaced, even by a different one.
    #[tokio::test]
    async fn a_stored_value_is_kept() {
        let home = home_with_transcripts();
        let (tmp, gdb) = legacy_database(&home).await;
        side(&tmp)
            .await
            .execute(
                "UPDATE turns SET effort = 'low' WHERE message_id = 'msg_a'",
                (),
            )
            .await
            .unwrap();

        let report = backfill_from_home(&gdb, home.path(), false).await.unwrap();

        assert_eq!(report.filled[0], 1, "only msg_s gains an effort");
        assert_eq!(
            gdb.turn_detail("msg_a").await.unwrap().effort.as_deref(),
            Some("low")
        );
    }

    /// A dry run reports what a run would do, and leaves the database files
    /// byte for byte as they were.
    #[tokio::test]
    async fn a_dry_run_writes_nothing_and_counts_the_same() {
        let home = home_with_transcripts();
        let (tmp, gdb) = legacy_database(&home).await;
        let files =
            || ["global.db", "global.db-wal"].map(|name| std::fs::read(tmp.path().join(name)).ok());
        let before = files();

        let dry = backfill_from_home(&gdb, home.path(), true).await.unwrap();

        assert_eq!(files(), before);
        let run = backfill_from_home(&gdb, home.path(), false).await.unwrap();
        assert!(dry.dry_run && !run.dry_run);
        assert_eq!(
            (dry.turns, dry.turns_on_disk, dry.turns_updated, dry.filled),
            (run.turns, run.turns_on_disk, run.turns_updated, run.filled)
        );
    }

    #[tokio::test]
    async fn a_database_newer_than_the_binary_is_not_backfilled() {
        let home = home_with_transcripts();
        let (tmp, gdb) = legacy_database(&home).await;
        drop(gdb);
        let newer = crate::global_db::SCHEMA_VERSION + 1;
        side(&tmp)
            .await
            .execute_batch(&format!("PRAGMA user_version = {newer}"))
            .await
            .unwrap();
        let gdb = open(&tmp).await;

        let result = backfill_from_home(&gdb, home.path(), false).await;

        assert_eq!(result, Err(BackfillError::NewerSchema(newer)));
    }

    #[test]
    fn a_value_is_gained_only_where_none_is_held() {
        let have = TurnDetail {
            effort: Some("high".into()),
            ..TurnDetail::default()
        };
        let found = TurnDetail {
            effort: Some("max".into()),
            speed: Some("standard".into()),
            ..TurnDetail::default()
        };
        assert_eq!(
            gains(&have, &found),
            [false, false, false, false, true, false, false, false]
        );
        assert_eq!(gains(&found, &found), [false; 8]);
    }
}
