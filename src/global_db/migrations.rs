//! Numbered schema migrations for `global.db` (inkcast #248).
//!
//! The database's version is SQLite's `PRAGMA user_version`, which SQLite
//! reserves for the application and never sets itself. Each migration runs
//! once, in its own `BEGIN IMMEDIATE` transaction that also sets the version
//! it reaches, so a failed migration leaves the database as it was, and two
//! processes opening an old database apply it once. A shipped migration is
//! never edited: a schema change is a new migration at the end of
//! [`MIGRATIONS`].

use std::fmt;

use libsql::{Connection, TransactionBehavior};

/// One step of a migration.
pub(super) enum Step {
    /// Statements run as one batch.
    Batch(&'static str),
    /// Columns added to `table` where it lacks them (a table created before
    /// them), each with the statement that adds it. Names compare as SQLite
    /// compares them, ignoring ASCII case.
    AddColumns {
        table: &'static str,
        columns: &'static [(&'static str, &'static str)],
    },
}

pub(super) struct Migration {
    /// The version the database is at once this migration is applied.
    pub(super) version: u32,
    pub(super) steps: &'static [Step],
}

/// Every migration, in order.
pub(super) const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    steps: V1,
}];

/// The schema version this binary writes.
pub(super) const LATEST: u32 = latest(MIGRATIONS);

const fn latest(migrations: &[Migration]) -> u32 {
    migrations[migrations.len() - 1].version
}

/// Version 1: the schema before versions were kept, reached from any earlier
/// shape of it, as the per-open checks that preceded this runner did.
const V1: &[Step] = &[
    Step::Batch(
        "CREATE TABLE IF NOT EXISTS projects (
            path TEXT PRIMARY KEY,
            tokens_saved INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS turns (
            message_id TEXT PRIMARY KEY,
            project_hash TEXT NOT NULL,
            session_id TEXT NOT NULL,
            model TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            cache_write_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cost_usd REAL NOT NULL,
            category TEXT NOT NULL,
            tool_names TEXT NOT NULL DEFAULT '',
            agent TEXT NOT NULL DEFAULT 'claude',
            credits INTEGER,
            tool_result_tokens INTEGER NOT NULL DEFAULT 0,
            effort TEXT,
            requested_model TEXT,
            thinking_tokens INTEGER,
            thinking_ms INTEGER,
            speed TEXT,
            service_tier TEXT,
            git_branch TEXT,
            run_id TEXT,
            tool_use_ids TEXT,
            bash_signals INTEGER,
            result_ids TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_turns_timestamp ON turns(timestamp);
        CREATE INDEX IF NOT EXISTS idx_turns_project ON turns(project_hash);
        CREATE INDEX IF NOT EXISTS idx_turns_model ON turns(model);
        CREATE TABLE IF NOT EXISTS agent_runs (
            run_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            project_hash TEXT NOT NULL,
            role TEXT NOT NULL,
            description TEXT,
            spawn_depth INTEGER,
            parent_run_id TEXT,
            tool_use_id TEXT,
            model TEXT,
            effort TEXT,
            finished INTEGER,
            worktree_branch TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_agent_runs_session ON agent_runs(session_id);
        CREATE TABLE IF NOT EXISTS parse_offsets (
            file_path TEXT PRIMARY KEY,
            byte_offset INTEGER NOT NULL,
            mtime INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS savings_ledger (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts INTEGER NOT NULL,
            project_path TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            before_tokens INTEGER NOT NULL,
            after_tokens INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_savings_ledger_ts ON savings_ledger(ts);
        CREATE INDEX IF NOT EXISTS idx_savings_ledger_project ON savings_ledger(project_path);
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
    ),
    // `turns` columns added after the table was first shipped. Rows parsed
    // before them keep the default: `tool_result_tokens` 0 rather than a
    // recomputed figure, since it comes from transcript lines this database
    // does not keep (#474); the turn detail and the merge columns NULL, for
    // an unknown value (#246, #250, #255).
    Step::AddColumns {
        table: "turns",
        columns: &[
            (
                "agent",
                "ALTER TABLE turns ADD COLUMN agent TEXT NOT NULL DEFAULT 'claude'",
            ),
            ("credits", "ALTER TABLE turns ADD COLUMN credits INTEGER"),
            (
                "tool_result_tokens",
                "ALTER TABLE turns ADD COLUMN tool_result_tokens INTEGER NOT NULL DEFAULT 0",
            ),
            ("effort", "ALTER TABLE turns ADD COLUMN effort TEXT"),
            (
                "requested_model",
                "ALTER TABLE turns ADD COLUMN requested_model TEXT",
            ),
            (
                "thinking_tokens",
                "ALTER TABLE turns ADD COLUMN thinking_tokens INTEGER",
            ),
            (
                "thinking_ms",
                "ALTER TABLE turns ADD COLUMN thinking_ms INTEGER",
            ),
            ("speed", "ALTER TABLE turns ADD COLUMN speed TEXT"),
            (
                "service_tier",
                "ALTER TABLE turns ADD COLUMN service_tier TEXT",
            ),
            ("git_branch", "ALTER TABLE turns ADD COLUMN git_branch TEXT"),
            ("run_id", "ALTER TABLE turns ADD COLUMN run_id TEXT"),
            (
                "tool_use_ids",
                "ALTER TABLE turns ADD COLUMN tool_use_ids TEXT",
            ),
            (
                "bash_signals",
                "ALTER TABLE turns ADD COLUMN bash_signals INTEGER",
            ),
            ("result_ids", "ALTER TABLE turns ADD COLUMN result_ids TEXT"),
        ],
    },
    // After the columns: on a table that predates `run_id`, this index in the
    // first batch would fail.
    Step::Batch("CREATE INDEX IF NOT EXISTS idx_turns_run ON turns(run_id)"),
];

/// Where a database's schema stands after [`migrate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    /// At the latest version, migrated if it was older.
    Current,
    /// At this version, newer than the latest known: neither migrated nor to
    /// be written to.
    Newer(u32),
}

/// A schema that could not be brought to the latest version.
#[derive(Debug)]
pub(super) enum MigrationError {
    /// The version could not be read.
    Version(libsql::Error),
    /// The migration to `version` failed and was rolled back.
    Migration { version: u32, source: libsql::Error },
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(source) => write!(f, "reading the schema version: {source}"),
            Self::Migration { version, source } => {
                write!(f, "migrating to schema version {version}: {source}")
            }
        }
    }
}

/// Applies each of `migrations` above the database's version, in order.
pub(super) async fn migrate(
    conn: &Connection,
    migrations: &[Migration],
) -> Result<Outcome, MigrationError> {
    let latest = latest(migrations);
    let found = user_version(conn).await.map_err(MigrationError::Version)?;
    if found > latest {
        return Ok(Outcome::Newer(found));
    }
    for migration in migrations.iter().filter(|m| m.version > found) {
        let applied =
            apply(conn, migration, latest)
                .await
                .map_err(|source| MigrationError::Migration {
                    version: migration.version,
                    source,
                })?;
        if let Some(newer) = applied {
            return Ok(Outcome::Newer(newer));
        }
    }
    Ok(Outcome::Current)
}

/// Applies `migration` and sets its version, in one transaction. Returns the
/// version found instead when another process moved the database past
/// `latest` while this one waited for the lock.
async fn apply(
    conn: &Connection,
    migration: &Migration,
    latest: u32,
) -> libsql::Result<Option<u32>> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .await?;
    let applied: libsql::Result<Option<u32>> = async {
        // Read again under the lock: another process may have migrated
        // while this one waited for it.
        let now = user_version(&tx).await?;
        if now > latest {
            return Ok(Some(now));
        }
        if now >= migration.version {
            return Ok(None);
        }
        for step in migration.steps {
            run(&tx, step).await?;
        }
        tx.execute_batch(&format!("PRAGMA user_version = {}", migration.version))
            .await?;
        Ok(None)
    }
    .await;
    match applied {
        Ok(newer) => {
            tx.commit().await?;
            Ok(newer)
        }
        Err(error) => {
            // Rolled back here so the outcome is not left to `Drop`; if the
            // ROLLBACK itself fails, libsql's `Drop` panics either way.
            let _ = tx.rollback().await;
            Err(error)
        }
    }
}

async fn run(conn: &Connection, step: &Step) -> libsql::Result<()> {
    match step {
        Step::Batch(sql) => {
            conn.execute_batch(sql).await?;
        }
        Step::AddColumns { table, columns } => {
            let present = column_names(conn, table).await?;
            for (column, ddl) in *columns {
                if !present.iter().any(|name| name.eq_ignore_ascii_case(column)) {
                    conn.execute(ddl, ()).await?;
                }
            }
        }
    }
    Ok(())
}

async fn column_names(conn: &Connection, table: &str) -> libsql::Result<Vec<String>> {
    let mut rows = conn
        .query(&format!("PRAGMA table_info({table})"), ())
        .await?;
    let mut names = Vec::new();
    while let Some(row) = rows.next().await? {
        names.push(row.get::<String>(1)?);
    }
    Ok(names)
}

/// The database's schema version. SQLite stores it as a signed 32-bit
/// integer; a negative one was not written by tokensave, and reads as newer
/// than any version, so the database is left alone.
pub(super) async fn user_version(conn: &Connection) -> libsql::Result<u32> {
    let mut rows = conn.query("PRAGMA user_version", ()).await?;
    let version = match rows.next().await? {
        Some(row) => row.get::<i64>(0)?,
        None => 0,
    };
    Ok(u32::try_from(version).unwrap_or(u32::MAX))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    async fn connection() -> (tempfile::TempDir, Connection) {
        let tmp = tempfile::TempDir::new().unwrap();
        let db = libsql::Builder::new_local(tmp.path().join("global.db"))
            .build()
            .await
            .unwrap();
        (tmp, db.connect().unwrap())
    }

    async fn tables(conn: &Connection) -> Vec<String> {
        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
                (),
            )
            .await
            .unwrap();
        let mut names = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            names.push(row.get(0).unwrap());
        }
        names
    }

    const TWO: &[Migration] = &[
        Migration {
            version: 1,
            steps: &[Step::Batch("CREATE TABLE a (x INTEGER)")],
        },
        Migration {
            version: 2,
            steps: &[
                Step::Batch("CREATE TABLE b (x INTEGER)"),
                Step::AddColumns {
                    table: "a",
                    columns: &[("y", "ALTER TABLE a ADD COLUMN y INTEGER")],
                },
                Step::Batch("CREATE TABLE b (x INTEGER)"),
            ],
        },
    ];

    /// A migration that fails is rolled back whole, version included; the
    /// migrations before it stay applied.
    #[tokio::test]
    async fn a_failed_migration_leaves_the_schema_and_version_as_they_were() {
        let (_tmp, conn) = connection().await;

        let error = migrate(&conn, TWO).await.unwrap_err();

        assert!(
            matches!(error, MigrationError::Migration { version: 2, .. }),
            "{error}"
        );
        assert_eq!(user_version(&conn).await.unwrap(), 1);
        assert_eq!(tables(&conn).await, ["a"]);
        assert_eq!(column_names(&conn, "a").await.unwrap(), ["x"]);
    }

    #[tokio::test]
    async fn migrations_run_once_each_in_order() {
        let (_tmp, conn) = connection().await;
        assert_eq!(migrate(&conn, MIGRATIONS).await.unwrap(), Outcome::Current);
        assert_eq!(user_version(&conn).await.unwrap(), LATEST);
        assert_eq!(migrate(&conn, MIGRATIONS).await.unwrap(), Outcome::Current);
        assert_eq!(user_version(&conn).await.unwrap(), LATEST);
    }

    #[tokio::test]
    async fn a_newer_or_foreign_version_is_not_migrated() {
        let (_tmp, conn) = connection().await;
        conn.execute_batch(&format!("PRAGMA user_version = {}", LATEST + 1))
            .await
            .unwrap();
        assert_eq!(
            migrate(&conn, MIGRATIONS).await.unwrap(),
            Outcome::Newer(LATEST + 1)
        );
        assert!(tables(&conn).await.is_empty());

        conn.execute_batch("PRAGMA user_version = -1")
            .await
            .unwrap();
        assert_eq!(
            migrate(&conn, MIGRATIONS).await.unwrap(),
            Outcome::Newer(u32::MAX)
        );
    }

    /// Versions strictly increase, so each migration has one place.
    #[test]
    fn versions_increase_from_one() {
        for (index, migration) in MIGRATIONS.iter().enumerate() {
            assert_eq!(migration.version, index as u32 + 1);
        }
    }
}
