//! User-level database that tracks all `TokenSave` projects and their saved tokens.
//!
//! Stored at `~/.tokensave/global.db`, this DB holds one row per project with
//! the project's DB path and its cumulative tokens-saved count. All operations
//! are best-effort: failures are silently ignored so they never block the main
//! MCP server loop.

use std::path::{Path, PathBuf};

use libsql::{params, Builder, Connection, Database as LibsqlDatabase, TransactionBehavior};

use crate::accounting::classifier::BashSignals;
use crate::accounting::turn_merge::{merge_line, ClaudeLine, TurnState};

/// Total savings + call count for a project (or all projects when `project` is None).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SavingsTotal {
    pub saved_tokens: u64,
    pub calls: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SavingsDay {
    /// Start-of-day epoch seconds (UTC).
    pub day: i64,
    pub saved_tokens: u64,
    pub calls: u64,
}

/// Per-agent cost summary returned by `cost_by_agent_since`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentCostSummary {
    pub agent: String,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    /// None if any row in the group has NULL credits (i.e. Claude turns).
    pub credits: Option<u64>,
    pub turns: u64,
}

mod migrations;

/// Rows written per transaction by [`GlobalDb::fill_turn_details`]: one
/// transaction per row would sync once per row, and one for every row would
/// hold the write lock against the hooks for the whole run.
const FILL_BATCH_ROWS: usize = 500;

/// The schema version of `global.db` this binary writes: its `PRAGMA
/// user_version` once opened (inkcast #248).
pub const SCHEMA_VERSION: u32 = migrations::LATEST;

/// User-level database tracking all `TokenSave` projects.
pub struct GlobalDb {
    conn: Connection,
    /// The schema version found, when newer than [`SCHEMA_VERSION`]: the
    /// database is then read only.
    newer_schema: Option<u32>,
    _db: LibsqlDatabase,
}

/// What a binary older than its database says: the database is read only.
pub fn newer_schema_notice(found: u32) -> String {
    format!(
        "global.db is at schema version {found}, newer than this tokensave knows \
         ({SCHEMA_VERSION}): it is read only, and no usage is recorded. Upgrade tokensave."
    )
}

/// The schema version of the database at `db_path`, read without migrating
/// it, or `None` when it cannot be read.
pub async fn schema_version_at(db_path: &Path) -> Option<u32> {
    let db = Builder::new_local(db_path).build().await.ok()?;
    let conn = db.connect().ok()?;
    migrations::user_version(&conn).await.ok()
}

/// Returns the path to the global database: `~/.tokensave/global.db`.
pub fn global_db_path() -> Option<PathBuf> {
    crate::data_home::tokensave_home(crate::agents::home_dir()).map(|d| d.join("global.db"))
}

/// Resolves a project directory to the canonical string used as its global-DB key.
///
/// Every spelling of one directory must land on one row, so `.`, `./`, `../name`
/// and trailing separators are resolved away and symlinks followed. On Windows
/// the verbatim `\\?\` prefix that [`Path::canonicalize`] adds is stripped and
/// the drive letter upper-cased, so `d:\foo` and `D:\foo` share a row. Paths that
/// cannot be resolved on disk (a deleted project, or a synthetic path in a test)
/// fall back to a lexical absolute form, which is still stable for that caller.
///
/// # Examples
///
/// ```
/// use tokensave::global_db::normalize_project_key;
///
/// let cwd = std::env::current_dir().unwrap();
/// assert_eq!(normalize_project_key("."), normalize_project_key(&cwd));
/// ```
pub fn normalize_project_key(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    let resolved = path
        .canonicalize()
        .unwrap_or_else(|_| lexical_absolute(path));
    normalize_key_string(&resolved.to_string_lossy())
}

/// Applies the textual half of [`normalize_project_key`] to an already-absolute path.
fn normalize_key_string(path: &str) -> String {
    let mut s = path.to_string();

    // Windows canonicalization yields verbatim paths (`\\?\C:\x`, `\\?\UNC\srv\s`),
    // which no other code path produces. Strip the prefix so keys stay comparable.
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        s = format!(r"\\{rest}");
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        s = rest.to_string();
    }

    // `d:\foo` and `D:\foo` are the same directory. Only a drive-letter prefix is
    // case-folded — the rest of the path is left alone, since a case-insensitive
    // filesystem is not something we can assume from the string.
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        s.replace_range(..1, &s[..1].to_ascii_uppercase());
    }

    trim_trailing_separators(&mut s);
    s
}

/// Makes a path absolute without touching the filesystem, resolving `.` and `..`.
fn lexical_absolute(path: &Path) -> PathBuf {
    use std::path::Component;

    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };

    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // A leading `..` on a relative fallback has nothing to pop; keep it
                // rather than silently changing which directory is meant.
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Strips trailing `/` and `\` without eating a root (`/`, `C:\`).
fn trim_trailing_separators(s: &mut String) {
    let root_len =
        if s.len() >= 3 && s.as_bytes()[0].is_ascii_alphabetic() && s.as_bytes()[1] == b':' {
            3
        } else {
            1
        };
    while s.len() > root_len && (s.ends_with('/') || s.ends_with('\\')) {
        s.pop();
    }
}

/// Splits a normalized project key into its parent directory, or `None` at a root.
///
/// Both separators are accepted because a key may have been written by either
/// platform, and the string half of normalization does not rewrite separators.
fn parent_key(key: &str) -> Option<&str> {
    let cut = key.rfind(['/', '\\'])?;
    let parent = &key[..cut];
    // `/foo` and `C:\foo` sit directly under a root that the cut erased.
    if parent.is_empty() || parent.ends_with(':') {
        return None;
    }
    Some(parent)
}

/// Most siblings ever surfaced, however many share the parent directory.
///
/// A shared scratch or checkout directory can hold dozens of indexed projects.
/// Naming them all buries the useful ones and, since this list is embedded in
/// tool responses, can consume the whole response budget on its own.
pub const MAX_SIBLING_PROJECTS: usize = 5;

/// Selects the projects that sit directly beside `served`, from `all` known keys.
///
/// Only immediate siblings qualify: a nested project or an unrelated checkout
/// elsewhere on disk is not something the served session can be assumed to care
/// about, and a broader net would make the hint noisy enough to ignore. Inputs
/// are normalized here so callers may pass raw rows. The result is sorted — so
/// the surfaced list is stable across calls — and capped at
/// [`MAX_SIBLING_PROJECTS`].
///
/// # Examples
///
/// ```
/// use tokensave::global_db::sibling_project_keys;
///
/// let all = vec!["/w/svc".to_string(), "/w/lib".to_string(), "/other/x".to_string()];
/// assert_eq!(sibling_project_keys("/w/svc", &all), vec!["/w/lib".to_string()]);
/// ```
pub fn sibling_project_keys(served: &str, all: &[String]) -> Vec<String> {
    let served = normalize_key_string(served);
    let Some(parent) = parent_key(&served) else {
        return Vec::new();
    };

    let mut siblings: Vec<String> = all
        .iter()
        .map(|path| normalize_key_string(path))
        .filter(|key| *key != served && parent_key(key) == Some(parent))
        .collect();
    siblings.sort();
    siblings.dedup();
    siblings.truncate(MAX_SIBLING_PROJECTS);
    siblings
}

impl GlobalDb {
    /// Returns the initialized projects sitting directly beside `served_root`.
    ///
    /// Used to tell a session which other graphs it can reach with `graph_root`;
    /// see [`sibling_project_keys`] for the selection rule.
    pub async fn sibling_projects(&self, served_root: &Path) -> Vec<String> {
        let served = normalize_project_key(served_root);
        sibling_project_keys(&served, &self.list_project_paths().await)
    }

    /// Opens (or creates) the global database at an explicit path, migrating
    /// its schema to [`SCHEMA_VERSION`]. Returns `None` if the directory
    /// cannot be created, the DB fails to open or a migration fails. A
    /// database at a newer version is opened read only ([`Self::newer_schema`]).
    pub async fn open_at(db_path: &std::path::Path) -> Option<Self> {
        Self::open_with(db_path, migrations::MIGRATIONS).await
    }

    async fn open_with(
        db_path: &std::path::Path,
        schema: &[migrations::Migration],
    ) -> Option<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }

        let db = Builder::new_local(db_path).build().await.ok()?;
        let conn = db.connect().ok()?;

        Self::apply_pragmas(&conn).await.ok()?;

        // A failed migration fails the open: `insert_turn` names the columns
        // it adds, so every turn would fail while the parser still advanced
        // its offsets. The next open retries.
        let newer_schema = match migrations::migrate(&conn, schema).await {
            Ok(migrations::Outcome::Current) => None,
            Ok(migrations::Outcome::Newer(found)) => {
                // Written to by a binary that does not know its schema, the
                // database could be broken in ways neither version detects.
                conn.execute_batch("PRAGMA query_only = 1").await.ok()?;
                Some(found)
            }
            Err(error) => {
                tracing::warn!(%error, path = %db_path.display(), "global.db not opened");
                return None;
            }
        };
        if newer_schema.is_none() {
            Self::migrate_project_paths(&conn).await;
        }

        Some(Self {
            conn,
            newer_schema,
            _db: db,
        })
    }

    /// Sets the connection's PRAGMAs, retrying a lock error.
    ///
    /// `busy_timeout` first: `journal_mode = WAL` takes a lock, and before the
    /// timeout is set a concurrent open fails at once with "database is
    /// locked" (5 to 9 of 160 simultaneous opens) instead of waiting. The
    /// timeout does not cover every case: SQLite skips the busy handler where
    /// waiting could deadlock (<https://sqlite.org/c3ref/busy_handler.html>), as when a
    /// new file is first switched to WAL by one connection while others hold
    /// a read lock (1 of 80 simultaneous first opens). The batch is
    /// idempotent, so that error is retried.
    async fn apply_pragmas(conn: &Connection) -> libsql::Result<()> {
        const ATTEMPTS: u32 = 5;
        let mut attempt = 1;
        loop {
            match conn
                .execute_batch(
                    "PRAGMA busy_timeout = 5000;
                     PRAGMA journal_mode = WAL;
                     PRAGMA synchronous = NORMAL;",
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(error)
                    if attempt < ATTEMPTS && error.to_string().contains("database is locked") =>
                {
                    // 20, 40, 80, 160 ms.
                    tokio::time::sleep(std::time::Duration::from_millis(20 << (attempt - 1))).await;
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Opens (or creates) the global database. Returns `None` if the home
    /// directory cannot be determined or the DB fails to open. A database
    /// newer than this binary is reported on stderr, once per process.
    pub async fn open() -> Option<Self> {
        static NOTICE: std::sync::Once = std::sync::Once::new();
        let db_path = global_db_path()?;
        let db = Self::open_at(&db_path).await?;
        if let Some(found) = db.newer_schema {
            NOTICE.call_once(|| eprintln!("[tokensave] {}", newer_schema_notice(found)));
        }
        Some(db)
    }

    /// The schema version found, when newer than [`SCHEMA_VERSION`]. Such a
    /// database is read only: reports read it, and ingestion is refused.
    pub fn newer_schema(&self) -> Option<u32> {
        self.newer_schema
    }

    /// Rewrites pre-existing project keys to their canonical form, once per DB.
    ///
    /// Rows written before keys were canonicalised can name the same directory
    /// several ways (`d:\p` vs `D:\p`), splitting one project's savings across
    /// duplicates; those are merged by summing. Rows keyed by a relative path
    /// (a literal `.`) name no recoverable directory and are dropped — they
    /// would otherwise be permanently unattributable and never purged as stale.
    /// Best-effort: failures leave the DB as it was and the flag unset, so the
    /// next open retries.
    async fn migrate_project_paths(conn: &Connection) {
        if let Ok(mut flag) = conn
            .query(
                "SELECT 1 FROM meta WHERE key = 'projects_path_normalized'",
                (),
            )
            .await
        {
            if matches!(flag.next().await, Ok(Some(_))) {
                return;
            }
        }

        let Ok(mut rows) = conn
            .query("SELECT path, tokens_saved FROM projects", ())
            .await
        else {
            return;
        };
        let mut existing: Vec<(String, i64)> = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            if let (Ok(path), Ok(tokens)) = (row.get::<String>(0), row.get::<i64>(1)) {
                existing.push((path, tokens));
            }
        }

        // old key -> canonical key, for the rows that actually move.
        let mut remapped: Vec<(String, String)> = Vec::new();
        let mut merged: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
        let mut dropped: Vec<String> = Vec::new();
        for (path, tokens) in existing {
            if !Path::new(&path).is_absolute() {
                dropped.push(path);
                continue;
            }
            let key = normalize_project_key(&path);
            if key != path {
                remapped.push((path, key.clone()));
            }
            *merged.entry(key).or_insert(0) += tokens;
        }

        if !remapped.is_empty() || !dropped.is_empty() {
            if conn.execute("DELETE FROM projects", ()).await.is_err() {
                return;
            }
            for (key, tokens) in &merged {
                let _ = conn
                    .execute(
                        "INSERT INTO projects (path, tokens_saved) VALUES (?1, ?2)",
                        params![key.as_str(), *tokens],
                    )
                    .await;
            }
            // Keep the ledger addressable by the same keys, so `gain` for a
            // project still sees the history recorded under its old spelling.
            for (old, new) in &remapped {
                let _ = conn
                    .execute(
                        "UPDATE savings_ledger SET project_path = ?1 WHERE project_path = ?2",
                        params![new.as_str(), old.as_str()],
                    )
                    .await;
            }
        }

        let _ = conn
            .execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('projects_path_normalized', '1')",
                (),
            )
            .await;
    }

    /// Registers or updates a project's tokens-saved count. Best-effort.
    ///
    /// The path is canonicalised via [`normalize_project_key`] so that `.`,
    /// relative paths and case-variant drive letters all update one row.
    pub async fn upsert(&self, project_path: &Path, tokens_saved: u64) {
        let path_str = normalize_project_key(project_path);
        let _ = self
            .conn
            .execute(
                "INSERT INTO projects (path, tokens_saved) VALUES (?1, ?2)
                 ON CONFLICT(path) DO UPDATE SET tokens_saved = ?2",
                params![path_str, tokens_saved as i64],
            )
            .await;
    }

    /// Returns the stored `tokens_saved` count for a specific project, or 0 if not found.
    pub async fn get_project_tokens(&self, project_path: &Path) -> u64 {
        let path_str = normalize_project_key(project_path);
        let Ok(mut rows) = self
            .conn
            .query(
                "SELECT tokens_saved FROM projects WHERE path = ?1",
                params![path_str],
            )
            .await
        else {
            return 0;
        };
        match rows.next().await {
            Ok(Some(row)) => row.get::<i64>(0).unwrap_or(0) as u64,
            _ => 0,
        }
    }

    /// Returns the sum of `tokens_saved` across all tracked projects.
    pub async fn global_tokens_saved(&self) -> Option<u64> {
        let mut rows = self
            .conn
            .query("SELECT COALESCE(SUM(tokens_saved), 0) FROM projects", ())
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        let total: i64 = row.get(0).ok()?;
        Some(total as u64)
    }

    /// Insert a new ledger row. Best-effort; errors are reported to stderr via eprintln
    /// but never propagated.
    pub async fn record_savings(
        &self,
        project_path: &str,
        tool_name: &str,
        before_tokens: u64,
        after_tokens: u64,
        ts: i64,
    ) {
        let project_path = normalize_project_key(project_path);
        let result = self
            .conn
            .execute(
                "INSERT INTO savings_ledger (ts, project_path, tool_name, before_tokens, after_tokens) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![ts, project_path, tool_name, before_tokens as i64, after_tokens as i64],
            )
            .await;
        if let Err(e) = result {
            eprintln!("[tokensave] savings_ledger insert failed: {e}");
        }
    }

    /// Sum (before-after) across the ledger entries, with `ts >= since`. Optionally
    /// filter by exact project path. Returns zeros on any DB error.
    pub async fn sum_savings(&self, project: Option<&str>, since: i64) -> SavingsTotal {
        let sql_with_project =
            "SELECT COALESCE(SUM(CASE WHEN before_tokens > after_tokens THEN before_tokens - after_tokens ELSE 0 END), 0), COUNT(*) \
             FROM savings_ledger WHERE project_path = ?1 AND ts >= ?2";
        let sql_all =
            "SELECT COALESCE(SUM(CASE WHEN before_tokens > after_tokens THEN before_tokens - after_tokens ELSE 0 END), 0), COUNT(*) \
             FROM savings_ledger WHERE ts >= ?1";

        let rows = match project.map(normalize_project_key) {
            Some(p) => self.conn.query(sql_with_project, params![p, since]).await,
            None => self.conn.query(sql_all, params![since]).await,
        };
        let Ok(mut rows) = rows else {
            return SavingsTotal {
                saved_tokens: 0,
                calls: 0,
            };
        };
        match rows.next().await {
            Ok(Some(row)) => SavingsTotal {
                saved_tokens: row.get::<i64>(0).unwrap_or(0).max(0) as u64,
                calls: row.get::<i64>(1).unwrap_or(0).max(0) as u64,
            },
            _ => SavingsTotal {
                saved_tokens: 0,
                calls: 0,
            },
        }
    }

    /// Group ledger entries by UTC calendar day. Newest-first.
    pub async fn savings_history(&self, project: Option<&str>, since: i64) -> Vec<SavingsDay> {
        let sql_with_project =
            "SELECT (ts/86400)*86400 AS day, \
                    COALESCE(SUM(CASE WHEN before_tokens > after_tokens THEN before_tokens - after_tokens ELSE 0 END), 0), \
                    COUNT(*) \
             FROM savings_ledger WHERE project_path = ?1 AND ts >= ?2 \
             GROUP BY day ORDER BY day DESC";
        let sql_all =
            "SELECT (ts/86400)*86400 AS day, \
                    COALESCE(SUM(CASE WHEN before_tokens > after_tokens THEN before_tokens - after_tokens ELSE 0 END), 0), \
                    COUNT(*) \
             FROM savings_ledger WHERE ts >= ?1 \
             GROUP BY day ORDER BY day DESC";

        let rows = match project.map(normalize_project_key) {
            Some(p) => self.conn.query(sql_with_project, params![p, since]).await,
            None => self.conn.query(sql_all, params![since]).await,
        };
        let Ok(mut rows) = rows else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            out.push(SavingsDay {
                day: row.get::<i64>(0).unwrap_or(0),
                saved_tokens: row.get::<i64>(1).unwrap_or(0).max(0) as u64,
                calls: row.get::<i64>(2).unwrap_or(0).max(0) as u64,
            });
        }
        out
    }

    /// Removes a project's row from the global DB. Best-effort.
    pub async fn delete_project(&self, project_path: &Path) {
        let path_str = normalize_project_key(project_path);
        let _ = self
            .conn
            .execute("DELETE FROM projects WHERE path = ?1", params![path_str])
            .await;
    }

    /// Removes many project rows in a single statement. Returns the number of
    /// rows actually deleted (0 on any error). Best-effort.
    ///
    /// Chunks the input at 256 paths per statement to stay well clear of
    /// `SQLite`'s default 999-parameter limit while still reducing N round trips
    /// to ⌈N/256⌉.
    pub async fn delete_projects(&self, project_paths: &[String]) -> usize {
        const CHUNK: usize = 256;
        let mut total: usize = 0;
        for chunk in project_paths.chunks(CHUNK) {
            if chunk.is_empty() {
                continue;
            }
            let placeholders: Vec<&str> = chunk.iter().map(|_| "?").collect();
            let sql = format!(
                "DELETE FROM projects WHERE path IN ({})",
                placeholders.join(",")
            );
            let values: Vec<libsql::Value> = chunk
                .iter()
                .map(|p| libsql::Value::Text(p.clone()))
                .collect();
            if let Ok(n) = self.conn.execute(&sql, values).await {
                total = total.saturating_add(n as usize);
            }
        }
        total
    }

    /// Returns all tracked project paths.
    pub async fn list_project_paths(&self) -> Vec<String> {
        let Ok(mut rows) = self.conn.query("SELECT path FROM projects", ()).await else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            if let Ok(path) = row.get::<String>(0) {
                paths.push(path);
            }
        }
        paths
    }

    // ── Accounting: turns table ──────────────────────────────────────

    /// Insert a parsed turn. Returns `true` if inserted, `false` if duplicate.
    pub async fn insert_turn(&self, turn: &crate::types::CostTurn) -> bool {
        self.insert_turn_row(turn, None, None, None).await
    }

    /// Insert the turn of a Claude Code message from the first of its lines
    /// read, with the calls and Bash signals a later line is merged against
    /// ([`Self::merge_claude_line`]) and no tool result counted yet. Returns
    /// `true` if inserted, `false` if the message is already recorded.
    pub async fn insert_claude_turn(&self, line: &ClaudeLine) -> bool {
        self.insert_turn_row(
            &line.turn,
            Some(line.call_ids()),
            Some(i64::from(line.bash.bits())),
            Some(String::new()),
        )
        .await
    }

    async fn insert_turn_row(
        &self,
        turn: &crate::types::CostTurn,
        tool_use_ids: Option<String>,
        bash_signals: Option<i64>,
        result_ids: Option<String>,
    ) -> bool {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO turns
                 (message_id, project_hash, session_id, model, timestamp,
                  input_tokens, output_tokens, cache_write_tokens, cache_read_tokens,
                  cost_usd, category, tool_names, agent, credits,
                  effort, requested_model, thinking_tokens, thinking_ms,
                  speed, service_tier, git_branch, run_id,
                  tool_use_ids, bash_signals, result_ids)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                         ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
                params![
                    turn.message_id.clone(),
                    turn.project_hash.clone(),
                    turn.session_id.clone(),
                    turn.model.clone(),
                    turn.timestamp as i64,
                    turn.input_tokens as i64,
                    turn.output_tokens as i64,
                    turn.cache_write_tokens as i64,
                    turn.cache_read_tokens as i64,
                    turn.cost_usd,
                    turn.category.clone(),
                    turn.tool_names.clone(),
                    turn.agent.clone(),
                    turn.credits.map(|c| c as i64),
                    turn.detail.effort.clone(),
                    turn.detail.requested_model.clone(),
                    turn.detail.thinking_tokens.map(|t| t as i64),
                    turn.detail.thinking_ms.map(|t| t as i64),
                    turn.detail.speed.clone(),
                    turn.detail.service_tier.clone(),
                    turn.detail.git_branch.clone(),
                    turn.detail.run_id.clone(),
                    tool_use_ids,
                    bash_signals,
                    result_ids,
                ],
            )
            .await
            .is_ok_and(|n| n > 0)
    }

    /// Merges a later line of a recorded Claude Code message into its turn,
    /// as [`merge_line`] describes. Returns `true` when the row changed.
    ///
    /// The row is read first without a lock, since most later lines add
    /// nothing. When one does, it is read again and written in one
    /// `BEGIN IMMEDIATE` transaction: ingests run in several processes at once
    /// (the Stop hook, `tokensave cost`, the monitor), and two merging
    /// different lines of a message from the same read would lose one. Every
    /// ingest caller opens its own `GlobalDb`, so no other statement runs on
    /// this connection inside the transaction.
    pub async fn merge_claude_line(&self, line: &ClaudeLine) -> bool {
        let message_id = line.turn.message_id.as_str();
        let adds = Self::turn_state(&self.conn, message_id)
            .await
            .and_then(|stored| merge_line(&stored, line))
            .is_some();
        if !adds {
            return false;
        }
        let Ok(tx) = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await
        else {
            return false;
        };
        let merged = Self::turn_state(&tx, message_id)
            .await
            .and_then(|stored| merge_line(&stored, line));
        let written = match merged {
            Some(next) => Self::write_turn_state(&tx, message_id, &next).await,
            None => false,
        };
        if written {
            tx.commit().await.is_ok()
        } else {
            // Rolled back here so the outcome is not left to `Drop`; if the
            // ROLLBACK itself fails, libsql's `Drop` panics either way.
            let _ = tx.rollback().await;
            false
        }
    }

    /// The [`TurnState`] of a recorded turn, or `None` for an unknown
    /// `message_id` or a row that cannot be read.
    async fn turn_state(conn: &Connection, message_id: &str) -> Option<TurnState> {
        let mut rows = conn
            .query(
                "SELECT model, input_tokens, output_tokens, cache_write_tokens,
                        cache_read_tokens, cost_usd, category, tool_names,
                        tool_use_ids, bash_signals, thinking_tokens, speed, service_tier
                 FROM turns WHERE message_id = ?1",
                params![message_id.to_string()],
            )
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        let count = |index| {
            row.get::<i64>(index)
                .ok()
                .and_then(|value| u64::try_from(value).ok())
        };
        Some(TurnState {
            model: row.get(0).ok()?,
            input_tokens: count(1)?,
            output_tokens: count(2)?,
            cache_write_tokens: count(3)?,
            cache_read_tokens: count(4)?,
            cost_usd: row.get(5).ok()?,
            category: row.get(6).ok()?,
            tool_names: row.get(7).ok()?,
            tool_use_ids: row.get(8).ok(),
            bash_signals: row
                .get::<i64>(9)
                .ok()
                .and_then(|bits| u8::try_from(bits).ok())
                .map(BashSignals::from_bits),
            thinking_tokens: count(10),
            speed: row.get(11).ok(),
            service_tier: row.get(12).ok(),
        })
    }

    async fn write_turn_state(conn: &Connection, message_id: &str, state: &TurnState) -> bool {
        conn.execute(
            "UPDATE turns SET
                 input_tokens = ?2, output_tokens = ?3,
                 cache_write_tokens = ?4, cache_read_tokens = ?5,
                 cost_usd = ?6, category = ?7, tool_names = ?8,
                 tool_use_ids = ?9, bash_signals = ?10,
                 thinking_tokens = ?11, speed = ?12, service_tier = ?13
             WHERE message_id = ?1",
            params![
                message_id.to_string(),
                state.input_tokens as i64,
                state.output_tokens as i64,
                state.cache_write_tokens as i64,
                state.cache_read_tokens as i64,
                state.cost_usd,
                state.category.clone(),
                state.tool_names.clone(),
                state.tool_use_ids.clone(),
                state.bash_signals.map(|bash| i64::from(bash.bits())),
                state.thinking_tokens.map(|t| t as i64),
                state.speed.clone(),
                state.service_tier.clone(),
            ],
        )
        .await
        .is_ok_and(|n| n > 0)
    }

    /// The recorded [`crate::types::TurnDetail`] of a turn, or `None` for an
    /// unknown `message_id`.
    pub async fn turn_detail(&self, message_id: &str) -> Option<crate::types::TurnDetail> {
        let mut rows = self
            .conn
            .query(
                "SELECT effort, requested_model, thinking_tokens, thinking_ms,
                        speed, service_tier, git_branch, run_id
                 FROM turns WHERE message_id = ?1",
                params![message_id.to_string()],
            )
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        Some(crate::types::TurnDetail {
            effort: row.get(0).ok(),
            requested_model: row.get(1).ok(),
            thinking_tokens: row.get::<i64>(2).ok().map(|t| t as u64),
            thinking_ms: row.get::<i64>(3).ok().map(|t| t as u64),
            speed: row.get(4).ok(),
            service_tier: row.get(5).ok(),
            git_branch: row.get(6).ok(),
            run_id: row.get(7).ok(),
        })
    }

    /// The [`crate::types::TurnDetail`] of every Claude Code turn, by message
    /// id, or `None` when the turns cannot be read.
    pub async fn claude_turn_details(
        &self,
    ) -> Option<std::collections::HashMap<String, crate::types::TurnDetail>> {
        let mut rows = self
            .conn
            .query(
                "SELECT message_id, effort, requested_model, thinking_tokens, thinking_ms,
                        speed, service_tier, git_branch, run_id
                 FROM turns WHERE agent = 'claude'",
                (),
            )
            .await
            .ok()?;
        let mut details = std::collections::HashMap::new();
        while let Some(row) = rows.next().await.ok()? {
            let count = |index| {
                row.get::<i64>(index)
                    .ok()
                    .and_then(|value| u64::try_from(value).ok())
            };
            let detail = crate::types::TurnDetail {
                effort: row.get(1).ok(),
                requested_model: row.get(2).ok(),
                thinking_tokens: count(3),
                thinking_ms: count(4),
                speed: row.get(5).ok(),
                service_tier: row.get(6).ok(),
                git_branch: row.get(7).ok(),
                run_id: row.get(8).ok(),
            };
            details.insert(row.get::<String>(0).ok()?, detail);
        }
        Some(details)
    }

    /// Fills the `NULL` detail columns of recorded turns from `details`,
    /// never replacing a stored value, in transactions of
    /// [`FILL_BATCH_ROWS`] rows. Returns the rows changed, or `None` when a
    /// write failed; the batches committed before it stay.
    pub async fn fill_turn_details(
        &self,
        details: &[(String, crate::types::TurnDetail)],
    ) -> Option<u64> {
        let mut changed = 0;
        for batch in details.chunks(FILL_BATCH_ROWS) {
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .await
                .ok()?;
            let mut written = Some(0);
            for (message_id, detail) in batch {
                let count = |value: Option<u64>| value.and_then(|v| i64::try_from(v).ok());
                // The WHERE names the columns this fills, so a row whose
                // NULLs have no value to take is not counted as changed.
                let Ok(rows) = tx
                    .execute(
                        "UPDATE turns SET
                             effort = COALESCE(effort, ?2),
                             requested_model = COALESCE(requested_model, ?3),
                             thinking_tokens = COALESCE(thinking_tokens, ?4),
                             thinking_ms = COALESCE(thinking_ms, ?5),
                             speed = COALESCE(speed, ?6),
                             service_tier = COALESCE(service_tier, ?7),
                             git_branch = COALESCE(git_branch, ?8),
                             run_id = COALESCE(run_id, ?9)
                         WHERE message_id = ?1
                           AND ((effort IS NULL AND ?2 IS NOT NULL)
                             OR (requested_model IS NULL AND ?3 IS NOT NULL)
                             OR (thinking_tokens IS NULL AND ?4 IS NOT NULL)
                             OR (thinking_ms IS NULL AND ?5 IS NOT NULL)
                             OR (speed IS NULL AND ?6 IS NOT NULL)
                             OR (service_tier IS NULL AND ?7 IS NOT NULL)
                             OR (git_branch IS NULL AND ?8 IS NOT NULL)
                             OR (run_id IS NULL AND ?9 IS NOT NULL))",
                        params![
                            message_id.as_str(),
                            detail.effort.clone(),
                            detail.requested_model.clone(),
                            count(detail.thinking_tokens),
                            count(detail.thinking_ms),
                            detail.speed.clone(),
                            detail.service_tier.clone(),
                            detail.git_branch.clone(),
                            detail.run_id.clone()
                        ],
                    )
                    .await
                else {
                    written = None;
                    break;
                };
                written = written.map(|n| n + rows);
            }
            let Some(rows) = written else {
                // Rolled back here so the outcome is not left to `Drop`; if
                // the ROLLBACK itself fails, libsql's `Drop` panics either way.
                let _ = tx.rollback().await;
                return None;
            };
            tx.commit().await.ok()?;
            changed += rows;
        }
        Some(changed)
    }

    /// Records a subagent run, replacing the stored row: its `.meta.json` is
    /// rewritten as the run goes on (`finished` arrives at the end), and the
    /// file's latest state is the run's.
    pub async fn upsert_agent_run(&self, run: &crate::types::AgentRun) -> bool {
        self.conn
            .execute(
                "INSERT INTO agent_runs
                 (run_id, session_id, project_hash, role, description, spawn_depth,
                  parent_run_id, tool_use_id, model, effort, finished, worktree_branch)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(run_id) DO UPDATE SET
                     session_id      = excluded.session_id,
                     project_hash    = excluded.project_hash,
                     role            = excluded.role,
                     description     = excluded.description,
                     spawn_depth     = excluded.spawn_depth,
                     parent_run_id   = excluded.parent_run_id,
                     tool_use_id     = excluded.tool_use_id,
                     model           = excluded.model,
                     effort          = excluded.effort,
                     finished        = excluded.finished,
                     worktree_branch = excluded.worktree_branch",
                params![
                    run.run_id.clone(),
                    run.session_id.clone(),
                    run.project_hash.clone(),
                    run.role.clone(),
                    run.description.clone(),
                    run.spawn_depth.map(|d| d as i64),
                    run.parent_run_id.clone(),
                    run.tool_use_id.clone(),
                    run.model.clone(),
                    run.effort.clone(),
                    run.finished.map(i64::from),
                    run.worktree_branch.clone(),
                ],
            )
            .await
            .is_ok_and(|n| n > 0)
    }

    /// The recorded subagent run with this id, if any.
    pub async fn agent_run(&self, run_id: &str) -> Option<crate::types::AgentRun> {
        let mut rows = self
            .conn
            .query(
                "SELECT run_id, session_id, project_hash, role, description, spawn_depth,
                        parent_run_id, tool_use_id, model, effort, finished, worktree_branch
                 FROM agent_runs WHERE run_id = ?1",
                params![run_id.to_string()],
            )
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        Some(crate::types::AgentRun {
            run_id: row.get(0).ok()?,
            session_id: row.get(1).ok()?,
            project_hash: row.get(2).ok()?,
            role: row.get(3).ok()?,
            description: row.get(4).ok(),
            spawn_depth: row.get::<i64>(5).ok().map(|d| d as u64),
            parent_run_id: row.get(6).ok(),
            tool_use_id: row.get(7).ok(),
            model: row.get(8).ok(),
            effort: row.get(9).ok(),
            finished: row.get::<i64>(10).ok().map(|f| f != 0),
            worktree_branch: row.get(11).ok(),
        })
    }

    /// Adds the `tokens` of the result of call `tool_use_id` to a turn's
    /// recorded tool-result size, once. Returns `true` when it was added.
    ///
    /// Tool results are not in the assistant message that requested them: they
    /// arrive in the following user message, one block per `tool_use_id`, so
    /// the size is known only after the turn has been inserted and is added to
    /// it here (#474). Accumulating rather than assigning is deliberate — a
    /// turn may issue several tools, and their results can be split across
    /// more than one user message.
    ///
    /// The call's id is recorded in `result_ids` by the same statement, and a
    /// result whose id is recorded adds nothing, so a transcript read again
    /// does not count its results twice (inkcast #255). A turn recorded before
    /// `result_ids` (`NULL`) is added to only while its size is 0: no result
    /// has been counted for it then; once one has, which ones is unknown. An
    /// id that is empty or holds a comma could not be found in the
    /// comma-joined list, so its result is not counted (none in 80,623 real
    /// ids, 2026-10-27).
    ///
    /// A no-op for an unknown `message_id`, which is what a resumed parse sees
    /// when the assistant line fell before the stored offset and its results
    /// after.
    pub async fn add_tool_result_tokens(
        &self,
        message_id: &str,
        tool_use_id: &str,
        tokens: u64,
    ) -> bool {
        if tokens == 0 || tool_use_id.is_empty() || tool_use_id.contains(',') {
            return false;
        }
        self.conn
            .execute(
                "UPDATE turns SET
                     tool_result_tokens = tool_result_tokens + ?3,
                     result_ids = CASE WHEN COALESCE(result_ids, '') = '' THEN ?2
                                       ELSE result_ids || ',' || ?2 END
                 WHERE message_id = ?1
                   AND (result_ids IS NOT NULL OR tool_result_tokens = 0)
                   AND instr(',' || COALESCE(result_ids, '') || ',', ',' || ?2 || ',') = 0",
                params![
                    message_id.to_string(),
                    tool_use_id.to_string(),
                    tokens as i64
                ],
            )
            .await
            .is_ok_and(|n| n > 0)
    }

    /// Upsert a Droid cumulative-snapshot turn (monotonic semantics).
    ///
    /// On conflict, the row is updated only when **every** token counter in the
    /// candidate is ≥ the stored value, credits do not regress, and **at least
    /// one** counter or credit strictly increases.  `timestamp` is never
    /// updated (it is the stable session-start bucket).
    ///
    /// Returns `true` if a row was inserted or updated, `false` otherwise.
    pub async fn upsert_droid_turn(&self, turn: &crate::types::CostTurn) -> bool {
        self.conn
            .execute(
                "INSERT INTO turns
                 (message_id, project_hash, session_id, model, timestamp,
                  input_tokens, output_tokens, cache_write_tokens, cache_read_tokens,
                  cost_usd, category, tool_names, agent, credits)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(message_id) DO UPDATE SET
                     project_hash        = excluded.project_hash,
                     session_id          = excluded.session_id,
                     model               = excluded.model,
                     input_tokens        = MAX(excluded.input_tokens,        turns.input_tokens),
                     output_tokens       = MAX(excluded.output_tokens,       turns.output_tokens),
                     cache_write_tokens  = MAX(excluded.cache_write_tokens,  turns.cache_write_tokens),
                     cache_read_tokens   = MAX(excluded.cache_read_tokens,   turns.cache_read_tokens),
                     cost_usd            = excluded.cost_usd,
                     category            = excluded.category,
                     tool_names          = excluded.tool_names,
                     agent               = excluded.agent,
                     credits             = COALESCE(excluded.credits, turns.credits)
                 WHERE
                     excluded.input_tokens        >= turns.input_tokens
                     AND excluded.output_tokens       >= turns.output_tokens
                     AND excluded.cache_write_tokens  >= turns.cache_write_tokens
                     AND excluded.cache_read_tokens   >= turns.cache_read_tokens
                     AND (excluded.credits IS NULL
                          OR turns.credits IS NULL
                          OR excluded.credits >= turns.credits)
                     AND (
                         excluded.input_tokens        > turns.input_tokens
                         OR excluded.output_tokens       > turns.output_tokens
                         OR excluded.cache_write_tokens  > turns.cache_write_tokens
                         OR excluded.cache_read_tokens   > turns.cache_read_tokens
                         OR (excluded.credits IS NOT NULL
                             AND (turns.credits IS NULL
                                  OR excluded.credits > turns.credits))
                     )",
                params![
                    turn.message_id.clone(),
                    turn.project_hash.clone(),
                    turn.session_id.clone(),
                    turn.model.clone(),
                    turn.timestamp as i64,
                    turn.input_tokens as i64,
                    turn.output_tokens as i64,
                    turn.cache_write_tokens as i64,
                    turn.cache_read_tokens as i64,
                    turn.cost_usd,
                    turn.category.clone(),
                    turn.tool_names.clone(),
                    turn.agent.clone(),
                    turn.credits.map(|c| c as i64),
                ],
            )
            .await
            .is_ok_and(|n| n > 0)
    }

    /// Total cost in USD since a given unix timestamp.
    pub async fn total_cost_since(&self, since: u64) -> Option<f64> {
        let mut rows = self
            .conn
            .query(
                "SELECT COALESCE(SUM(cost_usd), 0.0) FROM turns WHERE timestamp >= ?1",
                params![since as i64],
            )
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        Some(row.get::<f64>(0).unwrap_or(0.0))
    }

    /// Total input + output tokens since a given unix timestamp.
    pub async fn total_tokens_since(&self, since: u64) -> Option<u64> {
        let mut rows = self
            .conn
            .query(
                "SELECT COALESCE(SUM(input_tokens + output_tokens), 0) FROM turns WHERE timestamp >= ?1",
                params![since as i64],
            )
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        Some(row.get::<i64>(0).unwrap_or(0) as u64)
    }

    /// Token breakdown (input, output, `cache_read`) since a given timestamp.
    /// Claude-only: Droid turns are excluded so legacy USD views remain accurate.
    pub async fn token_breakdown_since(&self, since: u64) -> Option<(u64, u64, u64, u64)> {
        let mut rows = self
            .conn
            .query(
                "SELECT COALESCE(SUM(input_tokens), 0),
                        COALESCE(SUM(output_tokens), 0),
                        COALESCE(SUM(cache_read_tokens), 0),
                        COALESCE(SUM(cache_write_tokens), 0)
                 FROM turns WHERE timestamp >= ?1 AND agent = 'claude'",
                params![since as i64],
            )
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        Some((
            row.get::<i64>(0).unwrap_or(0) as u64,
            row.get::<i64>(1).unwrap_or(0) as u64,
            row.get::<i64>(2).unwrap_or(0) as u64,
            row.get::<i64>(3).unwrap_or(0) as u64,
        ))
    }

    /// Cost grouped by model since a given timestamp. Claude-only.
    /// Returns `(model, cost, total_tokens)`.
    ///
    /// `total_tokens` counts every category the cost was computed from, cache
    /// reads and cache writes included (#472). Summing only uncached input and
    /// output implied a price per million several times any published rate,
    /// because agent traffic is dominated by the cached context resent each
    /// turn — priced, but previously uncounted.
    pub async fn cost_by_model_since(&self, since: u64) -> Vec<(String, f64, u64)> {
        let Ok(mut rows) = self
            .conn
            .query(
                "SELECT model, SUM(cost_usd),
                        SUM(input_tokens + output_tokens + cache_read_tokens + cache_write_tokens)
                 FROM turns WHERE timestamp >= ?1 AND agent = 'claude'
                 GROUP BY model ORDER BY SUM(cost_usd) DESC",
                params![since as i64],
            )
            .await
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            let model: String = row.get(0).unwrap_or_default();
            let cost: f64 = row.get(1).unwrap_or(0.0);
            let tokens: i64 = row.get(2).unwrap_or(0);
            out.push((model, cost, tokens as u64));
        }
        out
    }

    /// Cost grouped by category since a given timestamp. Claude-only.
    /// Returns `(category, cost, turn_count)`.
    pub async fn cost_by_category_since(&self, since: u64) -> Vec<(String, f64, u64)> {
        let Ok(mut rows) = self
            .conn
            .query(
                "SELECT category, SUM(cost_usd), COUNT(*)
                 FROM turns WHERE timestamp >= ?1 AND agent = 'claude'
                 GROUP BY category ORDER BY SUM(cost_usd) DESC",
                params![since as i64],
            )
            .await
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            let cat: String = row.get(0).unwrap_or_default();
            let cost: f64 = row.get(1).unwrap_or(0.0);
            let count: i64 = row.get(2).unwrap_or(0);
            out.push((cat, cost, count as u64));
        }
        out
    }

    /// Fetch `(tool_names, tool_result_tokens)` for every Claude turn since a
    /// timestamp.
    ///
    /// The second element is the size of the tool *results* the turn's tools
    /// injected — the text a graph query could have served instead — and not
    /// `input_tokens`, which is what this read before (#474). Under prompt
    /// caching `input_tokens` is only the uncached remainder of the prompt, a
    /// double-digit figure per turn with nothing to do with what a `Read` or a
    /// `Grep` put into the conversation. The whole prompt, cache included, is
    /// not addressable either: most of it is conversation the navigation did
    /// not cause.
    ///
    /// Claude-only: the discover analyzer works on Claude Code navigation patterns.
    /// Returns an empty vector on any DB error.
    pub async fn nav_turns_since(&self, since: u64) -> Vec<(String, u64)> {
        let Ok(mut rows) = self
            .conn
            .query(
                "SELECT tool_names, tool_result_tokens FROM turns \
                 WHERE timestamp >= ?1 AND agent = 'claude'",
                params![since as i64],
            )
            .await
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            let tool_names: String = row.get(0).unwrap_or_default();
            let input_tokens: i64 = row.get(1).unwrap_or(0);
            out.push((tool_names, input_tokens.max(0) as u64));
        }
        out
    }

    /// Cost grouped by agent since a given timestamp.
    ///
    /// Credits aggregate to `None` if any row in the group has a NULL credit value.
    pub async fn cost_by_agent_since(&self, since: u64) -> Vec<AgentCostSummary> {
        let Ok(mut rows) = self
            .conn
            .query(
                "SELECT agent,
                        COALESCE(SUM(cost_usd), 0.0),
                        COALESCE(SUM(input_tokens), 0),
                        COALESCE(SUM(output_tokens), 0),
                        COALESCE(SUM(cache_write_tokens), 0),
                        COALESCE(SUM(cache_read_tokens), 0),
                        CASE WHEN COUNT(credits) = COUNT(*) THEN SUM(credits) ELSE NULL END,
                        COUNT(*)
                 FROM turns WHERE timestamp >= ?1
                 GROUP BY agent ORDER BY agent",
                params![since as i64],
            )
            .await
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        while let Ok(Some(row)) = rows.next().await {
            let agent: String = row.get(0).unwrap_or_default();
            let cost_usd: f64 = row.get(1).unwrap_or(0.0);
            let input_tokens: i64 = row.get(2).unwrap_or(0);
            let output_tokens: i64 = row.get(3).unwrap_or(0);
            let cache_write_tokens: i64 = row.get(4).unwrap_or(0);
            let cache_read_tokens: i64 = row.get(5).unwrap_or(0);
            let credits: Option<i64> = row.get(6).ok();
            let turns: i64 = row.get(7).unwrap_or(0);
            out.push(AgentCostSummary {
                agent,
                cost_usd,
                input_tokens: input_tokens.max(0) as u64,
                output_tokens: output_tokens.max(0) as u64,
                cache_write_tokens: cache_write_tokens.max(0) as u64,
                cache_read_tokens: cache_read_tokens.max(0) as u64,
                credits: credits.map(|c| c.max(0) as u64),
                turns: turns.max(0) as u64,
            });
        }
        out
    }

    // ── Accounting: parse_offsets table ────────────────────────────────

    /// Get the saved parse offset for a JSONL file.
    /// Returns `(byte_offset, mtime)` or `None` if not tracked.
    pub async fn get_parse_offset(&self, path: &str) -> Option<(u64, u64)> {
        let mut rows = self
            .conn
            .query(
                "SELECT byte_offset, mtime FROM parse_offsets WHERE file_path = ?1",
                params![path],
            )
            .await
            .ok()?;
        let row = rows.next().await.ok()??;
        let offset: i64 = row.get(0).ok()?;
        let mtime: i64 = row.get(1).ok()?;
        Some((offset as u64, mtime as u64))
    }

    /// Save the parse offset for a JSONL file. Best-effort.
    pub async fn set_parse_offset(&self, path: &str, offset: u64, mtime: u64) {
        let _ = self
            .conn
            .execute(
                "INSERT INTO parse_offsets (file_path, byte_offset, mtime) VALUES (?1, ?2, ?3)
                 ON CONFLICT(file_path) DO UPDATE SET byte_offset = ?2, mtime = ?3",
                params![path, offset as i64, mtime as i64],
            )
            .await;
    }

    /// Checkpoints the WAL. Best-effort.
    pub async fn checkpoint(&self) {
        let _ = self
            .conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .await;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn dot_resolves_to_the_same_key_as_the_absolute_path() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(normalize_project_key("."), normalize_project_key(&cwd));
        assert_eq!(normalize_project_key("./"), normalize_project_key(&cwd));
    }

    #[test]
    fn parent_relative_path_resolves_to_the_parent_directory() {
        let cwd = std::env::current_dir().unwrap();
        let parent = cwd.parent().unwrap();
        assert_eq!(normalize_project_key(".."), normalize_project_key(parent));
    }

    #[test]
    fn trailing_separators_do_not_create_a_second_key() {
        let cwd = std::env::current_dir().unwrap();
        let with_slash = format!("{}/", cwd.display());
        assert_eq!(
            normalize_project_key(&with_slash),
            normalize_project_key(&cwd)
        );
    }

    #[test]
    fn drive_letter_case_is_folded() {
        assert_eq!(
            normalize_key_string(r"d:\Work\ProjectA"),
            normalize_key_string(r"D:\Work\ProjectA")
        );
        assert_eq!(
            normalize_key_string(r"d:\Work\ProjectA"),
            r"D:\Work\ProjectA"
        );
    }

    #[test]
    fn windows_verbatim_prefixes_are_stripped() {
        assert_eq!(normalize_key_string(r"\\?\D:\Work\P"), r"D:\Work\P");
        assert_eq!(normalize_key_string(r"\\?\UNC\srv\share"), r"\\srv\share");
    }

    #[test]
    fn a_root_is_not_trimmed_away() {
        let mut unix_root = "/".to_string();
        trim_trailing_separators(&mut unix_root);
        assert_eq!(unix_root, "/");

        let mut drive_root = r"D:\".to_string();
        trim_trailing_separators(&mut drive_root);
        assert_eq!(drive_root, r"D:\");
    }

    #[test]
    fn unresolvable_absolute_paths_keep_a_stable_key() {
        // A deleted project must still map to one key, not to the cwd.
        let key = normalize_project_key("/definitely/not/here/../here");
        assert_eq!(key, normalize_project_key("/definitely/not/here"));
    }

    #[tokio::test]
    async fn relative_and_absolute_writes_share_one_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = GlobalDb::open_at(&dir.path().join("global.db"))
            .await
            .unwrap();
        let project = dir.path().join("proj");
        std::fs::create_dir(&project).unwrap();

        db.upsert(&project, 100).await;
        db.upsert(&project.join("."), 250).await;

        assert_eq!(db.list_project_paths().await.len(), 1);
        assert_eq!(db.get_project_tokens(&project).await, 250);
    }

    #[tokio::test]
    async fn migration_merges_duplicates_and_drops_relative_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("global.db");
        let project = dir.path().join("proj");
        std::fs::create_dir(&project).unwrap();
        let canonical = normalize_project_key(&project);
        let trailing = format!("{canonical}/");

        {
            // Simulate a pre-canonicalisation DB: one duplicate pair and a `.` row.
            let db = GlobalDb::open_at(&db_path).await.unwrap();
            db.conn
                .execute_batch(
                    "DELETE FROM meta WHERE key = 'projects_path_normalized';
                     DELETE FROM projects;",
                )
                .await
                .unwrap();
            for (path, tokens) in [
                (canonical.as_str(), 14_000_i64),
                (trailing.as_str(), 12_000),
                (".", 999),
            ] {
                db.conn
                    .execute(
                        "INSERT INTO projects (path, tokens_saved) VALUES (?1, ?2)",
                        params![path, tokens],
                    )
                    .await
                    .unwrap();
            }
            // Raw insert: record_savings would canonicalise it for us.
            db.conn
                .execute(
                    "INSERT INTO savings_ledger (ts, project_path, tool_name, before_tokens, after_tokens) \
                     VALUES (42, ?1, 'tokensave_context', 1000, 100)",
                    params![trailing.as_str()],
                )
                .await
                .unwrap();
        }

        let db = GlobalDb::open_at(&db_path).await.unwrap();
        let paths = db.list_project_paths().await;
        assert_eq!(paths, vec![canonical.clone()]);
        assert_eq!(db.get_project_tokens(&project).await, 26_000);
        // The ledger row written under the old spelling still answers for the project.
        assert_eq!(db.sum_savings(Some(&canonical), 0).await.calls, 1);
    }

    /// Processes that open the database at the same moment (the Stop hook,
    /// the monitor, `tokensave cost`) all get it.
    #[test]
    fn concurrent_opens_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("global.db");
        let runtime = || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
        };
        assert!(runtime().block_on(GlobalDb::open_at(&path)).is_some());
        let mut failed = 0;
        for _ in 0..20 {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            let opens: Vec<_> = (0..8)
                .map(|_| {
                    let (path, barrier) = (path.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        let rt = runtime();
                        barrier.wait();
                        rt.block_on(GlobalDb::open_at(&path)).is_none()
                    })
                })
                .collect();
            failed += opens
                .into_iter()
                .map(|open| open.join().unwrap())
                .filter(|&none| none)
                .count();
        }
        assert_eq!(failed, 0, "opens that returned no database, of 160");
    }

    /// Processes opening a database that does not exist yet, at the same
    /// moment, all get it: the first switch of the new file to WAL fails at
    /// once for the others, past the busy timeout. The schema is created once
    /// (inkcast #248).
    #[test]
    fn concurrent_first_opens_all_succeed() {
        let runtime = || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
        };
        for _ in 0..10 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("global.db");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            let opens: Vec<_> = (0..8)
                .map(|_| {
                    let (path, barrier) = (path.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        let rt = runtime();
                        barrier.wait();
                        rt.block_on(GlobalDb::open_at(&path))
                            .map(|db| db.newer_schema())
                    })
                })
                .collect();
            for open in opens {
                assert_eq!(open.join().unwrap(), Some(None), "every open succeeds");
            }
            let version = runtime().block_on(schema_version_at(&path));
            assert_eq!(version, Some(SCHEMA_VERSION));
        }
    }
}
