//! Merging the transcript lines of one Claude Code message into its turn.
//!
//! Claude Code writes one assistant message as several transcript lines, one
//! content block each, all with the same `message.id`. The first line seen
//! inserts the turn; every later one is merged into it here, so the turn holds
//! every tool call of the message and its final usage, not the first line's
//! (inkcast #250). On 76,473 messages (2026-10-27), 79% of `tool_use` blocks
//! were not on their message's first line, no block appeared on two lines, and
//! only `output_tokens` ever differed between lines, never decreasing.

use crate::accounting::classifier::{self, BashSignals};
use crate::accounting::pricing;
use crate::types::CostTurn;

/// One `tool_use` block of a transcript line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
}

/// One assistant line of a Claude Code transcript.
#[derive(Debug, Clone)]
pub struct ClaudeLine {
    /// The turn as this line alone describes it.
    pub turn: CostTurn,
    /// The line's `tool_use` blocks that have both an id and a name: the calls
    /// a later line of the message is merged against.
    pub calls: Vec<ToolCall>,
    /// The groups the line's Bash commands match.
    pub bash: BashSignals,
    /// Every `tool_use` id on the line, so the results in a later user line
    /// can be attributed to the turn (#474).
    pub tool_use_ids: Vec<String>,
}

impl ClaudeLine {
    /// The stored form of [`Self::calls`]' ids: comma-joined, as `tool_names`.
    /// Claude Code's ids are `toolu_` followed by `[A-Za-z0-9_-]` (all 80,623
    /// on 2026-10-27), so a comma never occurs in one.
    pub fn call_ids(&self) -> String {
        join(self.calls.iter().map(|call| call.id.as_str()))
    }
}

/// The columns of a recorded turn that a later line of its message can change,
/// and the model its cost is priced by.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnState {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: f64,
    pub category: String,
    pub tool_names: String,
    /// The ids of the calls recorded, comma-joined. `None` on a turn recorded
    /// before #250, whose calls are unknown.
    pub tool_use_ids: Option<String>,
    /// `None` on a turn recorded before #250.
    pub bash_signals: Option<BashSignals>,
    pub thinking_tokens: Option<u64>,
    pub speed: Option<String>,
    pub service_tier: Option<String>,
}

/// `stored` with `line` merged in, or `None` when the line adds nothing.
///
/// Every step only adds, so a line merged twice (a file read again, or two
/// ingests at once) changes nothing the second time, and a parse resumed
/// between two lines of a message gives the row a single pass gives:
///
/// - Each token counter keeps the larger value. A subagent message's first
///   line is written while it streams, with a fraction of its final
///   `output_tokens`. The cost is recomputed only when a counter grows.
/// - A call whose id is not recorded is appended, and the turn is classified
///   again from every call and Bash signal so far.
/// - `thinking_tokens`, `speed` and `service_tier` are filled where the turn
///   lacks them, never overwritten.
///
/// A turn recorded before #250 keeps its tools and category: which calls it
/// holds is unknown, so appending could repeat one.
pub fn merge_line(stored: &TurnState, line: &ClaudeLine) -> Option<TurnState> {
    let turn = &line.turn;
    let mut next = stored.clone();

    next.input_tokens = stored.input_tokens.max(turn.input_tokens);
    next.output_tokens = stored.output_tokens.max(turn.output_tokens);
    next.cache_write_tokens = stored.cache_write_tokens.max(turn.cache_write_tokens);
    next.cache_read_tokens = stored.cache_read_tokens.max(turn.cache_read_tokens);
    if counters(&next) != counters(stored) {
        next.cost_usd = pricing::cost_of_turn(
            &next.model,
            next.input_tokens,
            next.output_tokens,
            next.cache_write_tokens,
            next.cache_read_tokens,
        );
    }

    if let (Some(ids), Some(bash)) = (&stored.tool_use_ids, stored.bash_signals) {
        let mut ids = split(ids);
        let mut names = split(&stored.tool_names);
        let mut added = false;
        for call in &line.calls {
            if !ids.contains(&call.id.as_str()) {
                ids.push(&call.id);
                names.push(&call.name);
                added = true;
            }
        }
        let merged_bash = bash.union(line.bash);
        if added || merged_bash != bash {
            next.category = classifier::classify_with(&names, merged_bash)
                .as_str()
                .to_string();
            next.tool_names = join(names);
            next.tool_use_ids = Some(join(ids));
            next.bash_signals = Some(merged_bash);
        }
    }

    next.thinking_tokens = stored.thinking_tokens.or(turn.detail.thinking_tokens);
    next.speed = stored.speed.clone().or_else(|| turn.detail.speed.clone());
    next.service_tier = stored
        .service_tier
        .clone()
        .or_else(|| turn.detail.service_tier.clone());

    (next != *stored).then_some(next)
}

fn counters(turn: &TurnState) -> (u64, u64, u64, u64) {
    (
        turn.input_tokens,
        turn.output_tokens,
        turn.cache_write_tokens,
        turn.cache_read_tokens,
    )
}

/// The entries of a comma-joined list; an empty list has none.
fn split(list: &str) -> Vec<&str> {
    list.split(',').filter(|entry| !entry.is_empty()).collect()
}

fn join<'a>(entries: impl IntoIterator<Item = &'a str>) -> String {
    entries.into_iter().collect::<Vec<_>>().join(",")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::types::TurnDetail;

    fn line(output_tokens: u64, calls: &[(&str, &str)], bash: &[&str]) -> ClaudeLine {
        let calls: Vec<ToolCall> = calls
            .iter()
            .map(|(id, name)| ToolCall {
                id: (*id).to_string(),
                name: (*name).to_string(),
            })
            .collect();
        let names: Vec<&str> = calls.iter().map(|call| call.name.as_str()).collect();
        let bash = BashSignals::of(bash);
        ClaudeLine {
            turn: CostTurn {
                message_id: "msg_1".to_string(),
                project_hash: "p".to_string(),
                session_id: "s".to_string(),
                model: "claude-opus-4-6".to_string(),
                timestamp: 0,
                input_tokens: 3,
                output_tokens,
                cache_write_tokens: 10,
                cache_read_tokens: 20,
                cost_usd: 0.0,
                category: classifier::classify_with(&names, bash).as_str().to_string(),
                tool_names: names.join(","),
                agent: "claude".to_string(),
                credits: None,
                detail: TurnDetail::default(),
            },
            tool_use_ids: calls.iter().map(|call| call.id.clone()).collect(),
            calls,
            bash,
        }
    }

    /// The state the first line of a message inserts.
    fn inserted(first: &ClaudeLine) -> TurnState {
        let t = &first.turn;
        TurnState {
            model: t.model.clone(),
            input_tokens: t.input_tokens,
            output_tokens: t.output_tokens,
            cache_write_tokens: t.cache_write_tokens,
            cache_read_tokens: t.cache_read_tokens,
            cost_usd: t.cost_usd,
            category: t.category.clone(),
            tool_names: t.tool_names.clone(),
            tool_use_ids: Some(first.call_ids()),
            bash_signals: Some(first.bash),
            thinking_tokens: t.detail.thinking_tokens,
            speed: t.detail.speed.clone(),
            service_tier: t.detail.service_tier.clone(),
        }
    }

    fn merge_all(lines: &[ClaudeLine]) -> TurnState {
        lines[1..].iter().fold(inserted(&lines[0]), |state, l| {
            merge_line(&state, l).unwrap_or(state)
        })
    }

    #[test]
    fn calls_on_later_lines_are_appended_and_classified() {
        let lines = [
            line(5, &[], &[]),
            line(5, &[("toolu_a", "Read")], &[]),
            line(5, &[("toolu_b", "Edit")], &[]),
        ];
        let state = merge_all(&lines);
        assert_eq!(state.tool_names, "Read,Edit");
        assert_eq!(state.tool_use_ids.as_deref(), Some("toolu_a,toolu_b"));
        assert_eq!(state.category, "coding");
    }

    #[test]
    fn a_call_repeated_by_name_is_kept_and_one_read_twice_is_not() {
        let lines = [
            line(5, &[("toolu_a", "Read")], &[]),
            line(5, &[("toolu_b", "Read")], &[]),
            line(5, &[("toolu_b", "Read")], &[]),
        ];
        assert_eq!(merge_all(&lines).tool_names, "Read,Read");
    }

    #[test]
    fn a_bash_keyword_on_one_line_meets_an_edit_on_another() {
        let lines = [
            line(5, &[("toolu_a", "Bash")], &["fix the import"]),
            line(5, &[("toolu_b", "Edit")], &[]),
        ];
        assert_eq!(merge_all(&lines).category, "debugging");
    }

    #[test]
    fn the_largest_output_is_kept_and_priced() {
        let lines = [line(8, &[], &[]), line(1200, &[], &[]), line(900, &[], &[])];
        let state = merge_all(&lines);
        assert_eq!(state.output_tokens, 1200);
        let priced = pricing::cost_of_turn("claude-opus-4-6", 3, 1200, 10, 20);
        assert!(priced > 0.0);
        assert!((state.cost_usd - priced).abs() < f64::EPSILON);
    }

    #[test]
    fn every_line_merged_again_changes_nothing() {
        let lines = [
            line(8, &[], &[]),
            line(8, &[("toolu_a", "Bash")], &["git status"]),
            line(1200, &[("toolu_b", "Edit")], &[]),
        ];
        let state = merge_all(&lines);
        for l in &lines {
            assert_eq!(merge_line(&state, l), None);
        }
    }

    #[test]
    fn a_resumed_merge_gives_the_single_pass_row() {
        let lines = [
            line(8, &[("toolu_a", "Bash")], &["fix it"]),
            line(8, &[("toolu_b", "Read")], &[]),
            line(1200, &[("toolu_c", "Edit")], &[]),
        ];
        let first_pass = merge_all(&lines[..2]);
        let resumed = merge_line(&first_pass, &lines[2]).unwrap();
        assert_eq!(resumed, merge_all(&lines));
    }

    #[test]
    fn usage_detail_is_filled_never_overwritten() {
        let first = line(8, &[], &[]);
        let mut later = line(8, &[], &[]);
        later.turn.detail.speed = Some("standard".to_string());
        later.turn.detail.thinking_tokens = Some(648);
        let state = merge_line(&inserted(&first), &later).unwrap();
        assert_eq!(state.speed.as_deref(), Some("standard"));
        assert_eq!(state.thinking_tokens, Some(648));

        let mut other = line(8, &[], &[]);
        other.turn.detail.speed = Some("fast".to_string());
        assert_eq!(merge_line(&state, &other), None);
    }

    #[test]
    fn a_turn_recorded_before_the_merge_keeps_its_tools() {
        let mut legacy = inserted(&line(8, &[("toolu_a", "Read")], &[]));
        legacy.tool_use_ids = None;
        legacy.bash_signals = None;
        let state = merge_line(&legacy, &line(1200, &[("toolu_b", "Edit")], &[])).unwrap();
        assert_eq!(state.tool_names, "Read");
        assert_eq!(state.category, "exploration");
        assert_eq!(state.output_tokens, 1200);
    }
}
