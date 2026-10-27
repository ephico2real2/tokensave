//! The API's query parameters: parsed, validated, and turned into the values
//! every query binds.
//!
//! A parameter an endpoint does not take, a repeated one, or a value it cannot
//! use is refused with a [`ParamError`] that names it; the server answers 400.
//! An empty value (`?repo=`) means the parameter is absent, as the page writes
//! "all" that way.
//!
//! Days are UTC calendar days, as in tokensave's other reports (`cost`,
//! `gain`): a preset of N days is the N days ending today, and a custom range
//! is `from` to `to`, both included.

use libsql::Value;

/// Seconds in a day.
const DAY: i64 = 86_400;

/// The agent tools the dashboard shows, by the name the API uses, with the
/// `turns.agent` value each one's turns carry.
pub const TOOLS: [(&str, &str); 5] = [
    ("Claude Code", "claude"),
    ("Codex", "codex"),
    ("Cursor", "cursor"),
    ("Pi", "pi"),
    ("Factory Droid", "droid"),
];

/// The API name of a `turns.agent` value, or `None` for an agent the
/// dashboard does not show.
pub fn tool_name(agent: &str) -> Option<&'static str> {
    TOOLS
        .iter()
        .find(|(_, value)| *value == agent)
        .map(|(name, _)| *name)
}

/// The effort levels, in the order the API lists them.
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The most runs `/api/runs/top` returns.
pub const MAX_LIMIT: u32 = 100;

/// A parameter the server refuses: 400, naming it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamError {
    pub parameter: String,
    pub message: String,
}

impl ParamError {
    fn new(parameter: &str, message: impl Into<String>) -> Self {
        Self {
            parameter: parameter.to_string(),
            message: message.into(),
        }
    }
}

/// The days a request covers, both included, as days since 1970-01-01 (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub first_day: i64,
    pub last_day: i64,
}

impl Window {
    /// The first second of the window.
    pub fn start(&self) -> i64 {
        self.first_day * DAY
    }

    /// The first second after the window.
    pub fn end(&self) -> i64 {
        (self.last_day + 1) * DAY
    }

    /// The number of days in the window.
    pub fn days(&self) -> i64 {
        self.last_day - self.first_day + 1
    }
}

/// A repo the request is limited to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The key its tokensave calls are recorded under in `savings_ledger`.
    pub ledger_key: String,
    /// The bucket its agent turns are recorded under in `turns.project_hash`.
    pub project_hash: String,
}

/// Everything a request asked for, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Params {
    pub window: Window,
    pub repo: Option<Repo>,
    /// A `turns.agent` value.
    pub agent: Option<&'static str>,
    pub model: Option<String>,
    pub effort: Option<&'static str>,
    pub limit: Option<u32>,
}

impl Params {
    /// The values bound to `?1`..`?7` of the dashboard's queries: window
    /// start, window end, project hash, agent, model, effort, ledger key.
    /// The first `count` are returned, as a statement takes no more values
    /// than its highest parameter number.
    pub fn values(&self, count: usize) -> Vec<Value> {
        let text = |value: Option<&str>| value.map_or(Value::Null, |v| Value::Text(v.to_string()));
        let all = [
            Value::Integer(self.window.start()),
            Value::Integer(self.window.end()),
            text(self.repo.as_ref().map(|r| r.project_hash.as_str())),
            text(self.agent),
            text(self.model.as_deref()),
            text(self.effort),
            text(self.repo.as_ref().map(|r| r.ledger_key.as_str())),
        ];
        all.into_iter().take(count).collect()
    }
}

/// Parses `query` (the URL's query string, without `?`) for an endpoint that
/// takes the parameters `allowed`. `today` is the current UTC day, and
/// `default_days` the preset used when the request names no range.
pub fn parse(
    query: Option<&str>,
    allowed: &[&str],
    today: i64,
    default_days: u32,
) -> Result<Params, ParamError> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    for part in query
        .unwrap_or("")
        .split('&')
        .filter(|part| !part.is_empty())
    {
        let (raw_name, raw_value) = part.split_once('=').unwrap_or((part, ""));
        let name = decode(raw_name).ok_or_else(|| ParamError::new(raw_name, "is not UTF-8"))?;
        let value = decode(raw_value).ok_or_else(|| ParamError::new(&name, "is not UTF-8"))?;
        if !allowed.contains(&name.as_str()) {
            return Err(ParamError::new(
                &name,
                "is not a parameter of this endpoint",
            ));
        }
        if pairs.iter().any(|(seen, _)| *seen == name) {
            return Err(ParamError::new(&name, "is given more than once"));
        }
        if !value.is_empty() {
            pairs.push((name, value));
        }
    }
    let get = |name: &str| {
        pairs
            .iter()
            .find(|(seen, _)| seen == name)
            .map(|(_, value)| value.as_str())
    };

    let window = window(get("range"), get("from"), get("to"), today, default_days)?;
    let repo = get("repo").map(parse_repo).transpose()?;
    let agent = get("tool")
        .map(|value| {
            TOOLS
                .iter()
                .find(|(name, _)| *name == value)
                .map(|(_, agent)| *agent)
                .ok_or_else(|| {
                    ParamError::new(
                        "tool",
                        "must be one of Claude Code, Codex, Cursor, Pi, Factory Droid",
                    )
                })
        })
        .transpose()?;
    let model = get("model")
        .map(|value| {
            if value.chars().count() > 200 {
                Err(ParamError::new("model", "is longer than 200 characters"))
            } else {
                Ok(value.to_string())
            }
        })
        .transpose()?;
    let effort = get("effort")
        .map(|value| {
            EFFORTS
                .iter()
                .find(|effort| **effort == value)
                .copied()
                .ok_or_else(|| {
                    ParamError::new("effort", "must be one of low, medium, high, xhigh, max")
                })
        })
        .transpose()?;
    let limit = get("limit")
        .map(|value| {
            value
                .parse::<u32>()
                .ok()
                .filter(|limit| (1..=MAX_LIMIT).contains(limit))
                .ok_or_else(|| {
                    ParamError::new(
                        "limit",
                        format!("must be a whole number from 1 to {MAX_LIMIT}"),
                    )
                })
        })
        .transpose()?;
    if let Some(workspace) = get("workspace") {
        // Workspaces arrive with `tokensave workspace` (inkcast #227, S2);
        // until then there is none to name.
        return Err(ParamError::new(
            "workspace",
            format!("names no workspace: {workspace:?} is not one this machine has"),
        ));
    }
    Ok(Params {
        window,
        repo,
        agent,
        model,
        effort,
        limit,
    })
}

/// Percent-decodes a query-string component, `+` being a space.
fn decode(raw: &str) -> Option<String> {
    let spaced = raw.replace('+', " ");
    percent_encoding::percent_decode_str(&spaced)
        .decode_utf8()
        .ok()
        .map(std::borrow::Cow::into_owned)
}

fn window(
    range: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    today: i64,
    default_days: u32,
) -> Result<Window, ParamError> {
    let custom = match range {
        Some("custom") => true,
        Some(value) => {
            let days = crate::dashboard::config::parse_preset(value).ok_or_else(|| {
                ParamError::new("range", "must be one of 7, 30, 45, 60, 90 or custom")
            })?;
            if from.is_some() || to.is_some() {
                let name = if from.is_some() { "from" } else { "to" };
                return Err(ParamError::new(name, "is only taken with range=custom"));
            }
            return Ok(preset(today, days));
        }
        None => from.is_some() || to.is_some(),
    };
    if !custom {
        return Ok(preset(today, default_days));
    }
    let from = from.ok_or_else(|| ParamError::new("from", "is needed for a custom range"))?;
    let to = to.ok_or_else(|| ParamError::new("to", "is needed for a custom range"))?;
    let first_day =
        parse_day(from).ok_or_else(|| ParamError::new("from", "must be a date, YYYY-MM-DD"))?;
    let last_day =
        parse_day(to).ok_or_else(|| ParamError::new("to", "must be a date, YYYY-MM-DD"))?;
    // The page offers the viewer's local date, which east of UTC is up to a
    // day ahead of the UTC day (UTC+14 at most): refusing it would fail every
    // custom range there from local midnight until UTC midnight.
    if last_day > today + 1 {
        return Err(ParamError::new("to", "is after today"));
    }
    if first_day > last_day {
        return Err(ParamError::new("from", "is after to"));
    }
    Ok(Window {
        first_day,
        last_day,
    })
}

fn preset(today: i64, days: u32) -> Window {
    Window {
        first_day: today - i64::from(days) + 1,
        last_day: today,
    }
}

/// A `YYYY-MM-DD` date as days since 1970-01-01, or `None` when it is not
/// one (a wrong shape, month or day, or a year before 1970).
pub fn parse_day(value: &str) -> Option<i64> {
    if value.len() != 10 || !value.is_ascii() {
        return None;
    }
    let seconds = crate::accounting::parser::parse_timestamp(&format!("{value}T00:00:00Z"))?;
    Some(seconds as i64 / DAY)
}

/// The UTC day of a unix time.
pub fn day_of(unix_secs: i64) -> i64 {
    unix_secs.div_euclid(DAY)
}

/// A repo path given as `repo`: an absolute path, keyed as tokensave keys it.
fn parse_repo(value: &str) -> Result<Repo, ParamError> {
    if !std::path::Path::new(value).is_absolute() {
        return Err(ParamError::new(
            "repo",
            "must be a repo's absolute path, as /api/repos lists it",
        ));
    }
    let ledger_key = crate::global_db::normalize_project_key(value);
    Ok(Repo {
        project_hash: project_hash_of(&ledger_key),
        ledger_key,
    })
}

/// The `turns.project_hash` bucket of a repo path: Claude Code's project
/// folder name for it, as the Codex adapter writes it too.
pub fn project_hash_of(path: &str) -> String {
    crate::accounting::codex::project_hash_of(Some(path))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const ALL: &[&str] = &[
        "range",
        "from",
        "to",
        "repo",
        "tool",
        "model",
        "effort",
        "limit",
        "workspace",
    ];
    // 2026-10-27
    const TODAY: i64 = 20_753;

    /// A repo path that is absolute on this platform, percent-encoded as the
    /// page sends it, and its project bucket.
    #[cfg(not(windows))]
    const REPO: (&str, &str) = ("%2Fw%2Finkcast.app", "-w-inkcast-app");
    #[cfg(windows)]
    const REPO: (&str, &str) = ("C%3A%5Cw%5Cinkcast.app", "C--w-inkcast-app");

    fn ok(query: &str) -> Params {
        parse(Some(query), ALL, TODAY, 30).unwrap()
    }

    fn err(query: &str) -> String {
        parse(Some(query), ALL, TODAY, 30).unwrap_err().parameter
    }

    #[test]
    fn today_is_the_day_the_tests_assume() {
        assert_eq!(parse_day("2026-10-27"), Some(TODAY));
    }

    #[test]
    fn a_preset_is_the_days_ending_today() {
        let window = ok("range=7").window;
        assert_eq!(
            (window.first_day, window.last_day, window.days()),
            (TODAY - 6, TODAY, 7)
        );
        assert_eq!(ok("range=90d").window.days(), 90);
        assert_eq!(ok("").window.days(), 30);
        assert_eq!(parse(None, ALL, TODAY, 45).unwrap().window.days(), 45);
        assert_eq!(
            ok("range=7").window.end() - ok("range=7").window.start(),
            7 * DAY
        );
    }

    #[test]
    fn a_custom_range_includes_both_days() {
        let window = ok("range=custom&from=2026-10-01&to=2026-10-26").window;
        assert_eq!(window.days(), 26);
        assert_eq!(window.start(), parse_day("2026-10-01").unwrap() * DAY);
        assert_eq!(window.end(), parse_day("2026-10-27").unwrap() * DAY);
        assert_eq!(ok("from=2026-10-27&to=2026-10-27").window.days(), 1);
    }

    #[test]
    fn a_custom_range_may_end_on_the_viewers_local_today_east_of_utc() {
        // At 2026-10-27 21:00 UTC it is already 2026-10-28 in UTC+3 to UTC+14.
        let window = ok("range=custom&from=2026-09-29&to=2026-10-28").window;
        assert_eq!((window.first_day, window.last_day), (TODAY - 28, TODAY + 1));
    }

    #[test]
    fn a_wrong_range_names_its_parameter() {
        assert_eq!(err("range=14"), "range");
        assert_eq!(err("range=custom&to=2026-10-01"), "from");
        assert_eq!(err("range=custom&from=2026-10-01"), "to");
        assert_eq!(err("range=30&from=2026-10-01&to=2026-10-02"), "from");
        assert_eq!(err("from=2026-10-29&to=2026-10-29"), "to");
        assert_eq!(err("from=2026-10-02&to=2026-10-01"), "from");
        assert_eq!(err("from=2026-02-30&to=2026-10-01"), "from");
        assert_eq!(err("from=26-10-01&to=2026-10-01"), "from");
        assert_eq!(err("from=1969-12-31&to=2026-10-01"), "from");
    }

    #[test]
    fn filters_are_checked_and_mapped() {
        let params = ok(&format!(
            "tool=Claude+Code&effort=xhigh&model=claude-opus-5-5&repo={}&limit=5",
            REPO.0
        ));
        assert_eq!(params.agent, Some("claude"));
        assert_eq!(params.effort, Some("xhigh"));
        assert_eq!(params.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(params.limit, Some(5));
        assert_eq!(params.repo.unwrap().project_hash, REPO.1);
        assert_eq!(err("tool=claude"), "tool");
        assert_eq!(err("effort=minimal"), "effort");
        assert_eq!(err("limit=0"), "limit");
        assert_eq!(err("limit=101"), "limit");
        assert_eq!(err("repo=inkcast"), "repo");
        assert_eq!(err("workspace=~/gitRepos"), "workspace");
    }

    #[test]
    fn an_empty_value_is_absent() {
        let params = ok("repo=&tool=&model=&effort=&workspace=");
        assert_eq!(
            (params.repo, params.agent, params.model, params.effort),
            (None, None, None, None)
        );
    }

    #[test]
    fn an_unknown_or_repeated_parameter_is_refused() {
        assert_eq!(
            parse(Some("model=x"), &["range", "from", "to"], TODAY, 30)
                .unwrap_err()
                .parameter,
            "model"
        );
        assert_eq!(err("tool=Codex&tool=Pi"), "tool");
        assert_eq!(err("colour=red"), "colour");
        assert_eq!(err("model=%FF"), "model");
    }

    #[test]
    fn values_bind_in_the_documented_order() {
        let params = ok(&format!("range=7&tool=Codex&repo={}", REPO.0));
        let values = params.values(7);
        assert_eq!(values.len(), 7);
        assert_eq!(values[0], Value::Integer((TODAY - 6) * DAY));
        assert_eq!(values[1], Value::Integer((TODAY + 1) * DAY));
        assert_eq!(values[3], Value::Text("codex".into()));
        assert_eq!(values[4], Value::Null);
        assert_eq!(params.values(2).len(), 2);
    }
}
