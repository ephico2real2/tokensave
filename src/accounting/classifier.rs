//! Deterministic task classification for Claude Code turns.
//!
//! Classifies each API turn into one of 13 categories based on tool usage
//! patterns and keyword matching. Adapted from AgentSeal/codeburn (MIT).

use std::fmt;

/// Task category for a single API turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskCategory {
    Coding,
    Debugging,
    FeatureDev,
    Refactoring,
    Testing,
    Exploration,
    Planning,
    Delegation,
    GitOps,
    BuildDeploy,
    Brainstorming,
    Conversation,
    General,
}

impl fmt::Display for TaskCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TaskCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Coding => "coding",
            Self::Debugging => "debugging",
            Self::FeatureDev => "feature_dev",
            Self::Refactoring => "refactoring",
            Self::Testing => "testing",
            Self::Exploration => "exploration",
            Self::Planning => "planning",
            Self::Delegation => "delegation",
            Self::GitOps => "git_ops",
            Self::BuildDeploy => "build_deploy",
            Self::Brainstorming => "brainstorming",
            Self::Conversation => "conversation",
            Self::General => "general",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Coding => "Coding",
            Self::Debugging => "Debugging",
            Self::FeatureDev => "Feature Dev",
            Self::Refactoring => "Refactoring",
            Self::Testing => "Testing",
            Self::Exploration => "Exploration",
            Self::Planning => "Planning",
            Self::Delegation => "Delegation",
            Self::GitOps => "Git Ops",
            Self::BuildDeploy => "Build/Deploy",
            Self::Brainstorming => "Brainstorming",
            Self::Conversation => "Conversation",
            Self::General => "General",
        }
    }
}

/// The Bash patterns behind each command-based category rule, matched as
/// substrings of the lowercased command.
const GIT_PATTERNS: &[&str] = &["git "];
const TEST_PATTERNS: &[&str] = &[
    "cargo test",
    "cargo nextest",
    "pytest",
    "vitest",
    "jest ",
    "mocha ",
    "npm test",
    "npm run test",
    "pnpm test",
    "pnpm run test",
    "go test",
    "dotnet test",
    "flutter test",
];
const BUILD_PATTERNS: &[&str] = &[
    "cargo build",
    "cargo check",
    "npm run build",
    "pnpm build",
    "docker ",
    "kubectl ",
    "pm2 ",
    "tsc",
    "next build",
];
const DEBUG_PATTERNS: &[&str] = &[
    "fix",
    "debug",
    "error",
    "bug",
    "issue",
    "stacktrace",
    "panic",
];
const REFACTOR_PATTERNS: &[&str] = &["refactor", "rename", "simplify", "extract", "inline"];

/// Which of the classifier's Bash pattern groups a turn's commands matched.
///
/// Every rule asks only whether *any* command matches a group, so the groups
/// matched classify exactly as the commands themselves do. A Claude Code
/// message is written over several transcript lines, and its turn keeps these
/// rather than the command text, so a later line can be classified together
/// with the earlier ones (inkcast #250).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BashSignals(u8);

impl BashSignals {
    const GIT: u8 = 1;
    const TEST: u8 = 1 << 1;
    const BUILD: u8 = 1 << 2;
    const DEBUG: u8 = 1 << 3;
    const REFACTOR: u8 = 1 << 4;

    /// The groups `commands` match.
    pub fn of(commands: &[&str]) -> Self {
        let groups = [
            (Self::GIT, GIT_PATTERNS),
            (Self::TEST, TEST_PATTERNS),
            (Self::BUILD, BUILD_PATTERNS),
            (Self::DEBUG, DEBUG_PATTERNS),
            (Self::REFACTOR, REFACTOR_PATTERNS),
        ];
        let mut bits = 0;
        for command in commands {
            let lower = command.to_ascii_lowercase();
            for (bit, patterns) in groups {
                if patterns.iter().any(|p| lower.contains(p)) {
                    bits |= bit;
                }
            }
        }
        Self(bits)
    }

    /// The stored form.
    pub fn bits(self) -> u8 {
        self.0
    }

    /// From the stored form. Bits this version does not know are kept, so a
    /// value written by a newer version survives a merge made by this one.
    pub fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// The groups matched by either.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }
}

/// Classify a turn based on tool names and Bash command content.
///
/// `tool_names`: names of all `tool_use` blocks in the turn (e.g. "Edit", "Bash").
/// `bash_commands`: content of Bash tool inputs (the `command` field).
pub fn classify(tool_names: &[&str], bash_commands: &[&str]) -> TaskCategory {
    classify_with(tool_names, BashSignals::of(bash_commands))
}

/// [`classify`], with the Bash commands already reduced to [`BashSignals`].
pub fn classify_with(tool_names: &[&str], bash: BashSignals) -> TaskCategory {
    if tool_names.is_empty() {
        return TaskCategory::Conversation;
    }

    // Agent tool → delegation
    if tool_names.contains(&"Agent") {
        return TaskCategory::Delegation;
    }

    // Planning tools
    if tool_names.contains(&"EnterPlanMode") || tool_names.contains(&"TaskCreate") {
        return TaskCategory::Planning;
    }

    let has_edit = tool_names.contains(&"Edit") || tool_names.contains(&"Write");
    let has_read_only = tool_names.contains(&"Read")
        || tool_names.contains(&"Grep")
        || tool_names.contains(&"Glob")
        || tool_names.contains(&"WebSearch");

    // Git operations
    if bash.has(BashSignals::GIT) {
        return TaskCategory::GitOps;
    }

    // Testing: test runner commands
    if bash.has(BashSignals::TEST) {
        return TaskCategory::Testing;
    }

    // Build/deploy
    if bash.has(BashSignals::BUILD) {
        return TaskCategory::BuildDeploy;
    }

    // Debugging: error/fix keywords in bash output context
    if bash.has(BashSignals::DEBUG) && has_edit {
        return TaskCategory::Debugging;
    }

    // Refactoring keywords in bash commands
    if bash.has(BashSignals::REFACTOR) {
        return TaskCategory::Refactoring;
    }

    // Coding: has edit/write tools
    if has_edit {
        return TaskCategory::Coding;
    }

    // Exploration: read-only tools without edits
    if has_read_only {
        return TaskCategory::Exploration;
    }

    TaskCategory::General
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_tools_is_conversation() {
        assert_eq!(classify(&[], &[]), TaskCategory::Conversation);
    }

    #[test]
    fn test_edit_is_coding() {
        assert_eq!(classify(&["Edit"], &[]), TaskCategory::Coding);
    }

    #[test]
    fn test_write_is_coding() {
        assert_eq!(classify(&["Write"], &[]), TaskCategory::Coding);
    }

    #[test]
    fn test_agent_is_delegation() {
        assert_eq!(classify(&["Agent", "Edit"], &[]), TaskCategory::Delegation);
    }

    #[test]
    fn test_git_bash_is_gitops() {
        assert_eq!(classify(&["Bash"], &["git status"]), TaskCategory::GitOps);
    }

    #[test]
    fn test_cargo_test_is_testing() {
        assert_eq!(
            classify(&["Bash"], &["cargo test --lib"]),
            TaskCategory::Testing
        );
    }

    #[test]
    fn test_read_only_is_exploration() {
        assert_eq!(classify(&["Read", "Grep"], &[]), TaskCategory::Exploration);
    }

    #[test]
    fn test_plan_mode_is_planning() {
        assert_eq!(classify(&["EnterPlanMode"], &[]), TaskCategory::Planning);
    }

    #[test]
    fn test_docker_is_build_deploy() {
        assert_eq!(
            classify(&["Bash"], &["docker build -t myapp ."]),
            TaskCategory::BuildDeploy
        );
    }

    #[test]
    fn test_fix_with_edit_is_debugging() {
        assert_eq!(
            classify(&["Bash", "Edit"], &["fix the broken import"]),
            TaskCategory::Debugging
        );
    }

    #[test]
    fn test_category_display() {
        assert_eq!(TaskCategory::GitOps.as_str(), "git_ops");
        assert_eq!(TaskCategory::GitOps.label(), "Git Ops");
    }

    /// Commands reduced one at a time and joined classify as all of them
    /// together, for every rule that reads a command.
    #[test]
    fn signals_joined_per_command_classify_as_the_commands() {
        let commands = [
            "git status",
            "cargo test --lib",
            "docker build .",
            "fix the import",
            "rename the module",
            "ls -la",
        ];
        let tool_sets: [&[&str]; 4] = [&["Bash"], &["Bash", "Edit"], &["Bash", "Read"], &[]];
        for tools in tool_sets {
            for a in commands {
                for b in commands {
                    let joined = BashSignals::of(&[a]).union(BashSignals::of(&[b]));
                    assert_eq!(
                        classify_with(tools, joined),
                        classify(tools, &[a, b]),
                        "{tools:?} {a:?} {b:?}"
                    );
                }
            }
        }
    }

    /// The debugging rule needs a keyword and an edit; they may come from
    /// different lines of one message.
    #[test]
    fn a_keyword_kept_as_a_signal_meets_a_later_edit() {
        let first_line = BashSignals::of(&["fix the import"]);
        assert_eq!(classify_with(&["Bash"], first_line), TaskCategory::General);
        assert_eq!(
            classify_with(&["Bash", "Edit"], first_line),
            TaskCategory::Debugging
        );
    }

    #[test]
    fn signals_round_trip_their_stored_form_including_unknown_bits() {
        let stored = BashSignals::of(&["git log", "pytest"]).bits() | 0b1000_0000;
        assert_eq!(BashSignals::from_bits(stored).bits(), stored);
        assert_eq!(BashSignals::of(&[]).bits(), 0);
    }
}
