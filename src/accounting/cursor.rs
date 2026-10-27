//! Cursor usage, read from Cursor's servers (inkcast #261).
//!
//! Cursor keeps no token counts on disk: measured 2026-10-27, none of the
//! keys in its chat stores (`~/.cursor/chats`) or agent transcripts names a
//! token, usage or cost. `tokensave cursor sync` therefore fetches them, and
//! only when the user runs it: never from a hook, the MCP server or an
//! ingest.
//!
//! Two sources, the documented one first:
//! - the Admin API, `POST api.cursor.com/teams/filtered-usage-events`, with a
//!   Team API key from [`ADMIN_KEY_ENV`] or a key file readable by its owner
//!   only (Teams and Enterprise plans);
//! - with `--use-login` only, the Cursor app's own login: its access token is
//!   read from the app's `state.vscdb` and sent as the dashboard's session
//!   cookie to `POST cursor.com/api/dashboard/get-filtered-usage-events`, the
//!   undocumented endpoint Cursor's dashboard calls (the request shape is
//!   tokscale's, MIT).
//!
//! A credential is held in a [`Secret`], whose `Debug` prints `[redacted]`,
//! and goes into nothing but the request's authorization header. Errors name
//! the source and the HTTP status only, and no redirect is followed.
//!
//! Cursor reports one event several times while it runs, each time with its
//! running totals, under one `usageEventId`. The largest snapshot is kept,
//! by total tokens and then cost, both within a fetch and against the stored
//! row ([`GlobalDb::upsert_cursor_turns`]).

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::accounting::codex::project_hash_of;
use crate::accounting::{classifier, pricing};
use crate::global_db::GlobalDb;
use crate::types::{CostTurn, TurnDetail};

/// The environment variable that holds a Cursor Admin API key.
pub const ADMIN_KEY_ENV: &str = "TOKENSAVE_CURSOR_ADMIN_KEY";
/// The key file in tokensave's data folder, read when the variable is unset.
pub const ADMIN_KEY_FILE: &str = "cursor-admin-key";

const ADMIN_ENDPOINT: &str = "https://api.cursor.com/teams/filtered-usage-events";
const DASHBOARD_ENDPOINT: &str = "https://cursor.com/api/dashboard/get-filtered-usage-events";
/// The Admin API's maximum page size.
const ADMIN_PAGE_SIZE: u64 = 1000;
/// tokscale's page size for the dashboard endpoint.
const DASHBOARD_PAGE_SIZE: u64 = 500;
/// A walk that has not ended after this many pages is stopped, not trusted.
const MAX_PAGES: u64 = 2000;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// The Admin API allows 60 requests a minute per team.
const ADMIN_PAGE_INTERVAL: Duration = Duration::from_secs(1);
/// How far before the newest recorded event a sync starts again: an event's
/// later snapshots, with larger totals, can arrive after it was first seen.
const CORRECTION_WINDOW_SECS: u64 = 3600;
/// The largest response body read, per page.
const MAX_PAGE_BYTES: u64 = 64 * 1024 * 1024;

/// Replaces both endpoints in test builds, so a test can serve recorded
/// responses locally. It does not exist in a release build: an environment
/// variable can never send a credential anywhere else there.
#[cfg(feature = "test-transport")]
const TEST_ENDPOINT_ENV: &str = "TOKENSAVE_CURSOR_TEST_ENDPOINT";

/// A credential. Never printed: its `Debug` is `[redacted]`, and it has no
/// `Display`.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Secret(String);

impl Secret {
    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

/// Where the usage came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// The documented Admin API, with a Team API key.
    AdminApi,
    /// The dashboard endpoint, with the Cursor app's login.
    Login,
}

impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::AdminApi => "Cursor Admin API",
            Self::Login => "Cursor login (dashboard endpoint)",
        }
    }
}

/// How a request is authorized.
#[derive(Debug, Clone)]
enum Auth {
    /// An Admin API key, sent as the Basic user name with no password.
    Admin(Secret),
    /// The dashboard's `WorkosCursorSessionToken` cookie value.
    Login(Secret),
}

impl Auth {
    fn kind(&self) -> SourceKind {
        match self {
            Self::Admin(_) => SourceKind::AdminApi,
            Self::Login(_) => SourceKind::Login,
        }
    }
}

/// Why a sync stopped. Every message names the source and, for a refused
/// request, the HTTP status; none carries a credential or a response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    /// The key file is readable by someone other than its owner.
    KeyFileMode(PathBuf),
    /// The key file exists but could not be read.
    KeyFileUnreadable(PathBuf),
    /// The Cursor app's login could not be found or read.
    LoginUnavailable(&'static str),
    /// The Cursor app's login has expired, by its own expiry or by a 401/403.
    LoginExpired,
    /// The Admin API refused the key (401/403).
    AdminKeyRejected,
    /// Any other status that is not a success.
    Http(SourceKind, u16),
    /// The request did not complete.
    Network(SourceKind, &'static str),
    /// A response that is not the expected JSON.
    BadResponse(SourceKind, &'static str),
    /// The walk did not end within [`MAX_PAGES`] pages.
    TooManyPages(SourceKind),
    /// The global database could not be read or written; nothing was written.
    Database,
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeyFileMode(path) => write!(
                f,
                "the Cursor Admin API key file {} must be readable by its owner only (chmod 600)",
                path.display()
            ),
            Self::KeyFileUnreadable(path) => write!(
                f,
                "the Cursor Admin API key file {} could not be read",
                path.display()
            ),
            Self::LoginUnavailable(why) => write!(f, "Cursor login not available: {why}"),
            Self::LoginExpired => {
                f.write_str("Cursor login expired: sign in to the Cursor app again")
            }
            Self::AdminKeyRejected => f.write_str("Cursor Admin API key rejected (HTTP 401/403)"),
            Self::Http(source, status) => write!(f, "{} returned HTTP {status}", source.label()),
            Self::Network(source, why) => write!(f, "{}: {why}", source.label()),
            Self::BadResponse(source, why) => {
                write!(f, "{} sent an unexpected response: {why}", source.label())
            }
            Self::TooManyPages(source) => write!(
                f,
                "{} kept returning pages after {MAX_PAGES}; nothing was written",
                source.label()
            ),
            Self::Database => {
                f.write_str("the global database could not be updated; nothing was written")
            }
        }
    }
}

impl std::error::Error for SyncError {}

/// What `tokensave cursor sync` was asked to do.
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// Fetch from this unix time instead of the newest recorded event.
    pub since: Option<u64>,
    /// Fetch and count, and write nothing.
    pub dry_run: bool,
    /// Allow the Cursor app's login when no Admin API key is set.
    pub use_login: bool,
    /// Admin API only: this user's events.
    pub email: Option<String>,
}

/// Where a sync reads its inputs from. Its `Debug` redacts the Admin key.
#[derive(Clone)]
pub struct SyncEnv {
    /// The user's home: `~/.cursor/chats` and the Cursor app's data are under it.
    pub home: PathBuf,
    /// tokensave's data folder, which may hold [`ADMIN_KEY_FILE`].
    pub data_home: PathBuf,
    /// The value of [`ADMIN_KEY_ENV`], read once by the caller.
    pub admin_key: Option<String>,
    pub admin_endpoint: String,
    pub dashboard_endpoint: String,
    /// Now, in unix milliseconds: the end of the window.
    pub now_ms: u64,
}

impl fmt::Debug for SyncEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncEnv")
            .field("home", &self.home)
            .field("data_home", &self.data_home)
            .field("admin_key", &self.admin_key.as_ref().map(|_| "[redacted]"))
            .field("admin_endpoint", &self.admin_endpoint)
            .field("dashboard_endpoint", &self.dashboard_endpoint)
            .field("now_ms", &self.now_ms)
            .finish()
    }
}

impl SyncEnv {
    /// The real environment: the user's home, tokensave's data folder, the
    /// key variable and Cursor's endpoints.
    pub fn from_environment() -> Option<Self> {
        let home = crate::agents::home_dir()?;
        let data_home = crate::data_home::tokensave_home(Some(home.clone()))?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis() as u64;
        #[allow(unused_mut)]
        let mut env = Self {
            home,
            data_home,
            admin_key: std::env::var(ADMIN_KEY_ENV).ok(),
            admin_endpoint: ADMIN_ENDPOINT.to_string(),
            dashboard_endpoint: DASHBOARD_ENDPOINT.to_string(),
            now_ms,
        };
        #[cfg(feature = "test-transport")]
        if let Ok(endpoint) = std::env::var(TEST_ENDPOINT_ENV) {
            env.admin_endpoint = endpoint.clone();
            env.dashboard_endpoint = endpoint;
        }
        Some(env)
    }
}

/// What a sync did, as counts. Nothing in it identifies the account.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncReport {
    pub source: SourceKind,
    pub dry_run: bool,
    /// The start of the window, in unix seconds; `None` for all history.
    pub start: Option<u64>,
    pub pages: u64,
    /// Rows returned by the server, every snapshot counted.
    pub events_fetched: u64,
    /// Rows with no model or no time, which are not usage and are skipped.
    pub events_skipped: u64,
    /// Rows older than the window, which a server may return anyway.
    pub events_before_window: u64,
    pub distinct_events: u64,
    /// Snapshots folded into a larger one of the same event.
    pub snapshots_collapsed: u64,
    pub inserted: u64,
    pub updated: u64,
    pub unchanged: u64,
    pub priced_by_cursor: u64,
    pub priced_locally: u64,
    pub unpriced: u64,
    /// Events whose conversation has a local chat folder, which gives the project.
    pub matched_to_chat: u64,
    /// Per model: events and the known cost.
    pub per_model: BTreeMap<String, (u64, f64)>,
}

/// The outcome of a sync that did not fail.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncOutcome {
    /// No source is set up; no request was made.
    NoSource,
    Synced(SyncReport),
}

/// How a turn's cost was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CostFrom {
    Cursor,
    LocalPrice,
    Unpriced,
}

/// One usage event, one snapshot of it.
#[derive(Debug, Clone, PartialEq)]
struct Event {
    id: String,
    timestamp_ms: u64,
    model: String,
    conversation_id: Option<String>,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost_usd: Option<f64>,
    cost_from: CostFrom,
}

impl Event {
    fn total_tokens(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    /// Whether this snapshot replaces `other`: more tokens, or as many and a
    /// higher cost (an unknown cost is below every known one).
    fn outranks(&self, other: &Self) -> bool {
        let cost = |event: &Self| event.cost_usd.unwrap_or(-1.0);
        self.total_tokens() > other.total_tokens()
            || (self.total_tokens() == other.total_tokens() && cost(self) > cost(other))
    }
}

/// A count or amount that Cursor may send as a number or a numeric string.
fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|n: &f64| n.is_finite())
}

fn count(value: Option<&Value>) -> u64 {
    number(value).map_or(0, |n| if n > 0.0 { n as u64 } else { 0 })
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The event a row of either endpoint describes, or `None` for a row with no
/// model or no time.
///
/// Tokens come from `tokenUsage`; a request-priced call has none. The cost
/// is Cursor's metered `tokenUsage.totalCents`, set even when the plan
/// included the call, else `chargedCents`, else the local price of the
/// model, else unknown.
fn parse_event(row: &Value) -> Option<Event> {
    let model = text(row.get("model"))?;
    let timestamp_ms = count(row.get("timestamp"));
    if timestamp_ms == 0 {
        return None;
    }
    let conversation_id = text(row.get("conversationId"));
    let usage = row.get("tokenUsage");
    let field = |key: &str| count(usage.and_then(|u| u.get(key)));
    let (input, output, cache_read, cache_write) = (
        field("inputTokens"),
        field("outputTokens"),
        field("cacheReadTokens"),
        field("cacheWriteTokens"),
    );

    let cents =
        number(usage.and_then(|u| u.get("totalCents"))).or_else(|| number(row.get("chargedCents")));
    let (cost_usd, cost_from) = match cents {
        Some(cents) => (Some(cents / 100.0), CostFrom::Cursor),
        None => match pricing::cost_of_turn(&model, input, output, cache_write, cache_read) {
            Some(cost) => (Some(cost), CostFrom::LocalPrice),
            None => (None, CostFrom::Unpriced),
        },
    };

    let id = text(row.get("usageEventId")).unwrap_or_else(|| {
        // The Admin API sends no event id: one is derived from what does not
        // change between snapshots of an event.
        let mut hasher = Sha256::new();
        for part in [
            timestamp_ms.to_string(),
            model.clone(),
            conversation_id.clone().unwrap_or_default(),
            text(row.get("userEmail")).unwrap_or_default(),
        ] {
            hasher.update(part.as_bytes());
            hasher.update([0]);
        }
        format!("h{}", &hex::encode(hasher.finalize())[..32])
    });

    Some(Event {
        id,
        timestamp_ms,
        model,
        conversation_id,
        input,
        output,
        cache_read,
        cache_write,
        cost_usd,
        cost_from,
    })
}

/// Folds the snapshots of each event into its largest, and returns the
/// events with the number of snapshots folded.
fn collapse(events: Vec<Event>) -> (Vec<Event>, u64) {
    let mut largest: HashMap<String, Event> = HashMap::new();
    let mut folded = 0;
    for event in events {
        match largest.get_mut(&event.id) {
            Some(kept) => {
                folded += 1;
                if event.outranks(kept) {
                    *kept = event;
                }
            }
            None => {
                largest.insert(event.id.clone(), event);
            }
        }
    }
    let mut events: Vec<Event> = largest.into_values().collect();
    events.sort_by(|a, b| (a.timestamp_ms, &a.id).cmp(&(b.timestamp_ms, &b.id)));
    (events, folded)
}

const BASE64_STANDARD: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding, for the Basic authorization header.
fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(BASE64_STANDARD[((n >> shift) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes base64url, with or without padding, as a JWT segment is written.
fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut held = 0u8;
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    for byte in text.trim_end_matches('=').bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        held += 6;
        if held >= 8 {
            held -= 8;
            out.push((bits >> held) as u8);
            bits &= (1 << held) - 1;
        }
    }
    Some(out)
}

/// The dashboard's session cookie value for a Cursor app access token:
/// `<user id>%3A%3A<token>`, the user id taken from the token's `sub` claim
/// (`auth0|user_…`). A token whose `exp` has passed is refused here, with no
/// request made.
fn session_cookie(access_token: &str, now_secs: u64) -> Result<Secret, SyncError> {
    let unreadable =
        SyncError::LoginUnavailable("the Cursor app's login token is not in a known form");
    let payload = access_token
        .split('.')
        .nth(1)
        .and_then(base64url_decode)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .ok_or(unreadable.clone())?;
    if payload
        .get("exp")
        .and_then(Value::as_u64)
        .is_some_and(|exp| exp <= now_secs)
    {
        return Err(SyncError::LoginExpired);
    }
    let sub = payload
        .get("sub")
        .and_then(Value::as_str)
        .ok_or(unreadable.clone())?;
    let user = &sub[sub.find("user_").ok_or(unreadable.clone())?..];
    let end = user
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(user.len());
    let user = &user[..end];
    if user.len() <= "user_".len() {
        return Err(unreadable);
    }
    Ok(Secret(format!("{user}%3A%3A{access_token}")))
}

/// The Cursor app's state database, where it keeps its login.
fn state_db_path(home: &Path) -> PathBuf {
    let relative = Path::new("Cursor/User/globalStorage/state.vscdb");
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support").join(relative)
    } else if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map_or_else(|| home.join("AppData/Roaming"), PathBuf::from)
            .join(relative)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map_or_else(|| home.join(".config"), PathBuf::from)
            .join(relative)
    }
}

/// What to open for the Cursor app's state database: its path, or, for a
/// WAL-mode database with no `-wal` file beside it (the app is not running,
/// so everything is in the main file), an immutable URI. A read-only open of
/// a WAL-mode database otherwise creates `-wal` and `-shm` files in the
/// app's folder and leaves them there; an immutable open takes no lock and
/// creates nothing. When a `-wal` file exists the app may be writing, so the
/// open is the ordinary read-only one: it reads under the app's locks and
/// updates the shared `-shm` index, as every reader does, and changes
/// neither the database nor its `-wal`. A rollback-journal database is opened
/// by its path, under SQLite's shared lock, which creates nothing.
fn state_db_target(state_db: &Path) -> PathBuf {
    use std::fmt::Write as _;
    use std::io::Read as _;
    let mut header = [0u8; 20];
    // Bytes 18 and 19 of the header are the file format versions: 2 is WAL.
    let wal_mode = std::fs::File::open(state_db)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok()
        && header[18] == 2;
    let mut wal = state_db.as_os_str().to_owned();
    wal.push("-wal");
    let Some(text) = state_db.to_str() else {
        return state_db.to_path_buf();
    };
    if !wal_mode || Path::new(&wal).exists() {
        return state_db.to_path_buf();
    }
    let mut uri = String::from("file:");
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri.push_str("?mode=ro&immutable=1");
    PathBuf::from(uri)
}

/// Reads the Cursor app's access token, opening its database read only.
async fn read_login_token(state_db: &Path) -> Result<Secret, SyncError> {
    if !state_db.is_file() {
        return Err(SyncError::LoginUnavailable(
            "the Cursor app's data was not found: install Cursor and sign in",
        ));
    }
    let unreadable = SyncError::LoginUnavailable("the Cursor app's data could not be read");
    let db = libsql::Builder::new_local(state_db_target(state_db))
        .flags(libsql::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .build()
        .await
        .map_err(|_| unreadable.clone())?;
    let conn = db.connect().map_err(|_| unreadable.clone())?;
    let mut rows = conn
        .query(
            "SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken'",
            (),
        )
        .await
        .map_err(|_| unreadable.clone())?;
    // VS Code's `ItemTable.value` is a BLOB column that usually holds text.
    let token = match rows.next().await.map_err(|_| unreadable.clone())? {
        Some(row) => match row.get_value(0).map_err(|_| unreadable.clone())? {
            libsql::Value::Text(text) => text,
            libsql::Value::Blob(bytes) => String::from_utf8(bytes).map_err(|_| unreadable)?,
            _ => String::new(),
        },
        None => String::new(),
    };
    let token = token.trim();
    if token.is_empty() {
        return Err(SyncError::LoginUnavailable(
            "the Cursor app is not signed in",
        ));
    }
    Ok(Secret(token.to_string()))
}

/// The Admin API key: the variable when set, else the key file, which must
/// be readable by its owner only.
fn admin_key(env: &SyncEnv) -> Result<Option<Secret>, SyncError> {
    if let Some(key) = env
        .admin_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
    {
        return Ok(Some(Secret(key.to_string())));
    }
    let path = env.data_home.join(ADMIN_KEY_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .map_err(|_| SyncError::KeyFileUnreadable(path.clone()))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(SyncError::KeyFileMode(path));
        }
    }
    let key =
        std::fs::read_to_string(&path).map_err(|_| SyncError::KeyFileUnreadable(path.clone()))?;
    let key = key.trim();
    Ok((!key.is_empty()).then(|| Secret(key.to_string())))
}

/// The source to use: the Admin API key when one is set, else the Cursor
/// app's login when allowed, else none.
async fn resolve_auth(env: &SyncEnv, options: &SyncOptions) -> Result<Option<Auth>, SyncError> {
    if let Some(key) = admin_key(env)? {
        return Ok(Some(Auth::Admin(key)));
    }
    if !options.use_login {
        return Ok(None);
    }
    let token = read_login_token(&state_db_path(&env.home)).await?;
    session_cookie(token.expose(), env.now_ms / 1000).map(|cookie| Some(Auth::Login(cookie)))
}

/// What the walk over the pages returned.
#[derive(Debug, Default)]
struct Fetched {
    rows: Vec<Value>,
    pages: u64,
}

/// One page request.
fn post_page(
    agent: &ureq::Agent,
    endpoint: &str,
    auth: &Auth,
    body: &Value,
) -> Result<Value, SyncError> {
    let kind = auth.kind();
    let request = agent.post(endpoint).header("Accept", "application/json");
    let request = match auth {
        Auth::Admin(key) => request.header(
            "Authorization",
            format!(
                "Basic {}",
                base64_encode(format!("{}:", key.expose()).as_bytes())
            ),
        ),
        Auth::Login(cookie) => request
            .header("Origin", "https://cursor.com")
            .header("Referer", "https://cursor.com/dashboard")
            .header(
                "Cookie",
                format!("WorkosCursorSessionToken={}", cookie.expose()),
            ),
    };
    let mut response = match request.send_json(body) {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(401 | 403)) => {
            return Err(match kind {
                SourceKind::Login => SyncError::LoginExpired,
                SourceKind::AdminApi => SyncError::AdminKeyRejected,
            })
        }
        Err(ureq::Error::StatusCode(status)) => return Err(SyncError::Http(kind, status)),
        Err(ureq::Error::Timeout(_)) => {
            return Err(SyncError::Network(kind, "the request timed out"))
        }
        Err(ureq::Error::HostNotFound) => {
            return Err(SyncError::Network(kind, "the host was not found"))
        }
        Err(_) => return Err(SyncError::Network(kind, "the request failed")),
    };
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        // A redirect, which is not followed.
        return Err(SyncError::Http(kind, status));
    }
    response
        .body_mut()
        .with_config()
        .limit(MAX_PAGE_BYTES)
        .read_json::<Value>()
        .map_err(|_| SyncError::BadResponse(kind, "the body is not JSON"))
}

/// Walks every page of the window `[start_ms, end_ms]`.
///
/// The walk ends at a short or empty page, once the reported total is
/// reached, when the Admin API says there is no next page, or when a page
/// holds only events before the window (a server that ignores the start).
/// It is an error for it not to end within [`MAX_PAGES`] pages.
fn fetch(
    agent: &ureq::Agent,
    endpoint: &str,
    auth: &Auth,
    start_ms: u64,
    end_ms: u64,
    email: Option<&str>,
) -> Result<Fetched, SyncError> {
    let kind = auth.kind();
    let (page_size, events_key) = match kind {
        SourceKind::AdminApi => (ADMIN_PAGE_SIZE, "usageEvents"),
        SourceKind::Login => (DASHBOARD_PAGE_SIZE, "usageEventsDisplay"),
    };
    let mut fetched = Fetched::default();
    for page in 1..=MAX_PAGES {
        if page > 1 && kind == SourceKind::AdminApi {
            std::thread::sleep(ADMIN_PAGE_INTERVAL);
        }
        let body = match kind {
            SourceKind::AdminApi => {
                let mut body = json!({
                    "startDate": start_ms,
                    "endDate": end_ms,
                    "page": page,
                    "pageSize": page_size,
                });
                if let Some(email) = email {
                    body["email"] = json!(email);
                }
                body
            }
            // The dashboard takes the dates as strings.
            SourceKind::Login => json!({
                "startDate": start_ms.to_string(),
                "endDate": end_ms.to_string(),
                "page": page,
                "pageSize": page_size,
            }),
        };
        let response = post_page(agent, endpoint, auth, &body)?;
        let rows = response
            .get(events_key)
            .and_then(Value::as_array)
            .ok_or(SyncError::BadResponse(kind, "no list of usage events"))?;
        fetched.pages = page;
        let received = rows.len() as u64;
        let all_before_window = !rows.is_empty()
            && rows.iter().all(|row| {
                count(row.get("timestamp")) < start_ms && count(row.get("timestamp")) > 0
            });
        fetched.rows.extend(rows.iter().cloned());

        let total = response
            .get("totalUsageEventsCount")
            .and_then(Value::as_u64);
        let no_next = response
            .pointer("/pagination/hasNextPage")
            .and_then(Value::as_bool)
            == Some(false);
        if received < page_size
            || total.is_some_and(|total| fetched.rows.len() as u64 >= total)
            || no_next
            || all_before_window
        {
            return Ok(fetched);
        }
    }
    Err(SyncError::TooManyPages(kind))
}

/// The working directory of each local chat, by conversation id: Cursor
/// keeps a chat in `~/.cursor/chats/<workspace hash>/<conversation id>/`,
/// with its `cwd` in `meta.json`. Read only; a chat that cannot be read is
/// left out.
fn chat_folders(home: &Path) -> HashMap<String, String> {
    let mut folders = HashMap::new();
    let Ok(workspaces) = std::fs::read_dir(home.join(".cursor").join("chats")) else {
        return folders;
    };
    for workspace in workspaces.flatten() {
        let Ok(chats) = std::fs::read_dir(workspace.path()) else {
            continue;
        };
        for chat in chats.flatten() {
            let Some(id) = chat.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let cwd = std::fs::read(chat.path().join("meta.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|meta| text(meta.get("cwd")));
            if let Some(cwd) = cwd {
                folders.insert(id, cwd);
            }
        }
    }
    folders
}

/// The turn an event becomes.
fn turn_of(event: &Event, folders: &HashMap<String, String>) -> CostTurn {
    let cwd = event
        .conversation_id
        .as_ref()
        .and_then(|id| folders.get(id))
        .map(String::as_str);
    CostTurn {
        message_id: format!("cursor:{}", event.id),
        project_hash: project_hash_of(cwd),
        session_id: event
            .conversation_id
            .clone()
            .unwrap_or_else(|| "cursor".to_string()),
        model: event.model.clone(),
        timestamp: event.timestamp_ms / 1000,
        input_tokens: event.input,
        output_tokens: event.output,
        cache_write_tokens: event.cache_write,
        cache_read_tokens: event.cache_read,
        cost_usd: event.cost_usd,
        // The server sends no tool calls: the turn counts as conversation.
        category: classifier::classify(&[], &[]).as_str().to_string(),
        tool_names: String::new(),
        agent: "cursor".to_string(),
        credits: None,
        detail: TurnDetail::default(),
    }
}

/// Fetches the Cursor usage of the window and records it as turns.
///
/// The window starts at `options.since`, else an hour before the newest
/// recorded Cursor turn, else at the beginning; it ends now. Every page is
/// fetched before anything is written, so a failed walk writes nothing, and
/// the writes are one transaction, rolled back on a dry run.
pub async fn sync(
    gdb: &GlobalDb,
    env: &SyncEnv,
    options: &SyncOptions,
) -> Result<SyncOutcome, SyncError> {
    let Some(auth) = resolve_auth(env, options).await? else {
        return Ok(SyncOutcome::NoSource);
    };
    let kind = auth.kind();
    let start = match options.since {
        Some(since) => Some(since),
        None => gdb
            .newest_turn_timestamp("cursor")
            .await
            .map(|newest| newest.saturating_sub(CORRECTION_WINDOW_SECS)),
    };
    let start_ms = start.unwrap_or(0) * 1000;
    let end_ms = env.now_ms;
    let endpoint = match kind {
        SourceKind::AdminApi => env.admin_endpoint.clone(),
        SourceKind::Login => env.dashboard_endpoint.clone(),
    };
    let email = options.email.clone();
    let fetched = tokio::task::spawn_blocking(move || {
        let agent = crate::cloud::agent_without_redirects(REQUEST_TIMEOUT);
        fetch(&agent, &endpoint, &auth, start_ms, end_ms, email.as_deref())
    })
    .await
    .map_err(|_| SyncError::Network(kind, "the request was interrupted"))??;

    let mut report = SyncReport {
        source: kind,
        dry_run: options.dry_run,
        start,
        pages: fetched.pages,
        events_fetched: fetched.rows.len() as u64,
        events_skipped: 0,
        events_before_window: 0,
        distinct_events: 0,
        snapshots_collapsed: 0,
        inserted: 0,
        updated: 0,
        unchanged: 0,
        priced_by_cursor: 0,
        priced_locally: 0,
        unpriced: 0,
        matched_to_chat: 0,
        per_model: BTreeMap::new(),
    };
    let mut events = Vec::with_capacity(fetched.rows.len());
    for row in &fetched.rows {
        match parse_event(row) {
            None => report.events_skipped += 1,
            Some(event) if event.timestamp_ms < start_ms => report.events_before_window += 1,
            Some(event) => events.push(event),
        }
    }
    let (events, folded) = collapse(events);
    report.distinct_events = events.len() as u64;
    report.snapshots_collapsed = folded;

    let folders = chat_folders(&env.home);
    let mut turns = Vec::with_capacity(events.len());
    for event in &events {
        match event.cost_from {
            CostFrom::Cursor => report.priced_by_cursor += 1,
            CostFrom::LocalPrice => report.priced_locally += 1,
            CostFrom::Unpriced => report.unpriced += 1,
        }
        if event
            .conversation_id
            .as_ref()
            .is_some_and(|id| folders.contains_key(id))
        {
            report.matched_to_chat += 1;
        }
        let model = report.per_model.entry(event.model.clone()).or_default();
        model.0 += 1;
        model.1 += event.cost_usd.unwrap_or(0.0);
        turns.push(turn_of(event, &folders));
    }

    let written = gdb
        .upsert_cursor_turns(&turns, options.dry_run)
        .await
        .ok_or(SyncError::Database)?;
    report.inserted = written.inserted;
    report.updated = written.updated;
    report.unchanged = written.unchanged;
    Ok(SyncOutcome::Synced(report))
}

/// Parses a `--since` date, `YYYY-MM-DD`, as midnight UTC.
pub fn parse_since(date: &str) -> Option<u64> {
    let bytes = date.as_bytes();
    let shape = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    let month: u32 = date.get(5..7)?.parse().ok()?;
    let day: u32 = date.get(8..10)?.parse().ok()?;
    if !shape || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    crate::accounting::parser::parse_timestamp(&format!("{date}T00:00:00Z"))
}

/// The start of a sync window for people to read: a UTC time, or "the
/// beginning" when all history was fetched.
#[must_use]
pub fn window_start_label(start: Option<u64>) -> String {
    match start {
        None => "the beginning".to_string(),
        Some(start) => i64::try_from(start).map_or_else(
            |_| format!("{start} (unix seconds)"),
            crate::blame_engine::format_rfc3339,
        ),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A fake Cursor app token: `sub` names a user, `exp` is in 2100.
    pub(crate) const FAKE_TOKEN: &str = "eyJhbGciOiJub25lIn0.eyJzdWIiOiJhdXRoMHx1c2VyX0ZBS0UwMDEiLCJleHAiOjQxMDI0NDQ4MDB9.FAKE-SIGNATURE-0123456789";

    /// Serves `responses` in order, one per connection, and records each
    /// request (head and body).
    fn serve(responses: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        serve_raw(
            responses
                .into_iter()
                .map(|(status, body)| {
                    format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                })
                .collect(),
        )
    }

    /// Serves each whole HTTP response in `responses`, one per connection,
    /// after reading the request in full.
    fn serve_raw(responses: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/usage", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        std::thread::spawn(move || {
            for response in responses {
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
                seen.lock().unwrap().push(format!(
                    "{head}\r\n{}",
                    String::from_utf8_lossy(&request_body)
                ));
                let _ = reader.into_inner().write_all(response.as_bytes());
            }
        });
        (url, requests)
    }

    fn row(id: &str, ts: u64, model: &str, input: u64, cents: f64) -> Value {
        json!({
            "usageEventId": id,
            "timestamp": ts.to_string(),
            "model": model,
            "conversationId": "conv-1",
            "chargedCents": cents,
            "tokenUsage": {"inputTokens": input, "outputTokens": 10, "cacheReadTokens": 100, "totalCents": cents},
        })
    }

    fn page(rows: &[Value], total: usize) -> String {
        json!({"totalUsageEventsCount": total, "usageEventsDisplay": rows}).to_string()
    }

    fn env_with(home: &Path, url: &str, admin_key: Option<&str>) -> SyncEnv {
        SyncEnv {
            home: home.to_path_buf(),
            data_home: home.join(".tokensave"),
            admin_key: admin_key.map(str::to_string),
            admin_endpoint: url.to_string(),
            dashboard_endpoint: url.to_string(),
            now_ms: 1_800_000_000_000,
        }
    }

    /// A Cursor app state database holding `token`.
    async fn fake_state_db(home: &Path, token: &str) {
        let path = state_db_path(home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = libsql::Builder::new_local(&path).build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('cursorAuth/accessToken', ?1)",
            libsql::params![token],
        )
        .await
        .unwrap();
    }

    async fn open_db(home: &Path) -> GlobalDb {
        GlobalDb::open_at(&home.join("global.db")).await.unwrap()
    }

    #[test]
    fn a_secret_never_prints() {
        let secret = Secret("s3cr3t-value".to_string());
        let auth = Auth::Login(secret.clone());
        assert!(!format!("{secret:?} {auth:?}").contains("s3cr3t"));
    }

    #[test]
    fn base64_round_trips_and_matches_the_standard() {
        assert_eq!(base64_encode(b"key:"), "a2V5Og==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
        assert_eq!(base64url_decode("YWJj").unwrap(), b"abc");
        assert_eq!(base64url_decode("YWI").unwrap(), b"ab");
        assert!(base64url_decode("a*b").is_none());
    }

    #[test]
    fn the_session_cookie_is_the_user_and_the_token() {
        let cookie = session_cookie(FAKE_TOKEN, 1_800_000_000).unwrap();
        assert_eq!(cookie.expose(), format!("user_FAKE001%3A%3A{FAKE_TOKEN}"));
    }

    #[test]
    fn an_expired_token_is_refused_without_a_request() {
        assert_eq!(
            session_cookie(FAKE_TOKEN, 4_102_444_800),
            Err(SyncError::LoginExpired)
        );
        assert!(matches!(
            session_cookie("not-a-jwt", 0),
            Err(SyncError::LoginUnavailable(_))
        ));
    }

    #[test]
    fn snapshots_of_an_event_keep_the_largest_in_either_order() {
        let small = parse_event(&row("e1", 1_000_000, "m", 10, 1.0)).unwrap();
        let large = parse_event(&row("e1", 1_000_000, "m", 20, 2.0)).unwrap();
        for order in [
            vec![small.clone(), large.clone()],
            vec![large.clone(), small.clone()],
        ] {
            let (events, folded) = collapse(order);
            assert_eq!((events.len(), folded), (1, 1));
            assert_eq!(events[0].input, 20);
            assert_eq!(events[0].cost_usd, Some(0.02));
        }
    }

    #[test]
    fn equal_tokens_keep_the_higher_cost() {
        let low = parse_event(&row("e1", 1_000_000, "m", 10, 1.0)).unwrap();
        let high = parse_event(&row("e1", 1_000_000, "m", 10, 3.0)).unwrap();
        let (events, _) = collapse(vec![high, low]);
        assert_eq!(events[0].cost_usd, Some(0.03));
    }

    #[test]
    fn the_cost_is_cursors_then_the_local_price_then_unknown() {
        let charged = json!({"timestamp": "1000", "model": "m", "chargedCents": 250});
        assert_eq!(parse_event(&charged).unwrap().cost_usd, Some(2.5));
        let unknown = json!({"timestamp": "1000", "model": "no-such-model-xyz",
            "tokenUsage": {"inputTokens": 5}});
        let event = parse_event(&unknown).unwrap();
        assert_eq!(
            (event.cost_usd, event.cost_from),
            (None, CostFrom::Unpriced)
        );
        assert!(parse_event(&json!({"timestamp": "1000"})).is_none());
        assert!(parse_event(&json!({"model": "m"})).is_none());
    }

    #[test]
    fn an_admin_event_without_an_id_gets_a_stable_one() {
        let row = json!({"timestamp": "1000", "model": "m", "userEmail": "a@b.c",
            "tokenUsage": {"inputTokens": 5, "totalCents": 1}});
        let first = parse_event(&row).unwrap();
        let mut later = row.clone();
        later["tokenUsage"]["inputTokens"] = json!(9);
        assert_eq!(first.id, parse_event(&later).unwrap().id);
    }

    #[test]
    fn since_is_a_calendar_date() {
        assert_eq!(parse_since("2026-10-01"), Some(1_790_812_800));
        assert_eq!(parse_since("2026-13-01"), None);
        assert_eq!(parse_since("2026-1-01"), None);
        assert_eq!(parse_since("yesterday"), None);
    }

    #[test]
    fn the_window_start_reads_as_a_utc_time() {
        assert_eq!(window_start_label(None), "the beginning");
        assert_eq!(
            window_start_label(Some(1_789_996_500)),
            "2026-09-21T13:15:00Z"
        );
    }

    #[tokio::test]
    async fn with_no_source_nothing_is_requested() {
        let home = tempfile::TempDir::new().unwrap();
        fake_state_db(home.path(), FAKE_TOKEN).await;
        let (url, requests) = serve(vec![(200, page(&[], 0))]);
        let db = open_db(home.path()).await;
        let outcome = sync(
            &db,
            &env_with(home.path(), &url, None),
            &SyncOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(outcome, SyncOutcome::NoSource);
        assert!(requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_login_source_sends_the_cookie_and_stores_one_turn_per_event() {
        let home = tempfile::TempDir::new().unwrap();
        fake_state_db(home.path(), FAKE_TOKEN).await;
        let chat = home.path().join(".cursor/chats/ws/conv-1");
        std::fs::create_dir_all(&chat).unwrap();
        std::fs::write(chat.join("meta.json"), r#"{"cwd":"/work/repo"}"#).unwrap();
        let rows = vec![
            row("e1", 1_700_000_000_000, "grok-4.7-xhigh", 10, 1.0),
            row("e1", 1_700_000_000_000, "grok-4.7-xhigh", 30, 3.0),
            row("e2", 1_700_000_001_000, "grok-4.7-high", 5, 0.5),
        ];
        let (url, requests) = serve(vec![(200, page(&rows, 3))]);
        let db = open_db(home.path()).await;
        let options = SyncOptions {
            use_login: true,
            ..SyncOptions::default()
        };
        let SyncOutcome::Synced(report) = sync(&db, &env_with(home.path(), &url, None), &options)
            .await
            .unwrap()
        else {
            panic!("no source");
        };
        assert_eq!(report.source, SourceKind::Login);
        assert_eq!(
            (
                report.events_fetched,
                report.distinct_events,
                report.snapshots_collapsed
            ),
            (3, 2, 1)
        );
        assert_eq!(
            (
                report.inserted,
                report.matched_to_chat,
                report.priced_by_cursor
            ),
            (2, 2, 2)
        );
        let seen = requests.lock().unwrap().join("\n");
        assert!(seen.contains(&format!(
            "WorkosCursorSessionToken=user_FAKE001%3A%3A{FAKE_TOKEN}"
        )));
        assert!(
            seen.contains("Origin: https://cursor.com")
                || seen.contains("origin: https://cursor.com")
        );
        // ureq pretty-prints the body.
        let compact: String = seen.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact.contains(r#""startDate":"0""#));
        assert!(compact.contains(r#""endDate":"1800000000000""#));

        let stored = db.cursor_turn_row("cursor:e1").await.unwrap();
        assert_eq!(
            stored,
            (Some(0.03), 30 + 10 + 100, "-work-repo".to_string())
        );
    }

    #[tokio::test]
    async fn a_later_smaller_snapshot_never_replaces_a_stored_larger_one() {
        let home = tempfile::TempDir::new().unwrap();
        fake_state_db(home.path(), FAKE_TOKEN).await;
        let db = open_db(home.path()).await;
        let options = SyncOptions {
            use_login: true,
            since: Some(0),
            ..SyncOptions::default()
        };
        for (input, cents) in [(30, 3.0), (10, 1.0)] {
            let (url, _) = serve(vec![(
                200,
                page(&[row("e1", 1_000_000, "m", input, cents)], 1),
            )]);
            sync(&db, &env_with(home.path(), &url, None), &options)
                .await
                .unwrap();
        }
        let stored = db.cursor_turn_row("cursor:e1").await.unwrap();
        assert_eq!((stored.0, stored.1), (Some(0.03), 30 + 10 + 100));
        let (url, _) = serve(vec![(200, page(&[row("e1", 1_000_000, "m", 50, 5.0)], 1))]);
        let SyncOutcome::Synced(report) = sync(&db, &env_with(home.path(), &url, None), &options)
            .await
            .unwrap()
        else {
            panic!("no source");
        };
        assert_eq!((report.inserted, report.updated), (0, 1));
        assert_eq!(
            db.cursor_turn_row("cursor:e1").await.unwrap().1,
            50 + 10 + 100
        );
    }

    #[tokio::test]
    async fn a_redirect_is_not_followed_so_the_cookie_stays_with_its_host() {
        let home = tempfile::TempDir::new().unwrap();
        fake_state_db(home.path(), FAKE_TOKEN).await;
        let (elsewhere, elsewhere_requests) = serve(vec![(200, page(&[], 0))]);
        // A server that redirects to `elsewhere`.
        let (redirecting, _) = serve_raw(vec![format!(
            "HTTP/1.1 302 Found\r\nLocation: {elsewhere}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )]);
        let db = open_db(home.path()).await;
        let options = SyncOptions {
            use_login: true,
            ..SyncOptions::default()
        };
        let error = sync(&db, &env_with(home.path(), &redirecting, None), &options)
            .await
            .unwrap_err();
        assert_eq!(error, SyncError::Http(SourceKind::Login, 302));
        assert!(elsewhere_requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_rejected_login_writes_nothing() {
        let home = tempfile::TempDir::new().unwrap();
        fake_state_db(home.path(), FAKE_TOKEN).await;
        let (url, _) = serve(vec![(401, "{}".to_string())]);
        let db = open_db(home.path()).await;
        let options = SyncOptions {
            use_login: true,
            ..SyncOptions::default()
        };
        let error = sync(&db, &env_with(home.path(), &url, None), &options)
            .await
            .unwrap_err();
        assert_eq!(error, SyncError::LoginExpired);
        assert_eq!(
            error.to_string(),
            "Cursor login expired: sign in to the Cursor app again"
        );
        assert_eq!(db.newest_turn_timestamp("cursor").await, None);
    }

    #[tokio::test]
    async fn the_admin_key_wins_and_pages_until_there_is_no_next() {
        let home = tempfile::TempDir::new().unwrap();
        let admin_page = |rows: Vec<Value>, next: bool| {
            json!({"usageEvents": rows, "pagination": {"hasNextPage": next}}).to_string()
        };
        let full: Vec<Value> = (0..ADMIN_PAGE_SIZE)
            .map(|i| {
                json!({"timestamp": (1_000_000 + i).to_string(), "model": "m",
                "userEmail": "a@b.c", "tokenUsage": {"inputTokens": 1, "totalCents": 1}})
            })
            .collect();
        let (url, requests) = serve(vec![
            (200, admin_page(full, true)),
            (
                200,
                admin_page(
                    vec![json!({"timestamp": "5000000", "model": "m", "chargedCents": 2})],
                    false,
                ),
            ),
        ]);
        fake_state_db(home.path(), FAKE_TOKEN).await;
        let db = open_db(home.path()).await;
        let options = SyncOptions {
            use_login: true,
            dry_run: true,
            ..SyncOptions::default()
        };
        let SyncOutcome::Synced(report) = sync(
            &db,
            &env_with(home.path(), &url, Some("admin-key-1")),
            &options,
        )
        .await
        .unwrap() else {
            panic!("no source");
        };
        assert_eq!(report.source, SourceKind::AdminApi);
        assert_eq!(
            (report.pages, report.distinct_events),
            (2, ADMIN_PAGE_SIZE + 1)
        );
        let seen = requests.lock().unwrap().join("\n");
        assert!(seen.contains(&format!("Basic {}", base64_encode(b"admin-key-1:"))));
        assert!(!seen.contains("WorkosCursorSessionToken"));
        // A dry run rolls its writes back.
        assert_eq!(report.inserted, ADMIN_PAGE_SIZE + 1);
        assert_eq!(db.newest_turn_timestamp("cursor").await, None);
    }

    #[cfg(unix)]
    #[test]
    fn a_key_file_readable_by_others_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::TempDir::new().unwrap();
        let env = env_with(home.path(), "http://127.0.0.1:9", None);
        std::fs::create_dir_all(&env.data_home).unwrap();
        let path = env.data_home.join(ADMIN_KEY_FILE);
        std::fs::write(&path, "key\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(admin_key(&env), Err(SyncError::KeyFileMode(path.clone())));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(admin_key(&env), Ok(Some(Secret("key".to_string()))));
    }

    // ---- OB3 harness (review copy only) ----

    fn ob3_snapshot(dir: &Path) -> Vec<(String, u64, String)> {
        let mut out: Vec<(String, u64, String)> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| {
                let bytes = std::fs::read(e.path()).unwrap_or_default();
                (
                    e.file_name().to_string_lossy().to_string(),
                    bytes.len() as u64,
                    hex::encode(Sha256::digest(&bytes)),
                )
            })
            .collect();
        out.sort();
        out
    }

    async fn ob3_make_db(home: &Path, wal: bool) -> PathBuf {
        let path = state_db_path(home);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = libsql::Builder::new_local(&path).build().await.unwrap();
        let conn = db.connect().unwrap();
        if wal {
            conn.query("PRAGMA journal_mode = WAL", ()).await.unwrap();
        }
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('cursorAuth/accessToken', ?1)",
            libsql::params![FAKE_TOKEN],
        )
        .await
        .unwrap();
        drop(conn);
        drop(db);
        path
    }

    #[tokio::test]
    async fn ob3_read_leaves_the_state_db_byte_identical() {
        for wal in [false, true] {
            let home = tempfile::TempDir::new().unwrap();
            let path = ob3_make_db(home.path(), wal).await;
            let dir = path.parent().unwrap().to_path_buf();
            let before = ob3_snapshot(&dir);
            let token = read_login_token(&path).await;
            let after = ob3_snapshot(&dir);
            println!("OB3 wal={wal} before={before:?}");
            println!("OB3 wal={wal} after ={after:?} ok={}", token.is_ok());
            assert!(token.is_ok());
            assert_eq!(before, after, "wal={wal}: the read changed the folder");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ob3_read_needs_no_write_permission() {
        use std::os::unix::fs::PermissionsExt;
        for wal in [false, true] {
            let home = tempfile::TempDir::new().unwrap();
            let path = ob3_make_db(home.path(), wal).await;
            let dir = path.parent().unwrap().to_path_buf();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
            let token = read_login_token(&path).await;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            println!(
                "OB3 readonly-fs wal={wal} ok={} err={:?}",
                token.is_ok(),
                token.as_ref().err()
            );
            assert!(token.is_ok(), "wal={wal}");
        }
    }

    #[tokio::test]
    async fn ob3_read_while_the_app_holds_a_write_transaction() {
        for wal in [false, true] {
            let home = tempfile::TempDir::new().unwrap();
            let path = ob3_make_db(home.path(), wal).await;
            // The "Cursor app": an open connection inside a write transaction.
            let app = libsql::Builder::new_local(&path).build().await.unwrap();
            let app_conn = app.connect().unwrap();
            app_conn.execute("BEGIN IMMEDIATE", ()).await.unwrap();
            app_conn
                .execute("INSERT INTO ItemTable (key, value) VALUES ('k', 'v')", ())
                .await
                .unwrap();
            let start = std::time::Instant::now();
            let token = read_login_token(&path).await;
            let took = start.elapsed();
            let commit = app_conn.execute("COMMIT", ()).await;
            println!(
                "OB3 concurrent wal={wal} read_ok={} took={took:?} app_commit_ok={}",
                token.is_ok(),
                commit.is_ok()
            );
            assert!(token.is_ok());
            assert!(commit.is_ok(), "the app could not commit after the read");
        }
    }

    #[test]
    fn ob3_sync_env_debug_prints_the_admin_key() {
        let home = tempfile::TempDir::new().unwrap();
        let env = env_with(
            home.path(),
            "http://127.0.0.1:9",
            Some("key_OB3-ADMIN-SECRET"),
        );
        let printed = format!("{env:?}");
        println!("OB3 SyncEnv Debug = {printed}");
        assert!(
            !printed.contains("OB3-ADMIN-SECRET"),
            "SyncEnv's Debug prints the Admin key"
        );
    }

    #[test]
    fn ob3_full_pages_forever_stop_at_max_pages() {
        let full: Vec<Value> = (0..DASHBOARD_PAGE_SIZE).map(|_| json!({})).collect();
        let body =
            json!({"totalUsageEventsCount": 1_000_000_000_000u64, "usageEventsDisplay": full,
            "pagination": {"hasNextPage": true}})
            .to_string();
        let (url, requests) = serve(vec![(200, body); (MAX_PAGES + 5) as usize]);
        let agent = crate::cloud::agent_without_redirects(REQUEST_TIMEOUT);
        let auth = Auth::Login(Secret("x".into()));
        let start = std::time::Instant::now();
        let result = fetch(&agent, &url, &auth, 0, 1, None);
        let n = requests.lock().unwrap().len();
        println!(
            "OB3 forever: result_err={:?} requests={n} took={:?}",
            result.as_ref().err(),
            start.elapsed()
        );
        assert_eq!(
            result.unwrap_err(),
            SyncError::TooManyPages(SourceKind::Login)
        );
        assert_eq!(n as u64, MAX_PAGES);
    }

    #[tokio::test]
    async fn ob3_upsert_never_lowers_a_stored_row() {
        let home = tempfile::TempDir::new().unwrap();
        let db = open_db(home.path()).await;
        let mut folders = HashMap::new();
        folders.insert("conv-1".to_string(), "/work/repo".to_string());
        let ev = |input: u64, cents: Option<f64>| {
            let mut r = row("e1", 1_000_000, "m", input, 0.0);
            if let Some(c) = cents {
                r["chargedCents"] = json!(c);
                r["tokenUsage"]["totalCents"] = json!(c);
            } else {
                r.as_object_mut().unwrap().remove("chargedCents");
                r["tokenUsage"]
                    .as_object_mut()
                    .unwrap()
                    .remove("totalCents");
                r["model"] = json!("no-such-model-ob3");
            }
            parse_event(&r).unwrap()
        };
        let big = turn_of(&ev(30, Some(3.0)), &folders);
        assert_eq!(
            db.upsert_cursor_turns(&[big], false)
                .await
                .unwrap()
                .inserted,
            1
        );
        let stored = db.cursor_turn_row("cursor:e1").await.unwrap();
        // smaller tokens, higher cost; equal tokens, lower cost; equal tokens, unknown cost; no project
        let mut no_project = turn_of(&ev(30, Some(1.0)), &HashMap::new());
        no_project.project_hash = "unknown".into();
        for (label, t) in [
            (
                "smaller-tokens-higher-cost",
                turn_of(&ev(10, Some(99.0)), &folders),
            ),
            (
                "equal-tokens-lower-cost",
                turn_of(&ev(30, Some(1.0)), &folders),
            ),
            (
                "equal-tokens-unknown-cost",
                turn_of(&ev(30, None), &folders),
            ),
            ("no-project-lower", no_project),
        ] {
            let w = db.upsert_cursor_turns(&[t], false).await.unwrap();
            let now = db.cursor_turn_row("cursor:e1").await.unwrap();
            println!("OB3 upsert {label}: {w:?} row={now:?}");
            assert_eq!(w.unchanged, 1, "{label}");
            assert_eq!(now, stored, "{label}");
        }
        // A larger snapshot with no project keeps the stored project.
        let mut larger = turn_of(&ev(40, Some(4.0)), &HashMap::new());
        larger.project_hash = "unknown".into();
        let w = db.upsert_cursor_turns(&[larger], false).await.unwrap();
        let now = db.cursor_turn_row("cursor:e1").await.unwrap();
        println!("OB3 upsert larger-no-project: {w:?} row={now:?}");
        assert_eq!((w.updated, now.2.as_str()), (1, "-work-repo"));
    }

    #[test]
    fn ob3_huge_token_counts_do_not_panic() {
        let r = json!({"usageEventId": "e", "timestamp": "1000", "model": "m",
            "tokenUsage": {"inputTokens": 1e30, "outputTokens": 1e30, "totalCents": 1}});
        let a = parse_event(&r).unwrap();
        let outcome = std::panic::catch_unwind(|| collapse(vec![a.clone(), a.clone()]));
        println!("OB3 huge tokens: collapse panicked = {}", outcome.is_err());
        assert!(outcome.is_ok(), "collapse panicked on huge token counts");
    }
}
