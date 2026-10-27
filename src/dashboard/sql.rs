//! Every SQL statement the dashboard runs, on a read-only connection.
//!
//! The statements bind `?1`..`?7` as [`super::params::Params::values`]
//! orders them. Each one that reads `turns` for a figure filters with the
//! same clause (window, repo, agent, model, effort, and only the agents the
//! API names), and each that reads `savings_ledger` with the same clause
//! (window, repo); the tests below check every statement carries its clause
//! word for word, so the copies cannot drift. [`NAV_TURNS`] takes the window
//! and repo only, as `/api/savings` does, and Claude Code's turns, the only
//! ones `tokensave discover` reads.

/// Every repo path in the `projects` registry.
pub(super) const REGISTRY: &str = "SELECT path FROM projects ORDER BY path";

/// `/api/summary`: spent tokens, priced cost, unpriced turns, saved tokens and the two rates.
pub(super) const SUMMARY: &str = "SELECT t.spent, t.cost, t.unpriced, s.saved,
                COALESCE(s.saved * 1.0 / NULLIF(t.spent + s.saved, 0), 0.0),
                COALESCE(t.cache_read * 1.0 / NULLIF(t.prompt, 0), 0.0)
           FROM (SELECT COALESCE(SUM(input_tokens + output_tokens + cache_write_tokens + cache_read_tokens), 0) AS spent,
                        COALESCE(SUM(cost_usd), 0.0) AS cost,
                        COUNT(*) - COUNT(cost_usd) AS unpriced,
                        COALESCE(SUM(cache_read_tokens), 0) AS cache_read,
                        COALESCE(SUM(input_tokens + cache_write_tokens + cache_read_tokens), 0) AS prompt
                   FROM turns
                  WHERE timestamp >= ?1 AND timestamp < ?2
                    AND (?3 IS NULL OR project_hash = ?3)
                    AND (?4 IS NULL OR agent = ?4)
                    AND (?5 IS NULL OR model = ?5)
                    AND (?6 IS NULL OR effort = ?6)
                    AND agent IN ('claude', 'codex', 'cursor', 'pi', 'droid')) AS t,
                (SELECT COALESCE(SUM(CASE WHEN before_tokens > after_tokens THEN before_tokens - after_tokens ELSE 0 END), 0) AS saved
                   FROM savings_ledger
                  WHERE ts >= ?1 AND ts < ?2 AND (?7 IS NULL OR project_path = ?7)) AS s";

/// `/api/daily`: spent and saved tokens per UTC day, every day of the window.
pub(super) const DAILY: &str = "WITH RECURSIVE days(day) AS (
                 SELECT date(?1, 'unixepoch')
                 UNION ALL
                 SELECT date(day, '+1 day') FROM days WHERE day < date(?2 - 86400, 'unixepoch')
             ),
             spent(day, tokens) AS (
                 SELECT date(timestamp, 'unixepoch'),
                        SUM(input_tokens + output_tokens + cache_write_tokens + cache_read_tokens)
                   FROM turns
                  WHERE timestamp >= ?1 AND timestamp < ?2
                    AND (?3 IS NULL OR project_hash = ?3)
                    AND (?4 IS NULL OR agent = ?4)
                    AND (?5 IS NULL OR model = ?5)
                    AND (?6 IS NULL OR effort = ?6)
                    AND agent IN ('claude', 'codex', 'cursor', 'pi', 'droid')
                  GROUP BY 1
             ),
             saved(day, tokens) AS (
                 SELECT date(ts, 'unixepoch'),
                        SUM(CASE WHEN before_tokens > after_tokens THEN before_tokens - after_tokens ELSE 0 END)
                   FROM savings_ledger
                  WHERE ts >= ?1 AND ts < ?2 AND (?7 IS NULL OR project_path = ?7)
                  GROUP BY 1
             )
         SELECT days.day, COALESCE(spent.tokens, 0), COALESCE(saved.tokens, 0)
           FROM days
           LEFT JOIN spent ON spent.day = days.day
           LEFT JOIN saved ON saved.day = days.day
          ORDER BY days.day";

/// `/api/mix`: spent tokens and their share by agent.
pub(super) const MIX_BY_TOOL: &str = "SELECT agent, SUM(tokens), COALESCE(SUM(tokens) * 1.0 / NULLIF(SUM(SUM(tokens)) OVER (), 0), 0.0)
           FROM (SELECT agent, input_tokens + output_tokens + cache_write_tokens + cache_read_tokens AS tokens
                   FROM turns
                  WHERE timestamp >= ?1 AND timestamp < ?2
                    AND (?3 IS NULL OR project_hash = ?3)
                    AND (?4 IS NULL OR agent = ?4)
                    AND (?5 IS NULL OR model = ?5)
                    AND (?6 IS NULL OR effort = ?6)
                    AND agent IN ('claude', 'codex', 'cursor', 'pi', 'droid'))
          GROUP BY agent
          ORDER BY 2 DESC, agent";

/// `/api/mix`: spent tokens and their share by model.
pub(super) const MIX_BY_MODEL: &str = "SELECT model, SUM(tokens), COALESCE(SUM(tokens) * 1.0 / NULLIF(SUM(SUM(tokens)) OVER (), 0), 0.0)
           FROM (SELECT model, input_tokens + output_tokens + cache_write_tokens + cache_read_tokens AS tokens
                   FROM turns
                  WHERE timestamp >= ?1 AND timestamp < ?2
                    AND (?3 IS NULL OR project_hash = ?3)
                    AND (?4 IS NULL OR agent = ?4)
                    AND (?5 IS NULL OR model = ?5)
                    AND (?6 IS NULL OR effort = ?6)
                    AND agent IN ('claude', 'codex', 'cursor', 'pi', 'droid'))
          GROUP BY model
          ORDER BY 2 DESC, model";

/// `/api/runs` and `/api/runs/top`: one row per run (see `api::runs_query`).
pub(super) const RUNS: &str = "WITH f AS (
                 SELECT COALESCE(run_id, session_id) AS rk, run_id, session_id, project_hash, agent, model,
                        CASE WHEN effort IN ('low', 'medium', 'high', 'xhigh', 'max') THEN effort END AS effort,
                        input_tokens + cache_write_tokens + cache_read_tokens AS prompt, output_tokens, cost_usd
                   FROM turns
                  WHERE timestamp >= ?1 AND timestamp < ?2
                    AND (?3 IS NULL OR project_hash = ?3)
                    AND (?4 IS NULL OR agent = ?4)
                    AND (?5 IS NULL OR model = ?5)
                    AND (?6 IS NULL OR effort = ?6)
                    AND agent IN ('claude', 'codex', 'cursor', 'pi', 'droid')
             ),
             r AS (
                 SELECT rk, MAX(run_id) AS run_id, MIN(session_id) AS session_id,
                        MIN(project_hash) AS project_hash, MIN(agent) AS agent,
                        SUM(prompt) AS prompt, SUM(output_tokens) AS output,
                        CASE WHEN COUNT(cost_usd) = COUNT(*) THEN SUM(cost_usd) END AS cost,
                        COALESCE(SUM(cost_usd), 0.0) AS priced
                   FROM f
                  GROUP BY rk
             ),
             d AS (
                 SELECT rk, model, effort,
                        ROW_NUMBER() OVER (PARTITION BY rk ORDER BY SUM(prompt + output_tokens) DESC, model, effort) AS n
                   FROM f
                  GROUP BY rk, model, effort
             ),
             seen AS (
                 SELECT COALESCE(run_id, session_id) AS rk, MAX(timestamp) AS last
                   FROM turns
                  WHERE timestamp >= ?1
                  GROUP BY 1
             ),
             runs AS (
                 SELECT r.rk, r.run_id, r.project_hash, r.agent, r.prompt, r.output, r.cost, r.priced,
                        d.model, d.effort,
                        COALESCE(a.parent_run_id, a.session_id,
                                 CASE WHEN r.run_id IS NOT NULL AND r.session_id <> 'agent-' || r.run_id
                                      THEN r.session_id END) AS parent,
                        COALESCE(a.description, a.role) AS label,
                        CASE WHEN r.run_id IS NULL THEN 'main'
                             WHEN a.role = 'Explore' THEN 'Explore'
                             WHEN lower(a.role) IN ('builder', 'reviewer') THEN lower(a.role)
                             WHEN lower(a.description) LIKE '%review%' THEN 'reviewer'
                             WHEN lower(a.description) LIKE '%build%'
                               OR lower(a.description) LIKE '%implement%' THEN 'builder'
                             ELSE 'other' END AS role,
                        CASE WHEN r.run_id IS NOT NULL AND a.finished = 1 THEN 'finished'
                             WHEN seen.last >= ?9 THEN 'running'
                             WHEN r.run_id IS NULL THEN 'finished'
                             ELSE 'partial' END AS state
                   FROM r
                   JOIN d ON d.rk = r.rk AND d.n = 1
                   JOIN seen ON seen.rk = r.rk
                   LEFT JOIN agent_runs AS a ON a.run_id = r.run_id
             )
         SELECT rk, parent, label, role, agent, model, effort, prompt, output, cost, project_hash, state
           FROM runs
          ORDER BY priced DESC, prompt + output DESC, rk
          LIMIT ?8";

/// `/api/roles`: priced cost by role over the runs of [`RUNS`], whose CTE it repeats.
pub(super) const ROLES: &str = "WITH f AS (
                 SELECT COALESCE(run_id, session_id) AS rk, run_id, session_id, project_hash, agent, model,
                        CASE WHEN effort IN ('low', 'medium', 'high', 'xhigh', 'max') THEN effort END AS effort,
                        input_tokens + cache_write_tokens + cache_read_tokens AS prompt, output_tokens, cost_usd
                   FROM turns
                  WHERE timestamp >= ?1 AND timestamp < ?2
                    AND (?3 IS NULL OR project_hash = ?3)
                    AND (?4 IS NULL OR agent = ?4)
                    AND (?5 IS NULL OR model = ?5)
                    AND (?6 IS NULL OR effort = ?6)
                    AND agent IN ('claude', 'codex', 'cursor', 'pi', 'droid')
             ),
             r AS (
                 SELECT rk, MAX(run_id) AS run_id, MIN(session_id) AS session_id,
                        MIN(project_hash) AS project_hash, MIN(agent) AS agent,
                        SUM(prompt) AS prompt, SUM(output_tokens) AS output,
                        CASE WHEN COUNT(cost_usd) = COUNT(*) THEN SUM(cost_usd) END AS cost,
                        COALESCE(SUM(cost_usd), 0.0) AS priced
                   FROM f
                  GROUP BY rk
             ),
             d AS (
                 SELECT rk, model, effort,
                        ROW_NUMBER() OVER (PARTITION BY rk ORDER BY SUM(prompt + output_tokens) DESC, model, effort) AS n
                   FROM f
                  GROUP BY rk, model, effort
             ),
             seen AS (
                 SELECT COALESCE(run_id, session_id) AS rk, MAX(timestamp) AS last
                   FROM turns
                  WHERE timestamp >= ?1
                  GROUP BY 1
             ),
             runs AS (
                 SELECT r.rk, r.run_id, r.project_hash, r.agent, r.prompt, r.output, r.cost, r.priced,
                        d.model, d.effort,
                        COALESCE(a.parent_run_id, a.session_id,
                                 CASE WHEN r.run_id IS NOT NULL AND r.session_id <> 'agent-' || r.run_id
                                      THEN r.session_id END) AS parent,
                        COALESCE(a.description, a.role) AS label,
                        CASE WHEN r.run_id IS NULL THEN 'main'
                             WHEN a.role = 'Explore' THEN 'Explore'
                             WHEN lower(a.role) IN ('builder', 'reviewer') THEN lower(a.role)
                             WHEN lower(a.description) LIKE '%review%' THEN 'reviewer'
                             WHEN lower(a.description) LIKE '%build%'
                               OR lower(a.description) LIKE '%implement%' THEN 'builder'
                             ELSE 'other' END AS role,
                        CASE WHEN r.run_id IS NOT NULL AND a.finished = 1 THEN 'finished'
                             WHEN seen.last >= ?9 THEN 'running'
                             WHEN r.run_id IS NULL THEN 'finished'
                             ELSE 'partial' END AS state
                   FROM r
                   JOIN d ON d.rk = r.rk AND d.n = 1
                   JOIN seen ON seen.rk = r.rk
                   LEFT JOIN agent_runs AS a ON a.run_id = r.run_id
             )
         SELECT role, SUM(priced),
                COALESCE(SUM(priced) / NULLIF(SUM(SUM(priced)) OVER (), 0), 0.0),
                COALESCE(SUM(SUM(CASE WHEN role <> 'main' THEN priced ELSE 0.0 END)) OVER ()
                         / NULLIF(SUM(SUM(priced)) OVER (), 0), 0.0),
                SUM(SUM(role <> 'main' AND state <> 'finished')) OVER ()
           FROM runs
          GROUP BY role
          ORDER BY 2 DESC, role";

/// `/api/models`: priced cost per model and known effort.
pub(super) const MODEL_EFFORT: &str =
    "SELECT model, effort, CASE WHEN COUNT(cost_usd) = COUNT(*) THEN SUM(cost_usd) END
           FROM turns
          WHERE timestamp >= ?1 AND timestamp < ?2
            AND (?3 IS NULL OR project_hash = ?3)
            AND (?4 IS NULL OR agent = ?4)
            AND (?5 IS NULL OR model = ?5)
            AND (?6 IS NULL OR effort = ?6)
            AND agent IN ('claude', 'codex', 'cursor', 'pi', 'droid')
            AND effort IN ('low', 'medium', 'high', 'xhigh', 'max')
          GROUP BY model, effort
          ORDER BY SUM(COALESCE(SUM(cost_usd), 0.0)) OVER (PARTITION BY model) DESC, model";

/// `/api/thinking`: thinking over output tokens per model and effort.
pub(super) const THINKING: &str = "SELECT model, CASE WHEN effort IN ('low', 'medium', 'high', 'xhigh', 'max') THEN effort END AS level,
                SUM(thinking_tokens) * 1.0 / SUM(output_tokens)
           FROM turns
          WHERE timestamp >= ?1 AND timestamp < ?2
            AND (?3 IS NULL OR project_hash = ?3)
            AND (?4 IS NULL OR agent = ?4)
            AND (?5 IS NULL OR model = ?5)
            AND (?6 IS NULL OR effort = ?6)
            AND agent IN ('claude', 'codex', 'cursor', 'pi', 'droid')
            AND thinking_tokens IS NOT NULL
          GROUP BY model, level
         HAVING SUM(output_tokens) > 0
          ORDER BY 3 DESC, model, level";

/// `/api/fallbacks`: requested-to-served pairs and the fallback rate.
pub(super) const FALLBACKS: &str = "WITH f AS (
                 SELECT session_id, model,
                        CASE WHEN instr(requested_model, '[') > 0
                             THEN substr(requested_model, 1, instr(requested_model, '[') - 1)
                             ELSE requested_model END AS requested
                   FROM turns
                  WHERE timestamp >= ?1 AND timestamp < ?2
                    AND (?3 IS NULL OR project_hash = ?3)
                    AND (?4 IS NULL OR agent = ?4)
                    AND (?5 IS NULL OR model = ?5)
                    AND (?6 IS NULL OR effort = ?6)
                    AND agent IN ('claude', 'codex', 'cursor', 'pi', 'droid')
                    AND requested_model IS NOT NULL
             )
         SELECT requested, model, COUNT(*), COUNT(DISTINCT session_id),
                (SELECT SUM(requested <> model) * 1.0 / COUNT(*) FROM f)
           FROM f
          WHERE requested <> model
          GROUP BY requested, model
          ORDER BY 3 DESC, requested, model";

/// `/api/repos`: spent tokens and agents per project-hash bucket.
pub(super) const REPO_SPENT: &str = "SELECT project_hash, SUM(input_tokens + output_tokens + cache_write_tokens + cache_read_tokens),
                group_concat(DISTINCT agent)
           FROM turns
          WHERE timestamp >= ?1 AND timestamp < ?2
            AND (?3 IS NULL OR project_hash = ?3)
            AND (?4 IS NULL OR agent = ?4)
            AND (?5 IS NULL OR model = ?5)
            AND (?6 IS NULL OR effort = ?6)
            AND agent IN ('claude', 'codex', 'cursor', 'pi', 'droid')
          GROUP BY project_hash";

/// `/api/repos`: saved tokens per ledger key.
pub(super) const REPO_SAVED: &str = "SELECT project_path,
                SUM(CASE WHEN before_tokens > after_tokens THEN before_tokens - after_tokens ELSE 0 END)
           FROM savings_ledger
          WHERE ts >= ?1 AND ts < ?2 AND (?7 IS NULL OR project_path = ?7)
          GROUP BY project_path";

/// `/api/repos`: the latest turn per project-hash bucket, at any time.
pub(super) const REPO_LAST_TURN: &str =
    "SELECT project_hash, MAX(timestamp) FROM turns GROUP BY project_hash";

/// `/api/repos`: the latest tokensave call per ledger key, at any time.
pub(super) const REPO_LAST_CALL: &str =
    "SELECT project_path, MAX(ts) FROM savings_ledger GROUP BY project_path";

/// `/api/savings`: saved, before and after tokens and the number of calls.
pub(super) const SAVINGS: &str = "SELECT COALESCE(SUM(CASE WHEN before_tokens > after_tokens THEN before_tokens - after_tokens ELSE 0 END), 0),
                COALESCE(SUM(before_tokens), 0), COALESCE(SUM(after_tokens), 0), COUNT(*)
           FROM savings_ledger
          WHERE ts >= ?1 AND ts < ?2 AND (?7 IS NULL OR project_path = ?7)";

/// `/api/savings`: saved tokens and their share by tokensave tool.
pub(super) const SAVED_BY_TOOL: &str = "SELECT tool_name, SUM(saved),
                COALESCE(SUM(saved) * 1.0 / NULLIF(SUM(SUM(saved)) OVER (), 0), 0.0)
           FROM (SELECT tool_name,
                        CASE WHEN before_tokens > after_tokens THEN before_tokens - after_tokens ELSE 0 END AS saved
                   FROM savings_ledger
                  WHERE ts >= ?1 AND ts < ?2 AND (?7 IS NULL OR project_path = ?7))
          GROUP BY tool_name
          ORDER BY 2 DESC, tool_name";

/// `/api/savings`: the Claude Code turns `tokensave discover` analyses.
pub(super) const NAV_TURNS: &str = "SELECT tool_names, tool_result_tokens
           FROM turns
          WHERE timestamp >= ?1 AND timestamp < ?2
            AND (?3 IS NULL OR project_hash = ?3)
            AND agent = 'claude'";

/// `/api/sources`: turns recorded and the newest one per agent.
pub(super) const SOURCE_TURNS: &str =
    "SELECT agent, COUNT(*), strftime('%Y-%m-%dT%H:%M:%SZ', MAX(timestamp), 'unixepoch')
           FROM turns
          GROUP BY agent";

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    const TURN_FILTER: &str = "timestamp >= ?1 AND timestamp < ?2 \
        AND (?3 IS NULL OR project_hash = ?3) AND (?4 IS NULL OR agent = ?4) \
        AND (?5 IS NULL OR model = ?5) AND (?6 IS NULL OR effort = ?6) \
        AND agent IN ('claude', 'codex', 'cursor', 'pi', 'droid')";
    const LEDGER_FILTER: &str = "ts >= ?1 AND ts < ?2 AND (?7 IS NULL OR project_path = ?7)";

    fn words(sql: &str) -> String {
        sql.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn every_query_filters_with_the_same_clause() {
        let turns = [
            ("SUMMARY", SUMMARY),
            ("DAILY", DAILY),
            ("MIX_BY_TOOL", MIX_BY_TOOL),
            ("MIX_BY_MODEL", MIX_BY_MODEL),
            ("RUNS", RUNS),
            ("ROLES", ROLES),
            ("MODEL_EFFORT", MODEL_EFFORT),
            ("THINKING", THINKING),
            ("FALLBACKS", FALLBACKS),
            ("REPO_SPENT", REPO_SPENT),
        ];
        for (name, sql) in turns {
            assert!(words(sql).contains(&words(TURN_FILTER)), "{name}");
        }
        let ledger = [
            ("SUMMARY", SUMMARY),
            ("DAILY", DAILY),
            ("REPO_SAVED", REPO_SAVED),
            ("SAVINGS", SAVINGS),
            ("SAVED_BY_TOOL", SAVED_BY_TOOL),
        ];
        for (name, sql) in ledger {
            assert!(words(sql).contains(LEDGER_FILTER), "{name}");
        }
    }

    #[test]
    fn roles_aggregate_the_runs_of_the_runs_query() {
        let cte = |sql: &str| {
            let sql = words(sql);
            let end = sql.find(") SELECT ").expect("a final SELECT");
            sql[..end].to_string()
        };
        assert_eq!(cte(ROLES), cte(RUNS));
    }
}
