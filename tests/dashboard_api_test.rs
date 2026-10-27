//! Contract tests of the dashboard API (inkcast #264): a fixture `global.db`
//! seeded with known turns, runs and tokensave calls, served on a loopback
//! port. Each endpoint's JSON is checked key by key against its TypeScript
//! type in the mockup, and its figures against sums the test computes from the
//! seed itself, never through SQL.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;
use tokensave::dashboard::api::{self, Context};
use tokensave::dashboard::http::{self, Server};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DAY: i64 = 86_400;
/// 2026-10-27, days since 1970-01-01.
const TODAY: i64 = 20_753;
/// 2026-10-27T12:00:00Z.
const NOW: i64 = TODAY * DAY + 12 * 3600;

fn now() -> i64 {
    NOW
}

/// A turn of the seed.
#[derive(Clone, Copy)]
struct Turn {
    id: &'static str,
    repo: Repo,
    session: &'static str,
    model: &'static str,
    /// Days before today.
    days_ago: i64,
    /// Seconds into that day.
    secs: i64,
    input: i64,
    output: i64,
    cache_write: i64,
    cache_read: i64,
    cost: Option<f64>,
    agent: &'static str,
    effort: Option<&'static str>,
    requested: Option<&'static str>,
    thinking: Option<i64>,
    run: Option<&'static str>,
    tools: &'static str,
    tool_result: i64,
}

impl Turn {
    fn ts(&self) -> i64 {
        (TODAY - self.days_ago) * DAY + self.secs
    }

    fn spent(&self) -> i64 {
        self.input + self.output + self.cache_write + self.cache_read
    }

    fn known_agent(&self) -> bool {
        ["claude", "codex", "cursor", "pi", "droid"].contains(&self.agent)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Repo {
    Alpha,
    Beta,
    /// A working folder in no registered repo.
    Elsewhere,
}

const BASE: Turn = Turn {
    id: "",
    repo: Repo::Alpha,
    session: "S1",
    model: "claude-opus-5-5",
    days_ago: 0,
    secs: 0,
    input: 0,
    output: 0,
    cache_write: 0,
    cache_read: 0,
    cost: None,
    agent: "claude",
    effort: None,
    requested: None,
    thinking: None,
    run: None,
    tools: "",
    tool_result: 0,
};

const TURNS: [Turn; 10] = [
    // The main session in alpha, two hours ago.
    Turn {
        id: "t1",
        secs: 10 * 3600,
        input: 100,
        output: 50,
        cache_write: 10,
        cache_read: 1000,
        cost: Some(1.5),
        effort: Some("high"),
        requested: Some("claude-opus-5-5[1m]"),
        thinking: Some(20),
        tools: "Read",
        tool_result: 400,
        ..BASE
    },
    // A reviewer subagent, a minute ago: served by another model than asked.
    Turn {
        id: "t2",
        session: "agent-A1",
        secs: 12 * 3600 - 60,
        input: 200,
        output: 100,
        cache_read: 500,
        cost: Some(2.0),
        effort: Some("xhigh"),
        requested: Some("claude-fable-5-1"),
        thinking: Some(40),
        run: Some("A1"),
        tools: "Grep",
        tool_result: 1000,
        ..BASE
    },
    // An Explore subagent yesterday, on a model with no price.
    Turn {
        id: "t3",
        session: "agent-A2",
        model: "claude-sonnet-5-5",
        days_ago: 1,
        secs: 3600,
        input: 50,
        output: 10,
        effort: Some("medium"),
        run: Some("A2"),
        tools: "Read,Glob",
        tool_result: 60,
        ..BASE
    },
    // A Codex session in beta ten days ago, and a subagent thread in it.
    Turn {
        id: "t4",
        repo: Repo::Beta,
        session: "C1",
        model: "gpt-6.1-sol",
        days_ago: 10,
        secs: 3600,
        input: 300,
        output: 30,
        cache_read: 200,
        cost: Some(0.5),
        agent: "codex",
        effort: Some("high"),
        thinking: Some(5),
        ..BASE
    },
    Turn {
        id: "t5",
        repo: Repo::Beta,
        session: "C1",
        model: "gpt-6.1-sol",
        days_ago: 10,
        secs: 7200,
        input: 100,
        output: 10,
        cost: Some(0.25),
        agent: "codex",
        effort: Some("minimal"),
        run: Some("CT1"),
        ..BASE
    },
    // Outside 30 days, inside 45.
    Turn {
        id: "t6",
        repo: Repo::Elsewhere,
        session: "S9",
        days_ago: 40,
        input: 1000,
        output: 100,
        cost: Some(3.0),
        effort: Some("max"),
        ..BASE
    },
    // Outside 90 days.
    Turn {
        id: "t7",
        days_ago: 100,
        input: 5000,
        output: 500,
        cost: Some(9.0),
        effort: Some("high"),
        ..BASE
    },
    // An agent the dashboard does not show.
    Turn {
        id: "t8",
        secs: 9 * 3600,
        input: 7000,
        output: 700,
        cost: Some(7.0),
        agent: "mystery",
        ..BASE
    },
    // A builder subagent started by the reviewer, yesterday.
    Turn {
        id: "t9",
        session: "agent-A3",
        days_ago: 1,
        secs: 7200,
        input: 40,
        output: 20,
        cache_read: 300,
        cost: Some(0.75),
        effort: Some("high"),
        run: Some("A3"),
        tools: "Edit",
        tool_result: 90,
        ..BASE
    },
    // A priced turn of the Explore run whose other turn (t3) has no price:
    // the run's cost is unknown, not the priced part.
    Turn {
        id: "t10",
        session: "agent-A2",
        model: "claude-sonnet-5-5",
        days_ago: 1,
        secs: 3660,
        input: 5,
        output: 1,
        cost: Some(0.01),
        effort: Some("medium"),
        run: Some("A2"),
        ..BASE
    },
];

/// An `agent_runs` row: `(run_id, session_id, role, description,
/// parent_run_id, finished)`.
type AgentRun = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Option<i64>,
);

const RUNS: [AgentRun; 4] = [
    ("A1", "S1", "ob3", Some("Review PR #1"), None, None),
    ("A2", "S1", "Explore", Some("Find callers"), None, Some(1)),
    ("A3", "S1", "fork", Some("Build S3a"), Some("A1"), None),
    ("CT1", "C1", "worker", None, None, None),
];

/// A tokensave call: `(days_ago, repo, tool, before, after)`.
type Call = (i64, Repo, &'static str, i64, i64);

const CALLS: [Call; 4] = [
    (0, Repo::Alpha, "tokensave_context", 1000, 100),
    (1, Repo::Alpha, "tokensave_search", 500, 200),
    (10, Repo::Beta, "tokensave_context", 300, 400),
    (40, Repo::Alpha, "tokensave_files", 800, 0),
];

/// Seconds into its day a call is recorded at.
const CALL_SECS: i64 = 11 * 3600;

fn saved(call: &Call) -> i64 {
    (call.3 - call.4).max(0)
}

struct Fixture {
    _dir: TempDir,
    home: PathBuf,
    db_path: PathBuf,
    alpha: String,
    beta: String,
    gamma: String,
}

impl Fixture {
    fn path(&self, repo: Repo) -> &str {
        match repo {
            Repo::Alpha => &self.alpha,
            Repo::Beta => &self.beta,
            Repo::Elsewhere => "/elsewhere/repo",
        }
    }

    fn hash(&self, repo: Repo) -> String {
        self.path(repo)
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect()
    }
}

async fn fixture() -> Fixture {
    let dir = TempDir::new().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let home = root.join("home");
    let data = home.join(".tokensave");
    let db_path = data.join("global.db");
    // Keyed as tokensave keys a project: on Windows `canonicalize` gives a
    // verbatim `\\?\C:\...` path, which tokensave stores without the prefix.
    let repo = |name: &str| {
        let path = root.join("repos").join(name);
        std::fs::create_dir_all(&path).unwrap();
        tokensave::global_db::normalize_project_key(&path)
    };
    let (alpha, beta, gamma) = (repo("alpha"), repo("beta"), repo("gamma"));
    std::fs::create_dir_all(Path::new(&alpha).join(".tokensave")).unwrap();
    std::fs::write(
        Path::new(&alpha).join(".tokensave").join("tokensave.db"),
        b"",
    )
    .unwrap();
    // Claude Code, Codex and Pi data folders exist; Factory Droid's does not.
    for folder in [".claude/projects", ".codex/sessions", ".pi/agent/sessions"] {
        std::fs::create_dir_all(home.join(folder)).unwrap();
    }

    drop(
        tokensave::global_db::GlobalDb::open_at(&db_path)
            .await
            .unwrap(),
    );
    let fixture = Fixture {
        _dir: dir,
        home,
        db_path,
        alpha,
        beta,
        gamma,
    };
    let db = libsql::Builder::new_local(&fixture.db_path)
        .build()
        .await
        .unwrap();
    let conn = db.connect().unwrap();
    for path in [&fixture.alpha, &fixture.beta, &fixture.gamma] {
        conn.execute(
            "INSERT INTO projects (path, tokens_saved) VALUES (?1, 0)",
            [path.as_str()],
        )
        .await
        .unwrap();
    }
    for t in TURNS {
        let text =
            |v: Option<&str>| v.map_or(libsql::Value::Null, |v| libsql::Value::Text(v.into()));
        conn.execute(
            "INSERT INTO turns (message_id, project_hash, session_id, model, timestamp, input_tokens,
                                output_tokens, cache_write_tokens, cache_read_tokens, cost_usd, category,
                                tool_names, agent, tool_result_tokens, effort, requested_model,
                                thinking_tokens, run_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'coding', ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            vec![
                libsql::Value::Text(t.id.into()),
                libsql::Value::Text(fixture.hash(t.repo)),
                libsql::Value::Text(t.session.into()),
                libsql::Value::Text(t.model.into()),
                libsql::Value::Integer(t.ts()),
                libsql::Value::Integer(t.input),
                libsql::Value::Integer(t.output),
                libsql::Value::Integer(t.cache_write),
                libsql::Value::Integer(t.cache_read),
                t.cost.map_or(libsql::Value::Null, libsql::Value::Real),
                libsql::Value::Text(t.tools.into()),
                libsql::Value::Text(t.agent.into()),
                libsql::Value::Integer(t.tool_result),
                text(t.effort),
                text(t.requested),
                t.thinking.map_or(libsql::Value::Null, libsql::Value::Integer),
                text(t.run),
            ],
        )
        .await
        .unwrap();
    }
    for (run, session, role, description, parent, finished) in RUNS {
        let text =
            |v: Option<&str>| v.map_or(libsql::Value::Null, |v| libsql::Value::Text(v.into()));
        conn.execute(
            "INSERT INTO agent_runs (run_id, session_id, project_hash, role, description, parent_run_id, finished)
             VALUES (?1, ?2, 'p', ?3, ?4, ?5, ?6)",
            vec![
                libsql::Value::Text(run.into()),
                libsql::Value::Text(session.into()),
                libsql::Value::Text(role.into()),
                text(description),
                text(parent),
                finished.map_or(libsql::Value::Null, libsql::Value::Integer),
            ],
        )
        .await
        .unwrap();
    }
    for (days_ago, repo, tool, before, after) in CALLS {
        conn.execute(
            "INSERT INTO savings_ledger (ts, project_path, tool_name, before_tokens, after_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            vec![
                libsql::Value::Integer((TODAY - days_ago) * DAY + CALL_SECS),
                libsql::Value::Text(fixture.path(repo).into()),
                libsql::Value::Text(tool.into()),
                libsql::Value::Integer(before),
                libsql::Value::Integer(after),
            ],
        )
        .await
        .unwrap();
    }
    fixture
}

fn context(fx: &Fixture, port: u16) -> Context {
    Context {
        db_path: fx.db_path.clone(),
        bind: std::net::IpAddr::from([127, 0, 0, 1]),
        port,
        default_range_days: 30,
        home: Some(fx.home.clone()),
        codex_home: Some(fx.home.join(".codex")),
        now,
    }
}

static PAGE: &[(&str, &[u8])] = &[
    (
        "index.html",
        b"<!doctype html><title>tokensave</title><script src=\"assets/app.js\"></script>",
    ),
    ("assets/app.js", b"console.log(1)"),
    ("tokens.css", b":root{}"),
];

/// Serves the fixture with `assets` on a free loopback port until the
/// returned sender is dropped.
async fn serve(
    fx: &Fixture,
    assets: &'static [(&'static str, &'static [u8])],
) -> (u16, tokio::sync::oneshot::Sender<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = Arc::new(Server::new(context(fx, port), assets));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(http::serve(listener, server, async {
        let _ = stopped.await;
    }));
    (port, stop)
}

struct Reply {
    status: u16,
    headers: BTreeMap<String, String>,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        assert_eq!(
            self.headers["content-type"], "application/json",
            "{}",
            self.body
        );
        serde_json::from_str(&self.body).unwrap()
    }
}

async fn send(port: u16, request: &str) -> Reply {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    // A server that never answers fails the test instead of holding the run.
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut raw))
        .await
        .expect("the server did not answer within 10 s")
        .unwrap();
    let raw = String::from_utf8(raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.to_string()))
        .collect();
    Reply {
        status,
        headers,
        body: body.to_string(),
    }
}

async fn get(port: u16, path: &str) -> Reply {
    send(
        port,
        &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"),
    )
    .await
}

async fn get_json(port: u16, path: &str) -> Value {
    let reply = get(port, path).await;
    assert_eq!(reply.status, 200, "{path}: {}", reply.body);
    reply.json()
}

/// A repo path that is absolute on this platform and holds no turn.
#[cfg(not(windows))]
const NOWHERE: &str = "/nowhere";
#[cfg(windows)]
const NOWHERE: &str = r"C:\nowhere";

fn encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

// ── Type checks ───────────────────────────────────────────────────────────

/// A JSON kind a TypeScript field allows.
#[derive(Clone, Copy, Debug)]
enum K {
    Num,
    NumOrNull,
    Str,
    StrOrNull,
    Bool,
    Arr,
    ObjOrNull,
}

/// Checks `value` is an object with exactly `fields`, each of its kind.
fn shape(value: &Value, fields: &[(&str, K)]) {
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("not an object: {value}"));
    let keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = fields.iter().map(|(name, _)| *name).collect();
    assert_eq!(keys, expected, "{value}");
    for (name, kind) in fields {
        let field = &object[*name];
        let ok = match kind {
            K::Num => field.is_number(),
            K::NumOrNull => field.is_number() || field.is_null(),
            K::Str => field.is_string(),
            K::StrOrNull => field.is_string() || field.is_null(),
            K::Bool => field.is_boolean(),
            K::Arr => field.is_array(),
            K::ObjOrNull => field.is_object() || field.is_null(),
        };
        assert!(ok, "{name} is not {kind:?}: {field}");
    }
}

fn each(value: &Value, fields: &[(&str, K)]) {
    for item in value
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {value}"))
    {
        shape(item, fields);
    }
}

const TOOLS: [&str; 5] = ["Claude Code", "Codex", "Cursor", "Pi", "Factory Droid"];
const ROLES: [&str; 5] = ["main", "builder", "reviewer", "Explore", "other"];
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

fn one_of(value: &Value, allowed: &[&str]) {
    assert!(
        allowed.contains(&value.as_str().unwrap_or("")),
        "{value} not in {allowed:?}"
    );
}

fn effort_or_null(value: &Value) {
    if !value.is_null() {
        one_of(value, &EFFORTS);
    }
}

fn close(a: &Value, b: f64) {
    let a = a.as_f64().unwrap_or_else(|| panic!("not a number: {a}"));
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

/// The seed's turns the dashboard counts in the `days` ending today.
fn window(days: i64) -> impl Fn(&&Turn) -> bool {
    move |t| t.known_agent() && t.ts() >= (TODAY - days + 1) * DAY && t.ts() < (TODAY + 1) * DAY
}

fn calls_in(days: i64) -> impl Fn(&&Call) -> bool {
    move |c| c.0 < days
}

#[tokio::test]
async fn every_endpoint_answers_in_its_type() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;

    shape(
        &get_json(port, "/api/status").await,
        &[("bind", K::Str), ("port", K::Num), ("workspaces", K::Arr)],
    );
    shape(
        &get_json(port, "/api/summary").await,
        &[
            ("spentTokens", K::Num),
            ("costUsd", K::Num),
            ("unpricedTurns", K::Num),
            ("savedTokens", K::Num),
            ("savingRate", K::Num),
            ("cacheHitRate", K::Num),
        ],
    );
    each(
        &get_json(port, "/api/daily").await,
        &[
            ("date", K::Str),
            ("spentTokens", K::Num),
            ("savedTokens", K::Num),
        ],
    );
    let mix = get_json(port, "/api/mix").await;
    shape(&mix, &[("byTool", K::Arr), ("byModel", K::Arr)]);
    each(
        &mix["byTool"],
        &[("tool", K::Str), ("tokens", K::Num), ("share", K::Num)],
    );
    each(
        &mix["byModel"],
        &[("model", K::Str), ("tokens", K::Num), ("share", K::Num)],
    );
    for row in mix["byTool"].as_array().unwrap() {
        one_of(&row["tool"], &TOOLS);
    }
    let top = get_json(port, "/api/runs/top?limit=5").await;
    each(
        &top,
        &[
            ("id", K::Str),
            ("name", K::Str),
            ("role", K::Str),
            ("repo", K::Str),
            ("model", K::Str),
            ("effort", K::StrOrNull),
            ("tokens", K::Num),
            ("costUsd", K::NumOrNull),
        ],
    );
    let runs = get_json(port, "/api/runs").await;
    each(
        &runs,
        &[
            ("id", K::Str),
            ("parentId", K::StrOrNull),
            ("name", K::Str),
            ("role", K::Str),
            ("tool", K::Str),
            ("model", K::Str),
            ("effort", K::StrOrNull),
            ("inputTokens", K::Num),
            ("outputTokens", K::NumOrNull),
            ("costUsd", K::NumOrNull),
            ("state", K::Str),
        ],
    );
    for run in top
        .as_array()
        .unwrap()
        .iter()
        .chain(runs.as_array().unwrap())
    {
        one_of(&run["role"], &ROLES);
        effort_or_null(&run["effort"]);
    }
    for run in runs.as_array().unwrap() {
        one_of(&run["tool"], &TOOLS);
        one_of(
            &run["state"],
            &["finished", "running", "stopped", "partial"],
        );
    }
    let roles = get_json(port, "/api/roles").await;
    shape(
        &roles,
        &[
            ("roles", K::Arr),
            ("subagentShare", K::Num),
            ("unfinishedRuns", K::Num),
        ],
    );
    each(
        &roles["roles"],
        &[("role", K::Str), ("costUsd", K::Num), ("share", K::Num)],
    );
    let models = get_json(port, "/api/models").await;
    shape(&models, &[("efforts", K::Arr), ("rows", K::Arr)]);
    each(&models["rows"], &[("model", K::Str), ("costUsd", K::Arr)]);
    for row in models["rows"].as_array().unwrap() {
        assert_eq!(row["costUsd"].as_array().unwrap().len(), 5);
        assert!(row["costUsd"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c.is_number() || c.is_null()));
    }
    let thinking = get_json(port, "/api/thinking").await;
    each(
        &thinking,
        &[
            ("model", K::Str),
            ("effort", K::StrOrNull),
            ("share", K::Num),
        ],
    );
    let fallbacks = get_json(port, "/api/fallbacks").await;
    shape(&fallbacks, &[("pairs", K::Arr), ("rate", K::Num)]);
    each(
        &fallbacks["pairs"],
        &[
            ("requested", K::Str),
            ("served", K::Str),
            ("turns", K::Num),
            ("sessions", K::Num),
        ],
    );
    let repos = get_json(port, "/api/repos").await;
    shape(&repos, &[("workspace", K::ObjOrNull), ("repos", K::Arr)]);
    each(
        &repos["repos"],
        &[
            ("name", K::Str),
            ("path", K::Str),
            ("indexed", K::Bool),
            ("spentTokens", K::NumOrNull),
            ("savedTokens", K::NumOrNull),
            ("tools", K::Arr),
            ("lastActivity", K::StrOrNull),
        ],
    );
    let savings = get_json(port, "/api/savings").await;
    shape(
        &savings,
        &[
            ("savedTokens", K::Num),
            ("beforeTokens", K::Num),
            ("afterTokens", K::Num),
            ("graphCalls", K::Num),
            ("missedTokens", K::Num),
            ("byTool", K::Arr),
            ("missed", K::Arr),
        ],
    );
    each(
        &savings["byTool"],
        &[("tool", K::Str), ("savedTokens", K::Num), ("share", K::Num)],
    );
    each(
        &savings["missed"],
        &[
            ("kind", K::Str),
            ("tokens", K::Num),
            ("turns", K::Num),
            ("instead", K::Str),
        ],
    );
    let sources = get_json(port, "/api/sources").await;
    each(
        &sources,
        &[
            ("tool", K::Str),
            ("path", K::Str),
            ("turns", K::NumOrNull),
            ("lastParsed", K::StrOrNull),
            ("status", K::Str),
            ("note", K::Str),
        ],
    );
    for source in sources.as_array().unwrap() {
        one_of(&source["tool"], &TOOLS);
        one_of(
            &source["status"],
            &["verified", "partial", "not found", "unknown"],
        );
    }
}

#[tokio::test]
async fn status_names_the_address_and_no_workspace_yet() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    let status = get_json(port, "/api/status").await;
    assert_eq!(status["bind"], "127.0.0.1");
    assert_eq!(status["port"], port);
    assert_eq!(status["workspaces"], serde_json::json!([]));
}

/// The summary's figures, computed from the seed.
fn expected_summary(turns: &[&Turn], calls: &[&Call]) -> (i64, f64, i64, i64, f64, f64) {
    let spent: i64 = turns.iter().map(|t| t.spent()).sum();
    let cost: f64 = turns.iter().filter_map(|t| t.cost).sum();
    let unpriced = turns.iter().filter(|t| t.cost.is_none()).count() as i64;
    let saved: i64 = calls.iter().map(|c| saved(c)).sum();
    let prompt: i64 = turns
        .iter()
        .map(|t| t.input + t.cache_write + t.cache_read)
        .sum();
    let cache_read: i64 = turns.iter().map(|t| t.cache_read).sum();
    let rate = if spent + saved > 0 {
        saved as f64 / (spent + saved) as f64
    } else {
        0.0
    };
    let hit = if prompt > 0 {
        cache_read as f64 / prompt as f64
    } else {
        0.0
    };
    (spent, cost, unpriced, saved, rate, hit)
}

fn check_summary(summary: &Value, expected: (i64, f64, i64, i64, f64, f64)) {
    assert_eq!(summary["spentTokens"], expected.0, "{summary}");
    close(&summary["costUsd"], expected.1);
    assert_eq!(summary["unpricedTurns"], expected.2, "{summary}");
    assert_eq!(summary["savedTokens"], expected.3, "{summary}");
    close(&summary["savingRate"], expected.4);
    close(&summary["cacheHitRate"], expected.5);
}

#[tokio::test]
async fn the_summary_follows_every_range_and_filter() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    let in30: Vec<&Turn> = TURNS.iter().filter(window(30)).collect();
    let calls30: Vec<&Call> = CALLS.iter().filter(calls_in(30)).collect();

    // The default range is 30 days: the unknown agent and older turns are out.
    check_summary(
        &get_json(port, "/api/summary").await,
        expected_summary(&in30, &calls30),
    );
    assert_eq!(
        get_json(port, "/api/summary").await,
        get_json(port, "/api/summary?range=30").await
    );
    for days in [7, 45, 90] {
        let turns: Vec<&Turn> = TURNS.iter().filter(window(days)).collect();
        let calls: Vec<&Call> = CALLS.iter().filter(calls_in(days)).collect();
        check_summary(
            &get_json(port, &format!("/api/summary?range={days}")).await,
            expected_summary(&turns, &calls),
        );
    }

    // A custom range includes both of its days.
    let custom: Vec<&Turn> = TURNS
        .iter()
        .filter(|t| t.known_agent() && (1..=10).contains(&t.days_ago))
        .collect();
    let custom_calls: Vec<&Call> = CALLS.iter().filter(|c| (1..=10).contains(&c.0)).collect();
    check_summary(
        &get_json(
            port,
            "/api/summary?range=custom&from=2026-10-17&to=2026-10-26",
        )
        .await,
        expected_summary(&custom, &custom_calls),
    );

    // The agent tool, model and effort filters select turns; the ledger
    // records none of them, so the saved tokens stay those of the window.
    let only = |keep: &dyn Fn(&Turn) -> bool| -> Vec<&Turn> {
        in30.iter().copied().filter(|t| keep(t)).collect()
    };
    check_summary(
        &get_json(port, "/api/summary?tool=Codex").await,
        expected_summary(&only(&|t| t.agent == "codex"), &calls30),
    );
    check_summary(
        &get_json(port, "/api/summary?model=claude-opus-5-5").await,
        expected_summary(&only(&|t| t.model == "claude-opus-5-5"), &calls30),
    );
    check_summary(
        &get_json(port, "/api/summary?effort=high").await,
        expected_summary(&only(&|t| t.effort == Some("high")), &calls30),
    );
    // The repo filter selects both its turns and its tokensave calls.
    for repo in [Repo::Alpha, Repo::Beta] {
        let calls: Vec<&Call> = calls30.iter().copied().filter(|c| c.1 == repo).collect();
        check_summary(
            &get_json(
                port,
                &format!("/api/summary?repo={}", encode(fx.path(repo))),
            )
            .await,
            expected_summary(&only(&|t| t.repo == repo), &calls),
        );
    }
    // Filters combine.
    check_summary(
        &get_json(
            port,
            &format!(
                "/api/summary?range=7&repo={}&effort=xhigh",
                encode(&fx.alpha)
            ),
        )
        .await,
        expected_summary(
            &TURNS
                .iter()
                .filter(window(7))
                .filter(|t| t.repo == Repo::Alpha && t.effort == Some("xhigh"))
                .collect::<Vec<_>>(),
            &CALLS
                .iter()
                .filter(calls_in(7))
                .filter(|c| c.1 == Repo::Alpha)
                .collect::<Vec<_>>(),
        ),
    );
}

#[tokio::test]
async fn daily_has_one_row_per_day_of_the_range() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    for days in [7_i64, 30, 45, 60, 90] {
        let daily = get_json(port, &format!("/api/daily?range={days}")).await;
        let rows = daily.as_array().unwrap();
        assert_eq!(rows.len() as i64, days);
        for (index, row) in rows.iter().enumerate() {
            let days_ago = days - 1 - index as i64;
            let date = crate_date(TODAY - days_ago);
            assert_eq!(row["date"], date);
            let spent: i64 = TURNS
                .iter()
                .filter(|t| t.known_agent() && t.days_ago == days_ago)
                .map(Turn::spent)
                .sum();
            let saved_tokens: i64 = CALLS.iter().filter(|c| c.0 == days_ago).map(saved).sum();
            assert_eq!(row["spentTokens"], spent, "{date}");
            assert_eq!(row["savedTokens"], saved_tokens, "{date}");
        }
    }
    let custom = get_json(
        port,
        "/api/daily?from=2026-10-25&to=2026-10-26&tool=Claude%20Code",
    )
    .await;
    let alpha_yesterday: i64 = TURNS
        .iter()
        .filter(|t| t.agent == "claude" && t.days_ago == 1)
        .map(Turn::spent)
        .sum();
    assert_eq!(
        custom,
        serde_json::json!([
            {"date": "2026-10-25", "spentTokens": 0, "savedTokens": 0},
            {"date": "2026-10-26", "spentTokens": alpha_yesterday, "savedTokens": 300},
        ])
    );
}

/// `YYYY-MM-DD` of a day number, by counting from a known date.
fn crate_date(day: i64) -> String {
    let base = TODAY; // 2026-10-27
    let mut date = (2026, 10, 27);
    let mut current = base;
    let month_len = |y: i32, m: u32| match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    };
    while current > day {
        let (y, m, d) = date;
        date = if d > 1 {
            (y, m, d - 1)
        } else if m > 1 {
            (y, m - 1, month_len(y, m - 1))
        } else {
            (y - 1, 12, 31)
        };
        current -= 1;
    }
    format!("{:04}-{:02}-{:02}", date.0, date.1, date.2)
}

#[tokio::test]
async fn the_mix_shares_spent_tokens_by_tool_and_model() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    let mix = get_json(port, "/api/mix").await;
    let in30: Vec<&Turn> = TURNS.iter().filter(window(30)).collect();
    let total: i64 = in30.iter().map(|t| t.spent()).sum();
    let by = |key: &dyn Fn(&Turn) -> String| {
        let mut sums: BTreeMap<String, i64> = BTreeMap::new();
        for t in &in30 {
            *sums.entry(key(t)).or_default() += t.spent();
        }
        let mut rows: Vec<(String, i64)> = sums.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        rows
    };
    let tools = by(&|t| {
        (if t.agent == "claude" {
            "Claude Code"
        } else {
            "Codex"
        })
        .to_string()
    });
    assert_eq!(mix["byTool"].as_array().unwrap().len(), tools.len());
    for (row, (tool, tokens)) in mix["byTool"].as_array().unwrap().iter().zip(&tools) {
        assert_eq!(row["tool"], tool.as_str());
        assert_eq!(row["tokens"], *tokens);
        close(&row["share"], *tokens as f64 / total as f64);
    }
    let models = by(&|t| t.model.to_string());
    assert_eq!(mix["byModel"].as_array().unwrap().len(), models.len());
    for (row, (model, tokens)) in mix["byModel"].as_array().unwrap().iter().zip(&models) {
        assert_eq!(row["model"], model.as_str());
        assert_eq!(row["tokens"], *tokens);
        close(&row["share"], *tokens as f64 / total as f64);
    }
    // A filter that leaves nothing gives empty lists, not a division by zero.
    assert_eq!(
        get_json(port, "/api/mix?tool=Pi").await,
        serde_json::json!({"byTool": [], "byModel": []})
    );
}

#[tokio::test]
async fn runs_nest_subagents_under_their_session_with_role_and_state() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    let runs = get_json(port, "/api/runs").await;
    let ids: Vec<&str> = runs
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    // Sessions the costliest first (S1 1.5 + its runs, C1 0.5), each followed
    // by its runs, A3 under A1 that started it.
    assert_eq!(ids, ["S1", "A1", "A3", "A2", "C1", "CT1"]);
    let by_id: BTreeMap<&str, &Value> = runs
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["id"].as_str().unwrap(), r))
        .collect();
    let expect = |id: &str, parent: Value, name: &str, role: &str, tool: &str, state: &str| {
        let run = by_id[id];
        assert_eq!(run["parentId"], parent, "{id}");
        assert_eq!(run["name"], name, "{id}");
        assert_eq!(run["role"], role, "{id}");
        assert_eq!(run["tool"], tool, "{id}");
        assert_eq!(run["state"], state, "{id}");
    };
    expect(
        "S1",
        Value::Null,
        "alpha · session",
        "main",
        "Claude Code",
        "finished",
    );
    expect(
        "A1",
        "S1".into(),
        "Review PR #1",
        "reviewer",
        "Claude Code",
        "running",
    );
    expect(
        "A3",
        "A1".into(),
        "Build S3a",
        "builder",
        "Claude Code",
        "partial",
    );
    expect(
        "A2",
        "S1".into(),
        "Find callers",
        "Explore",
        "Claude Code",
        "finished",
    );
    expect(
        "C1",
        Value::Null,
        "beta · session",
        "main",
        "Codex",
        "finished",
    );
    expect("CT1", "C1".into(), "worker", "other", "Codex", "partial");

    // Tokens and cost of each run, from its turns.
    for (id, run) in &by_id {
        let turns: Vec<&Turn> = TURNS
            .iter()
            .filter(window(30))
            .filter(|t| t.run.unwrap_or(t.session) == *id)
            .collect();
        assert_eq!(
            run["inputTokens"],
            turns
                .iter()
                .map(|t| t.input + t.cache_write + t.cache_read)
                .sum::<i64>(),
            "{id}"
        );
        assert_eq!(
            run["outputTokens"],
            turns.iter().map(|t| t.output).sum::<i64>(),
            "{id}"
        );
        if turns.iter().all(|t| t.cost.is_some()) {
            close(&run["costUsd"], turns.iter().filter_map(|t| t.cost).sum());
        } else {
            assert!(
                run["costUsd"].is_null(),
                "{id}: an unpriced turn makes the cost unknown"
            );
        }
    }
    // An effort the API does not name is null; the model is the run's.
    assert_eq!(by_id["CT1"]["effort"], Value::Null);
    assert_eq!(by_id["A2"]["model"], "claude-sonnet-5-5");

    // The top runs are the costliest, with their repo; the unpriced one last.
    let top = get_json(port, "/api/runs/top").await;
    let top_ids: Vec<&str> = top
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(top_ids, ["A1", "S1", "A3", "C1", "CT1"]);
    assert_eq!(top[0]["repo"], "alpha");
    assert_eq!(top[0]["tokens"], 800);
    assert_eq!(top[3]["repo"], "beta");
    let two = get_json(port, "/api/runs/top?limit=2").await;
    assert_eq!(two.as_array().unwrap().len(), 2);

    // Filters select the turns the runs are made of.
    let codex = get_json(port, "/api/runs?tool=Codex").await;
    let ids: Vec<&str> = codex
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["C1", "CT1"]);
    let week = get_json(port, "/api/runs?range=7").await;
    assert_eq!(week.as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn roles_share_the_priced_cost() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    let roles = get_json(port, "/api/roles").await;
    let role_of = |t: &Turn| match t.run {
        None => "main",
        Some("A1") => "reviewer",
        Some("A2") => "Explore",
        Some("A3") => "builder",
        Some(_) => "other",
    };
    let in30: Vec<&Turn> = TURNS.iter().filter(window(30)).collect();
    let total: f64 = in30.iter().filter_map(|t| t.cost).sum();
    let mut costs: BTreeMap<&str, f64> = BTreeMap::new();
    for t in &in30 {
        *costs.entry(role_of(t)).or_default() += t.cost.unwrap_or(0.0);
    }
    let rows = roles["roles"].as_array().unwrap();
    assert_eq!(rows.len(), costs.len());
    let names: Vec<&str> = rows.iter().map(|r| r["role"].as_str().unwrap()).collect();
    assert_eq!(names, ["main", "reviewer", "builder", "other", "Explore"]);
    for row in rows {
        let cost = costs[row["role"].as_str().unwrap()];
        close(&row["costUsd"], cost);
        close(&row["share"], cost / total);
    }
    close(&roles["subagentShare"], (total - costs["main"]) / total);
    // A1 running, A3 and CT1 never said they finished.
    assert_eq!(roles["unfinishedRuns"], 3);
    assert_eq!(
        get_json(port, "/api/roles?tool=Pi").await,
        serde_json::json!({"roles": [], "subagentShare": 0.0, "unfinishedRuns": 0})
    );
}

#[tokio::test]
async fn models_thinking_and_fallbacks_follow_the_turns() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    let in30: Vec<&Turn> = TURNS.iter().filter(window(30)).collect();

    let models = get_json(port, "/api/models").await;
    assert_eq!(models["efforts"], serde_json::json!(EFFORTS));
    let cell = |model: &str, effort: &str| -> Value {
        let turns: Vec<&&Turn> = in30
            .iter()
            .filter(|t| t.model == model && t.effort == Some(effort))
            .collect();
        if turns.is_empty() || turns.iter().any(|t| t.cost.is_none()) {
            Value::Null
        } else {
            serde_json::json!(turns.iter().filter_map(|t| t.cost).sum::<f64>())
        }
    };
    let names: Vec<&str> = models["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["model"].as_str().unwrap())
        .collect();
    // By priced cost: opus 4.25, gpt 0.5, sonnet (unpriced) 0.
    assert_eq!(
        names,
        ["claude-opus-5-5", "gpt-6.1-sol", "claude-sonnet-5-5"]
    );
    for row in models["rows"].as_array().unwrap() {
        let model = row["model"].as_str().unwrap();
        let expected: Vec<Value> = EFFORTS.iter().map(|e| cell(model, e)).collect();
        assert_eq!(row["costUsd"], Value::Array(expected), "{model}");
    }

    let thinking = get_json(port, "/api/thinking").await;
    assert_eq!(
        thinking,
        serde_json::json!([
            {"model": "claude-opus-5-5", "effort": "high", "share": 20.0 / 50.0},
            {"model": "claude-opus-5-5", "effort": "xhigh", "share": 40.0 / 100.0},
            {"model": "gpt-6.1-sol", "effort": "high", "share": 5.0 / 30.0},
        ])
    );
    let codex_thinking = get_json(port, "/api/thinking?tool=Codex").await;
    assert_eq!(codex_thinking.as_array().unwrap().len(), 1);

    // claude-opus-5-5[1m] asked for the model that served it; fable did not.
    let fallbacks = get_json(port, "/api/fallbacks").await;
    assert_eq!(
        fallbacks,
        serde_json::json!({
            "pairs": [{"requested": "claude-fable-5-1", "served": "claude-opus-5-5", "turns": 1, "sessions": 1}],
            "rate": 0.5,
        })
    );
    assert_eq!(
        get_json(
            port,
            &format!("/api/fallbacks?range=7&repo={}", encode(NOWHERE))
        )
        .await,
        serde_json::json!({"pairs": [], "rate": 0.0})
    );
}

#[tokio::test]
async fn repos_come_from_the_registry_with_their_figures() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    let repos = get_json(port, "/api/repos").await;
    assert_eq!(repos["workspace"], Value::Null);
    let rows = repos["repos"].as_array().unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["alpha", "beta", "gamma"]);

    let in30: Vec<&Turn> = TURNS.iter().filter(window(30)).collect();
    let spent = |repo: Repo| {
        in30.iter()
            .filter(|t| t.repo == repo)
            .map(|t| t.spent())
            .sum::<i64>()
    };
    let iso = |ts: i64| tokensave_iso(ts);
    let last = |repo: Repo| {
        let turn = TURNS.iter().filter(|t| t.repo == repo).map(Turn::ts).max();
        let call = CALLS
            .iter()
            .filter(|c| c.1 == repo)
            .map(|c| (TODAY - c.0) * DAY + CALL_SECS)
            .max();
        turn.max(call).map(iso)
    };
    assert_eq!(
        rows[0],
        serde_json::json!({
            "name": "alpha", "path": fx.alpha, "indexed": true, "spentTokens": spent(Repo::Alpha),
            "savedTokens": 1200, "tools": ["Claude Code"], "lastActivity": last(Repo::Alpha),
        })
    );
    assert_eq!(
        rows[1],
        serde_json::json!({
            "name": "beta", "path": fx.beta, "indexed": false, "spentTokens": spent(Repo::Beta),
            "savedTokens": 0, "tools": ["Codex"], "lastActivity": last(Repo::Beta),
        })
    );
    assert_eq!(
        rows[2],
        serde_json::json!({
            "name": "gamma", "path": fx.gamma, "indexed": false, "spentTokens": null,
            "savedTokens": null, "tools": [], "lastActivity": null,
        })
    );
    // The tool filter limits the spent tokens and the tools.
    let codex = get_json(port, "/api/repos?tool=Codex").await;
    assert_eq!(codex["repos"][0]["name"], "beta");
    assert_eq!(codex["repos"][1]["spentTokens"], Value::Null);
}

/// A unix time as the API writes it.
fn tokensave_iso(ts: i64) -> String {
    let day = ts.div_euclid(DAY);
    let secs = ts.rem_euclid(DAY);
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        crate_date(day),
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

#[tokio::test]
async fn savings_and_missed_savings_follow_the_range_and_repo() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    let savings = get_json(port, "/api/savings").await;
    let calls: Vec<&Call> = CALLS.iter().filter(calls_in(30)).collect();
    let total: i64 = calls.iter().map(|c| saved(c)).sum();
    assert_eq!(savings["savedTokens"], total);
    assert_eq!(
        savings["beforeTokens"],
        calls.iter().map(|c| c.3).sum::<i64>()
    );
    assert_eq!(
        savings["afterTokens"],
        calls.iter().map(|c| c.4).sum::<i64>()
    );
    assert_eq!(savings["graphCalls"], calls.len());
    assert_eq!(
        savings["byTool"],
        serde_json::json!([
            {"tool": "tokensave_context", "savedTokens": 900, "share": 900.0 / 1200.0},
            {"tool": "tokensave_search", "savedTokens": 300, "share": 300.0 / 1200.0},
        ])
    );
    // Claude Code turns whose every tool was Read, Grep or Glob, half their
    // tool results recoverable: t2 Grep 1000, t1 Read 400, t3 Read+Glob 60.
    assert_eq!(
        savings["missed"],
        serde_json::json!([
            {"kind": "Symbol searches with grep", "tokens": 500, "turns": 1, "instead": "search / callers / callees / impact"},
            {"kind": "Whole-file reads", "tokens": 200, "turns": 1, "instead": "outline / read / node"},
            {"kind": "File discovery with glob", "tokens": 30, "turns": 1, "instead": "files / search"},
        ])
    );
    assert_eq!(savings["missedTokens"], 730);

    let beta = get_json(port, &format!("/api/savings?repo={}", encode(&fx.beta))).await;
    assert_eq!(
        (
            beta["savedTokens"].clone(),
            beta["graphCalls"].clone(),
            beta["missedTokens"].clone()
        ),
        (0.into(), 1.into(), 0.into())
    );
    let quarter = get_json(port, "/api/savings?range=45").await;
    assert_eq!(quarter["savedTokens"], 2000);
}

#[tokio::test]
async fn sources_say_what_was_found_and_recorded() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    let sources = get_json(port, "/api/sources").await;
    let rows = sources.as_array().unwrap();
    let tools: Vec<&str> = rows.iter().map(|s| s["tool"].as_str().unwrap()).collect();
    assert_eq!(tools, TOOLS);
    let count = |agent: &str| TURNS.iter().filter(|t| t.agent == agent).count() as i64;
    let newest = |agent: &str| {
        TURNS
            .iter()
            .filter(|t| t.agent == agent)
            .map(Turn::ts)
            .max()
            .map(tokensave_iso)
    };

    assert_eq!(rows[0]["status"], "verified");
    assert_eq!(rows[0]["turns"], count("claude"));
    assert_eq!(rows[0]["lastParsed"], newest("claude").unwrap());
    assert_eq!(
        rows[0]["path"],
        fx.home
            .join(".claude")
            .join("projects")
            .display()
            .to_string()
    );
    assert_eq!(
        (rows[1]["status"].clone(), rows[1]["turns"].clone()),
        ("verified".into(), count("codex").into())
    );
    // Cursor has no local data and nothing recorded.
    assert_eq!(
        (
            rows[2]["status"].clone(),
            rows[2]["turns"].clone(),
            rows[2]["lastParsed"].clone()
        ),
        ("not found".into(), Value::Null, Value::Null)
    );
    // Pi's folder exists with nothing recorded from it.
    assert_eq!(
        (rows[3]["status"].clone(), rows[3]["turns"].clone()),
        ("unknown".into(), 0.into())
    );
    // Factory Droid's folder does not exist.
    assert_eq!(
        (rows[4]["status"].clone(), rows[4]["turns"].clone()),
        ("not found".into(), Value::Null)
    );
}

#[tokio::test]
async fn an_invalid_parameter_is_a_400_that_names_it() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    for (path, parameter) in [
        ("/api/summary?range=14", "range"),
        ("/api/summary?range=custom&from=2026-10-01", "to"),
        // A day past UTC today: no viewer's local date is that far ahead.
        ("/api/summary?from=2026-10-29&to=2026-10-29", "to"),
        ("/api/summary?from=2026-10-20&to=2026-10-10", "from"),
        ("/api/daily?from=2026-13-01&to=2026-10-10", "from"),
        ("/api/summary?range=7&from=2026-10-01&to=2026-10-02", "from"),
        ("/api/mix?tool=claude", "tool"),
        ("/api/runs?effort=minimal", "effort"),
        ("/api/runs/top?limit=0", "limit"),
        ("/api/runs/top?limit=abc", "limit"),
        ("/api/summary?repo=alpha", "repo"),
        ("/api/models?model=x", "model"),
        ("/api/thinking?effort=high", "effort"),
        ("/api/savings?tool=Codex", "tool"),
        ("/api/repos?workspace=%7E%2FgitRepos", "workspace"),
        ("/api/status?range=7", "range"),
        ("/api/sources?x=1", "x"),
        ("/api/summary?tool=Codex&tool=Pi", "tool"),
    ] {
        let reply = get(port, path).await;
        assert_eq!(reply.status, 400, "{path}: {}", reply.body);
        let body = reply.json();
        assert_eq!(body["parameter"], parameter, "{path}: {body}");
        assert!(
            body["error"].as_str().unwrap().contains(parameter),
            "{path}: {body}"
        );
    }
    let reply = get(port, "/api/nothing").await;
    assert_eq!(reply.status, 404);
}

#[tokio::test]
async fn the_page_is_served_with_its_types_and_no_remote_origin() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    let index = get(port, "/").await;
    assert_eq!(index.status, 200);
    assert_eq!(index.headers["content-type"], "text/html; charset=utf-8");
    assert_eq!(
        index.headers["content-security-policy"],
        "default-src 'self'"
    );
    assert!(index.body.contains("<title>tokensave</title>"));
    let script = get(port, "/assets/app.js").await;
    assert_eq!(
        (script.status, script.headers["content-type"].as_str()),
        (200, "text/javascript; charset=utf-8")
    );
    let css = get(port, "/tokens.css").await;
    assert_eq!(css.headers["content-type"], "text/css; charset=utf-8");
    assert_eq!(get(port, "/missing.js").await.status, 404);
    assert_eq!(get(port, "/../global.db").await.status, 404);
    // The API carries the policy too.
    assert_eq!(
        get(port, "/api/status").await.headers["content-security-policy"],
        "default-src 'self'"
    );
}

#[tokio::test]
async fn without_a_built_page_the_root_is_a_503_and_the_api_works() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, &[]).await;
    let index = get(port, "/").await;
    assert_eq!(index.status, 503);
    assert!(
        index.body.contains("dashboard page not built"),
        "{}",
        index.body
    );
    assert_eq!(get(port, "/api/summary").await.status, 200);
}

#[tokio::test]
async fn another_host_name_and_other_methods_are_refused() {
    let fx = fixture().await;
    let (port, _stop) = serve(&fx, PAGE).await;
    let rebound = send(port, &format!("GET /api/summary HTTP/1.1\r\nHost: attacker.example:{port}\r\nConnection: close\r\n\r\n")).await;
    assert_eq!(rebound.status, 403);
    let localhost = send(
        port,
        &format!("GET /api/status HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert_eq!(localhost.status, 200);
    let post = send(port, &format!("POST /api/summary HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")).await;
    assert_eq!((post.status, post.headers["allow"].as_str()), (405, "GET"));
}

#[tokio::test]
async fn the_servers_connection_cannot_write() {
    let fx = fixture().await;
    let db = api::open_read_only(&fx.db_path).await.unwrap();
    let conn = db.connection();
    let insert = "INSERT INTO projects (path, tokens_saved) VALUES ('/x', 1)";
    let error = conn.execute(insert, ()).await.unwrap_err().to_string();
    assert!(
        error.contains("readonly") || error.contains("read-only") || error.contains("query_only"),
        "{error}"
    );
    // The open flag holds on its own, with `query_only` switched off.
    conn.execute_batch("PRAGMA query_only = 0").await.unwrap();
    let error = conn.execute(insert, ()).await.unwrap_err().to_string();
    assert!(error.contains("readonly"), "{error}");
    let mut rows = conn
        .query("SELECT COUNT(*) FROM projects WHERE path = '/x'", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        0
    );
    drop(rows);

    // `query_only` is set too.
    let db = api::open_read_only(&fx.db_path).await.unwrap();
    let mut rows = db
        .connection()
        .query("PRAGMA query_only", ())
        .await
        .unwrap();
    assert_eq!(
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(),
        1
    );
}

#[tokio::test]
async fn a_missing_or_older_database_is_reported_and_left_alone() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("global.db");
    let error = api::open_read_only(&missing).await.err().unwrap();
    assert_eq!(error.status, 503);
    assert!(!missing.exists(), "the server created global.db");

    let fx = fixture().await;
    let db = libsql::Builder::new_local(&fx.db_path)
        .build()
        .await
        .unwrap();
    db.connect()
        .unwrap()
        .execute_batch("PRAGMA user_version = 1")
        .await
        .unwrap();
    drop(db);
    let (port, _stop) = serve(&fx, PAGE).await;
    let reply = get(port, "/api/summary").await;
    assert_eq!(reply.status, 503, "{}", reply.body);
    assert!(reply.json()["error"]
        .as_str()
        .unwrap()
        .contains("schema version 1"));
    // Status needs no database.
    assert_eq!(get(port, "/api/status").await.status, 200);
}
