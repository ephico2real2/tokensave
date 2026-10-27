-- global.db as tokensave 7.16.0 creates it: the CREATE statements of
-- GlobalDb::open_at at tag v7.16.0 (src/global_db.rs), and one row in each
-- table, with the flag row its project-path migration writes.
CREATE TABLE IF NOT EXISTS projects (
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
    tool_result_tokens INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_turns_timestamp ON turns(timestamp);
CREATE INDEX IF NOT EXISTS idx_turns_project ON turns(project_hash);
CREATE INDEX IF NOT EXISTS idx_turns_model ON turns(model);
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
);

INSERT INTO projects VALUES ('/work/app', 1234);
INSERT INTO turns VALUES
    ('msg_716', 'proj', 'sess', 'claude-opus-4-6', 1000, 100, 50, 7, 20, 0.05,
     'coding', 'Edit,Read', 'claude', NULL, 30);
INSERT INTO parse_offsets VALUES ('/home/.claude/projects/p/s.jsonl', 4096, 1700000000);
INSERT INTO savings_ledger (ts, project_path, tool_name, before_tokens, after_tokens)
    VALUES (1700000000, '/work/app', 'tokensave_context', 900, 100);
INSERT INTO meta VALUES ('projects_path_normalized', '1');
