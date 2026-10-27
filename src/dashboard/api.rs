//! The dashboard's API: one function per endpoint, each answering from
//! `global.db` on a read-only connection with the figures computed in SQL.
//!
//! The JSON shapes are the TypeScript types of the mockup's datasets
//! (inkcast `docs/tooling/tokensave-dashboard/mockups/source/Main.dc.html`),
//! field for field: `camelCase` names, `null` where a type allows it.
//!
//! Every query on `turns` binds the same seven values ([`Params::values`]),
//! and every one filters with the same clause: the window (`?1`, `?2`), repo
//! (`?3`), agent (`?4`), model (`?5`) and effort (`?6`), each skipped when
//! NULL, and only the agents the API names (`AND agent IN (...)`). Queries on
//! `savings_ledger` filter by the window and the repo's ledger key (`?7`):
//! the ledger records no agent, model or effort, so those filters do not
//! apply to saved tokens. A unit test checks every statement carries its
//! clause word for word.
//!
//! Spent tokens are input + output + cache write + cache read (output
//! includes thinking for Claude Code and reasoning for Codex). A turn whose
//! model had no price has `cost_usd` NULL (inkcast #252): sums leave it out
//! and say how many they left out, and a cost that would understate is
//! `null`, never `0`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use libsql::{Connection, Value};
use serde::Serialize;

use super::params::{self, Params, EFFORTS};
use super::sql;

/// A subagent with no turn for this long is no longer `running`.
pub const RUNNING_SECS: i64 = 15 * 60;

/// Why a request failed, and the HTTP status that says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
    pub parameter: Option<String>,
}

impl ApiError {
    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            status: 503,
            message: message.into(),
            parameter: None,
        }
    }

    fn query(error: &libsql::Error) -> Self {
        Self {
            status: 500,
            message: format!("query failed: {error}"),
            parameter: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: 500,
            message: message.into(),
            parameter: None,
        }
    }
}

impl From<params::ParamError> for ApiError {
    fn from(error: params::ParamError) -> Self {
        Self {
            status: 400,
            message: format!("`{}` {}", error.parameter, error.message),
            parameter: Some(error.parameter),
        }
    }
}

/// A read-only connection to `global.db`.
pub struct ReadOnlyDb {
    conn: Connection,
    _db: libsql::Database,
}

impl ReadOnlyDb {
    /// The connection, for a caller that needs to prove it is read only.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

/// Opens `global.db` at `path` read only: opened with
/// `SQLITE_OPEN_READ_ONLY`, so no statement can write it, and with
/// `query_only` set as well. A missing file is not created, and a schema
/// older than this binary's is refused rather than migrated, since the
/// dashboard never writes the database (a newer one is read, inkcast #248).
pub async fn open_read_only(path: &Path) -> Result<ReadOnlyDb, ApiError> {
    if !path.is_file() {
        return Err(ApiError::unavailable(format!(
            "no usage is recorded yet: {} does not exist",
            path.display()
        )));
    }
    let unavailable = |error: libsql::Error| {
        ApiError::unavailable(format!("cannot open {}: {error}", path.display()))
    };
    let db = libsql::Builder::new_local(path)
        .flags(libsql::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .build()
        .await
        .map_err(unavailable)?;
    let conn = db.connect().map_err(unavailable)?;
    conn.execute_batch("PRAGMA query_only = 1; PRAGMA busy_timeout = 5000;")
        .await
        .map_err(unavailable)?;
    let mut rows = conn
        .query("PRAGMA user_version", ())
        .await
        .map_err(unavailable)?;
    let version = match rows.next().await.map_err(unavailable)? {
        Some(row) => row.get::<i64>(0).map_err(unavailable)?,
        None => 0,
    };
    let latest = i64::from(crate::global_db::SCHEMA_VERSION);
    if version < latest {
        return Err(ApiError::unavailable(format!(
            "{} is at schema version {version}, older than this tokensave reads ({latest}); \
             run `tokensave cost` once to upgrade it",
            path.display()
        )));
    }
    Ok(ReadOnlyDb { conn, _db: db })
}

/// What the endpoints need besides the request.
#[derive(Debug, Clone)]
pub struct Context {
    pub db_path: PathBuf,
    pub bind: std::net::IpAddr,
    pub port: u16,
    pub default_range_days: u32,
    /// The user's home folder, where the agent tools keep their data.
    pub home: Option<PathBuf>,
    /// Codex's data folder (`CODEX_HOME`, else `~/.codex`).
    pub codex_home: Option<PathBuf>,
    /// The current unix time, in seconds.
    pub now: fn() -> i64,
}

// ── JSON types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct Status {
    pub bind: String,
    pub port: u16,
    pub workspaces: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub spent_tokens: i64,
    pub cost_usd: f64,
    pub unpriced_turns: i64,
    pub saved_tokens: i64,
    pub saving_rate: f64,
    pub cache_hit_rate: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    pub date: String,
    pub spent_tokens: i64,
    pub saved_tokens: i64,
}

#[derive(Debug, Serialize)]
pub struct ToolShare {
    pub tool: &'static str,
    pub tokens: i64,
    pub share: f64,
}

#[derive(Debug, Serialize)]
pub struct ModelShare {
    pub model: String,
    pub tokens: i64,
    pub share: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mix {
    pub by_tool: Vec<ToolShare>,
    pub by_model: Vec<ModelShare>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopRun {
    pub id: String,
    pub name: String,
    pub role: String,
    pub repo: String,
    pub model: String,
    pub effort: Option<String>,
    pub tokens: i64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub role: String,
    pub tool: &'static str,
    pub model: String,
    pub effort: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub state: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleShare {
    pub role: String,
    pub cost_usd: f64,
    pub share: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleCost {
    pub roles: Vec<RoleShare>,
    pub subagent_share: f64,
    pub unfinished_runs: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRow {
    pub model: String,
    pub cost_usd: Vec<Option<f64>>,
}

#[derive(Debug, Serialize)]
pub struct ModelEffort {
    pub efforts: Vec<&'static str>,
    pub rows: Vec<ModelRow>,
}

#[derive(Debug, Serialize)]
pub struct Thinking {
    pub model: String,
    pub effort: Option<String>,
    pub share: f64,
}

#[derive(Debug, Serialize)]
pub struct FallbackPair {
    pub requested: String,
    pub served: String,
    pub turns: i64,
    pub sessions: i64,
}

#[derive(Debug, Serialize)]
pub struct Fallbacks {
    pub pairs: Vec<FallbackPair>,
    pub rate: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub path: String,
    pub marker: String,
    pub scan_depth: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    pub name: String,
    pub path: String,
    pub indexed: bool,
    pub spent_tokens: Option<i64>,
    pub saved_tokens: Option<i64>,
    pub tools: Vec<&'static str>,
    pub last_activity: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Repos {
    /// `null` until workspaces exist (inkcast #227, S2).
    pub workspace: Option<Workspace>,
    pub repos: Vec<Repo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedByTool {
    pub tool: String,
    pub saved_tokens: i64,
    pub share: f64,
}

#[derive(Debug, Serialize)]
pub struct Missed {
    pub kind: &'static str,
    pub tokens: i64,
    pub turns: i64,
    pub instead: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Savings {
    pub saved_tokens: i64,
    pub before_tokens: i64,
    pub after_tokens: i64,
    pub graph_calls: i64,
    pub missed_tokens: i64,
    pub by_tool: Vec<SavedByTool>,
    pub missed: Vec<Missed>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub tool: &'static str,
    pub path: String,
    pub turns: Option<i64>,
    pub last_parsed: Option<String>,
    pub status: &'static str,
    pub note: &'static str,
}

// ── Row helpers ───────────────────────────────────────────────────────────

fn value(row: &libsql::Row, index: i32) -> Result<Value, ApiError> {
    row.get_value(index)
        .map_err(|error| ApiError::query(&error))
}

fn int(row: &libsql::Row, index: i32) -> Result<i64, ApiError> {
    Ok(opt_int(row, index)?.unwrap_or(0))
}

fn opt_int(row: &libsql::Row, index: i32) -> Result<Option<i64>, ApiError> {
    match value(row, index)? {
        Value::Null => Ok(None),
        Value::Integer(v) => Ok(Some(v)),
        Value::Real(v) => Ok(Some(v.round() as i64)),
        other => Err(ApiError::internal(format!(
            "column {index} is {other:?}, not a number"
        ))),
    }
}

fn real(row: &libsql::Row, index: i32) -> Result<f64, ApiError> {
    Ok(opt_real(row, index)?.unwrap_or(0.0))
}

fn opt_real(row: &libsql::Row, index: i32) -> Result<Option<f64>, ApiError> {
    match value(row, index)? {
        Value::Null => Ok(None),
        Value::Integer(v) => Ok(Some(v as f64)),
        Value::Real(v) => Ok(Some(v)),
        other => Err(ApiError::internal(format!(
            "column {index} is {other:?}, not a number"
        ))),
    }
}

fn opt_text(row: &libsql::Row, index: i32) -> Result<Option<String>, ApiError> {
    match value(row, index)? {
        Value::Null => Ok(None),
        Value::Text(v) => Ok(Some(v)),
        other => Err(ApiError::internal(format!(
            "column {index} is {other:?}, not text"
        ))),
    }
}

fn text(row: &libsql::Row, index: i32) -> Result<String, ApiError> {
    Ok(opt_text(row, index)?.unwrap_or_default())
}

/// Runs `sql` with `values` and maps every row with `map`.
async fn rows<T>(
    db: &ReadOnlyDb,
    sql: &str,
    values: Vec<Value>,
    mut map: impl FnMut(&libsql::Row) -> Result<T, ApiError>,
) -> Result<Vec<T>, ApiError> {
    let mut rows = db
        .conn
        .query(sql, values)
        .await
        .map_err(|error| ApiError::query(&error))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await.map_err(|error| ApiError::query(&error))? {
        out.push(map(&row)?);
    }
    Ok(out)
}

/// The API name of an agent the turn filter let through.
fn tool_of(agent: &str) -> Result<&'static str, ApiError> {
    params::tool_name(agent).ok_or_else(|| ApiError::internal(format!("unknown agent {agent:?}")))
}

/// The tools named in a `group_concat` of agents, in the API's tool order.
fn tools_of(agents: Option<&str>) -> Vec<&'static str> {
    let agents: Vec<&str> = agents.unwrap_or("").split(',').collect();
    params::TOOLS
        .iter()
        .filter(|(_, agent)| agents.contains(agent))
        .map(|(name, _)| *name)
        .collect()
}

/// A unix time as ISO 8601 in UTC.
fn iso(unix_secs: i64) -> String {
    crate::blame_engine::format_rfc3339(unix_secs)
}

/// The repo name of each project-hash bucket, from the `projects` registry.
async fn repo_names(db: &ReadOnlyDb) -> Result<HashMap<String, String>, ApiError> {
    let paths = registry(db).await?;
    Ok(paths
        .into_iter()
        .map(|path| (params::project_hash_of(&path), repo_name(&path)))
        .collect())
}

/// Every repo path in the `projects` registry.
async fn registry(db: &ReadOnlyDb) -> Result<Vec<String>, ApiError> {
    rows(db, sql::REGISTRY, Vec::new(), |row| text(row, 0)).await
}

fn repo_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(
        || path.to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

// ── Endpoints ─────────────────────────────────────────────────────────────

/// `GET /api/status`.
pub fn status(ctx: &Context) -> Status {
    Status {
        bind: ctx.bind.to_string(),
        port: ctx.port,
        // Workspaces arrive with `tokensave workspace` (inkcast #227, S2).
        workspaces: Vec::new(),
    }
}

/// `GET /api/summary`.
pub async fn summary(db: &ReadOnlyDb, params: &Params) -> Result<Summary, ApiError> {
    let mut out = rows(db, sql::SUMMARY, params.values(7), |row| {
        Ok(Summary {
            spent_tokens: int(row, 0)?,
            cost_usd: real(row, 1)?,
            unpriced_turns: int(row, 2)?,
            saved_tokens: int(row, 3)?,
            saving_rate: real(row, 4)?,
            cache_hit_rate: real(row, 5)?,
        })
    })
    .await?;
    out.pop()
        .ok_or_else(|| ApiError::internal("the summary query returned no row"))
}

/// `GET /api/daily`: one row per day of the window, days without data at 0.
pub async fn daily(db: &ReadOnlyDb, params: &Params) -> Result<Vec<Day>, ApiError> {
    rows(db, sql::DAILY, params.values(7), |row| {
        Ok(Day {
            date: text(row, 0)?,
            spent_tokens: int(row, 1)?,
            saved_tokens: int(row, 2)?,
        })
    })
    .await
}

/// `GET /api/mix`: spent tokens by agent tool and by model, largest first.
pub async fn mix(db: &ReadOnlyDb, params: &Params) -> Result<Mix, ApiError> {
    let by_tool = rows(db, sql::MIX_BY_TOOL, params.values(6), |row| {
        Ok(ToolShare {
            tool: tool_of(&text(row, 0)?)?,
            tokens: int(row, 1)?,
            share: real(row, 2)?,
        })
    })
    .await?;
    let by_model = rows(db, sql::MIX_BY_MODEL, params.values(6), |row| {
        Ok(ModelShare {
            model: text(row, 0)?,
            tokens: int(row, 1)?,
            share: real(row, 2)?,
        })
    })
    .await?;
    Ok(Mix { by_tool, by_model })
}

/// One run as the runs query returns it.
struct RunRow {
    id: String,
    parent: Option<String>,
    label: Option<String>,
    role: String,
    agent: String,
    model: String,
    effort: Option<String>,
    prompt: i64,
    output: i64,
    cost: Option<f64>,
    project_hash: String,
    state: String,
}

/// Every run in the window, the costliest first: a top-level session (its
/// turns carry no run id) or a subagent run (its turns carry the run id).
///
/// A run's model and effort are those of its turns with the most tokens.
/// Its role is `main` for a session; for a subagent, `Explore`, `builder` or
/// `reviewer` from its type (`agent_runs.role`) or, failing that, from words
/// in its description ("review"; "build", "implement"), else `other`. Its
/// state is `finished` when its record says so; else `running` when it had a
/// turn in the last [`RUNNING_SECS`]; else `finished` for a session (a
/// session records no end) and `partial` for a subagent, whose record never
/// said it finished. Claude Code writes `finished` only once a run ends, so a
/// stopped run cannot be told from one whose record was cut short, and
/// `stopped` is never returned.
///
/// `?8` is the row limit (`-1` for all) and `?9` the time a turn must be at
/// or after for its run to be `running`.
async fn runs_query(
    db: &ReadOnlyDb,
    params: &Params,
    limit: i64,
    now: i64,
) -> Result<Vec<RunRow>, ApiError> {
    let mut values = params.values(7);
    values.push(Value::Integer(limit));
    values.push(Value::Integer(now - RUNNING_SECS));
    rows(db, sql::RUNS, values, |row| {
        Ok(RunRow {
            id: text(row, 0)?,
            parent: opt_text(row, 1)?,
            label: opt_text(row, 2)?,
            role: text(row, 3)?,
            agent: text(row, 4)?,
            model: text(row, 5)?,
            effort: opt_text(row, 6)?,
            prompt: int(row, 7)?,
            output: int(row, 8)?,
            cost: opt_real(row, 9)?,
            project_hash: text(row, 10)?,
            state: text(row, 11)?,
        })
    })
    .await
}

/// A run's display name: a subagent's description (else its type), or the
/// repo of a session.
fn run_name(run: &RunRow, repo: &str) -> String {
    match (&run.label, run.role.as_str()) {
        (_, "main") => format!("{repo} · session"),
        (Some(label), _) => label.clone(),
        (None, _) => "subagent".to_string(),
    }
}

/// `GET /api/runs/top`: the costliest runs, `limit` of them (5 by default).
pub async fn top_runs(
    db: &ReadOnlyDb,
    params: &Params,
    ctx: &Context,
) -> Result<Vec<TopRun>, ApiError> {
    let limit = i64::from(params.limit.unwrap_or(5));
    let names = repo_names(db).await?;
    let runs = runs_query(db, params, limit, (ctx.now)()).await?;
    Ok(runs
        .into_iter()
        .map(|run| {
            let repo = names
                .get(&run.project_hash)
                .cloned()
                .unwrap_or_else(|| run.project_hash.clone());
            TopRun {
                name: run_name(&run, &repo),
                id: run.id,
                role: run.role,
                repo,
                model: run.model,
                effort: run.effort,
                tokens: run.prompt + run.output,
                cost_usd: run.cost,
            }
        })
        .collect())
}

/// `GET /api/runs`: every run, each session followed by the runs it
/// started (nested runs after their parent), sessions the costliest first.
/// A run whose parent is not in the list stands at the top level.
pub async fn runs(db: &ReadOnlyDb, params: &Params, ctx: &Context) -> Result<Vec<Run>, ApiError> {
    let names = repo_names(db).await?;
    let rows = runs_query(db, params, -1, (ctx.now)()).await?;
    let ids: std::collections::HashSet<&str> = rows.iter().map(|run| run.id.as_str()).collect();
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for (index, run) in rows.iter().enumerate() {
        match run
            .parent
            .as_deref()
            .filter(|parent| ids.contains(parent) && *parent != run.id)
        {
            Some(parent) => children.entry(parent).or_default().push(index),
            None => roots.push(index),
        }
    }
    let mut order = Vec::with_capacity(rows.len());
    let mut stack: Vec<usize> = roots.into_iter().rev().collect();
    let mut placed = vec![false; rows.len()];
    while let Some(index) = stack.pop() {
        if std::mem::replace(&mut placed[index], true) {
            continue;
        }
        order.push(index);
        if let Some(kids) = children.get(rows[index].id.as_str()) {
            stack.extend(kids.iter().rev());
        }
    }
    // A parent cycle has no root to reach it from; list its runs last.
    order.extend((0..rows.len()).filter(|index| !placed[*index]));

    order
        .into_iter()
        .map(|index| {
            let run = &rows[index];
            let repo = names
                .get(&run.project_hash)
                .map_or(run.project_hash.as_str(), String::as_str);
            Ok(Run {
                id: run.id.clone(),
                parent_id: run.parent.clone(),
                name: run_name(run, repo),
                role: run.role.clone(),
                tool: tool_of(&run.agent)?,
                model: run.model.clone(),
                effort: run.effort.clone(),
                input_tokens: run.prompt,
                output_tokens: Some(run.output),
                cost_usd: run.cost,
                state: run.state.clone(),
            })
        })
        .collect()
}

/// `GET /api/roles`: priced cost by role, the share of it subagents spent,
/// and how many subagent runs are not `finished` (see [`runs_query`]).
pub async fn roles(db: &ReadOnlyDb, params: &Params, ctx: &Context) -> Result<RoleCost, ApiError> {
    let mut values = params.values(7);
    values.push(Value::Integer(-1));
    values.push(Value::Integer((ctx.now)() - RUNNING_SECS));
    let mut subagent_share = 0.0;
    let mut unfinished_runs = 0;
    let roles = rows(db, sql::ROLES, values, |row| {
        subagent_share = real(row, 3)?;
        unfinished_runs = int(row, 4)?;
        Ok(RoleShare {
            role: text(row, 0)?,
            cost_usd: real(row, 1)?,
            share: real(row, 2)?,
        })
    })
    .await?;
    Ok(RoleCost {
        roles,
        subagent_share,
        unfinished_runs,
    })
}

/// `GET /api/models`: priced cost per model and effort, one column per
/// effort level; `null` where the model has no turn at that effort or a
/// turn there has no price. Turns of an unknown effort are left out; models
/// are ordered by their priced cost, largest first.
pub async fn models(db: &ReadOnlyDb, params: &Params) -> Result<ModelEffort, ApiError> {
    let cells = rows(db, sql::MODEL_EFFORT, params.values(6), |row| {
        Ok((text(row, 0)?, text(row, 1)?, opt_real(row, 2)?))
    })
    .await?;
    let mut rows_out: Vec<ModelRow> = Vec::new();
    for (model, effort, cost) in cells {
        if rows_out.last().is_none_or(|row| row.model != model) {
            rows_out.push(ModelRow {
                model: model.clone(),
                cost_usd: vec![None; EFFORTS.len()],
            });
        }
        let column = EFFORTS
            .iter()
            .position(|level| *level == effort)
            .ok_or_else(|| ApiError::internal(format!("unknown effort {effort:?}")))?;
        if let Some(row) = rows_out.last_mut() {
            row.cost_usd[column] = cost;
        }
    }
    Ok(ModelEffort {
        efforts: EFFORTS.to_vec(),
        rows: rows_out,
    })
}

/// `GET /api/thinking`: thinking tokens over output tokens per model and
/// effort, over the turns whose thinking tokens were recorded.
pub async fn thinking(db: &ReadOnlyDb, params: &Params) -> Result<Vec<Thinking>, ApiError> {
    rows(db, sql::THINKING, params.values(6), |row| {
        Ok(Thinking {
            model: text(row, 0)?,
            effort: opt_text(row, 1)?,
            share: real(row, 2)?,
        })
    })
    .await
}

/// `GET /api/fallbacks`: turns served by another model than the one asked
/// for, by pair, and their share of the turns whose requested model was
/// recorded. A requested model's context suffix (`claude-opus-5-5[1m]`)
/// names the same model and is not a fallback.
pub async fn fallbacks(db: &ReadOnlyDb, params: &Params) -> Result<Fallbacks, ApiError> {
    let mut rate = 0.0;
    let pairs = rows(db, sql::FALLBACKS, params.values(6), |row| {
        rate = real(row, 4)?;
        Ok(FallbackPair {
            requested: text(row, 0)?,
            served: text(row, 1)?,
            turns: int(row, 2)?,
            sessions: int(row, 3)?,
        })
    })
    .await?;
    Ok(Fallbacks { pairs, rate })
}

/// `GET /api/repos`: every repo in the `projects` registry, with the tokens
/// spent in it in the window (its turns' working folder is the repo), the
/// tokens tokensave saved there, the agent tools that worked there, and its
/// latest turn or tokensave call at any time. A repo is indexed when its
/// `.tokensave/tokensave.db` exists. Spent and saved are `null` when the
/// window holds nothing for them.
pub async fn repos(db: &ReadOnlyDb, params: &Params) -> Result<Repos, ApiError> {
    let spent: HashMap<String, (i64, Vec<&'static str>)> =
        rows(db, sql::REPO_SPENT, params.values(6), |row| {
            Ok((
                text(row, 0)?,
                (int(row, 1)?, tools_of(opt_text(row, 2)?.as_deref())),
            ))
        })
        .await?
        .into_iter()
        .collect();
    let saved: HashMap<String, i64> = rows(db, sql::REPO_SAVED, params.values(7), |row| {
        Ok((text(row, 0)?, int(row, 1)?))
    })
    .await?
    .into_iter()
    .collect();
    let last_turn: HashMap<String, i64> = rows(db, sql::REPO_LAST_TURN, Vec::new(), |row| {
        Ok((text(row, 0)?, int(row, 1)?))
    })
    .await?
    .into_iter()
    .collect();
    let last_call: HashMap<String, i64> = rows(db, sql::REPO_LAST_CALL, Vec::new(), |row| {
        Ok((text(row, 0)?, int(row, 1)?))
    })
    .await?
    .into_iter()
    .collect();

    let mut repos: Vec<Repo> = registry(db)
        .await?
        .into_iter()
        .map(|path| {
            let hash = params::project_hash_of(&path);
            let (spent_tokens, tools) = spent
                .get(&hash)
                .map_or((None, Vec::new()), |(tokens, tools)| {
                    (Some(*tokens), tools.clone())
                });
            let last = last_turn
                .get(&hash)
                .copied()
                .max(last_call.get(&path).copied());
            Repo {
                name: repo_name(&path),
                indexed: Path::new(&path)
                    .join(".tokensave")
                    .join("tokensave.db")
                    .is_file(),
                spent_tokens,
                saved_tokens: saved.get(&path).copied(),
                tools,
                last_activity: last.map(iso),
                path,
            }
        })
        .collect();
    repos.sort_by(|a, b| {
        b.spent_tokens
            .unwrap_or(-1)
            .cmp(&a.spent_tokens.unwrap_or(-1))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(Repos {
        workspace: None,
        repos,
    })
}

/// `GET /api/savings`: tokensave's savings in the window, by tool, and the
/// savings missed: Claude Code turns `tokensave discover` finds a graph call
/// could have replaced, with its conservative recoverable estimate.
pub async fn savings(db: &ReadOnlyDb, params: &Params) -> Result<Savings, ApiError> {
    let totals = rows(db, sql::SAVINGS, params.values(7), |row| {
        Ok((int(row, 0)?, int(row, 1)?, int(row, 2)?, int(row, 3)?))
    })
    .await?;
    let (saved_tokens, before_tokens, after_tokens, graph_calls) =
        totals.first().copied().unwrap_or_default();
    let by_tool = rows(db, sql::SAVED_BY_TOOL, params.values(7), |row| {
        Ok(SavedByTool {
            tool: text(row, 0)?,
            saved_tokens: int(row, 1)?,
            share: real(row, 2)?,
        })
    })
    .await?;
    let nav = rows(db, sql::NAV_TURNS, params.values(3), |row| {
        Ok((text(row, 0)?, int(row, 1)?.max(0) as u64))
    })
    .await?;
    let report = crate::accounting::discover::analyze(&nav);
    let missed = report
        .buckets
        .iter()
        .map(|bucket| Missed {
            kind: match bucket.bucket {
                crate::accounting::discover::NavBucket::Read => "Whole-file reads",
                crate::accounting::discover::NavBucket::Grep => "Symbol searches with grep",
                crate::accounting::discover::NavBucket::Glob => "File discovery with glob",
            },
            tokens: bucket.recoverable_input_tokens() as i64,
            turns: bucket.turns as i64,
            instead: bucket.bucket.suggestion(),
        })
        .collect();
    Ok(Savings {
        saved_tokens,
        before_tokens,
        after_tokens,
        graph_calls,
        missed_tokens: report.total_recoverable_input_tokens() as i64,
        by_tool,
        missed,
    })
}

/// `GET /api/sources`: each agent tool, where its data is, how many turns
/// tokensave recorded from it and its newest one. `verified` when its data
/// is found and turns were recorded from it; `unknown` when its data is
/// found but nothing was recorded (its format is not checked here: the
/// server reads only the database); `not found` otherwise. Cursor has no
/// local data: its usage comes from the Cursor account.
pub async fn sources(db: &ReadOnlyDb, ctx: &Context) -> Result<Vec<Source>, ApiError> {
    let recorded: HashMap<String, (i64, Option<String>)> =
        rows(db, sql::SOURCE_TURNS, Vec::new(), |row| {
            Ok((text(row, 0)?, (int(row, 1)?, opt_text(row, 2)?)))
        })
        .await?
        .into_iter()
        .collect();
    let home = ctx.home.as_deref();
    let under_home = |parts: &[&str]| {
        home.map(|home| {
            parts
                .iter()
                .fold(home.to_path_buf(), |path, part| path.join(part))
        })
    };
    // `None` for a source with no local data folder.
    let sources: [(&str, Option<Option<PathBuf>>, &str); 5] = [
        (
            "claude",
            Some(under_home(&[".claude", "projects"])),
            "Model, requested model, effort, thinking, subagent roles.",
        ),
        (
            "codex",
            Some(ctx.codex_home.as_ref().map(|dir| dir.join("sessions"))),
            "One turn per response: model and effort from turn context, tokens from its usage record.",
        ),
        (
            "cursor",
            None,
            "Usage per request from your Cursor account (tokensave cursor sync).",
        ),
        (
            "pi",
            Some(under_home(&[".pi", "agent", "sessions"])),
            "Model and tokens per message, priced by Pi.",
        ),
        (
            "droid",
            Some(under_home(&[".factory", "sessions"])),
            "Model, tokens and credits per session.",
        ),
    ];
    sources
        .into_iter()
        .map(|(agent, folder, note)| {
            let (count, newest) = recorded.get(agent).cloned().unwrap_or((0, None));
            let (path, found) = match folder {
                Some(dir) => {
                    let found = dir.as_deref().is_some_and(Path::is_dir);
                    let path = dir.map_or_else(
                        || "(no home folder)".to_string(),
                        |dir| dir.display().to_string(),
                    );
                    (path, found)
                }
                None => (
                    "Cursor account (tokensave cursor sync)".to_string(),
                    count > 0,
                ),
            };
            let status = match (found, count > 0) {
                (true, true) => "verified",
                (true, false) => "unknown",
                (false, _) => "not found",
            };
            Ok(Source {
                tool: tool_of(agent)?,
                path,
                turns: (found || count > 0).then_some(count),
                last_parsed: newest,
                status,
                note,
            })
        })
        .collect()
}
