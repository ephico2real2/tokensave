#!/bin/sh
# Prints the commands that make your shell and your agents run this fork.
#
# `cargo install --path .` puts the fork in ~/.cargo/bin (or $CARGO_HOME/bin),
# and an upstream install (Homebrew, a release download) usually comes first
# on PATH. This script:
#   1. detects your login shell from $SHELL, finds its startup file, and
#      prints the lines to add there, last, so they run after Homebrew's setup;
#   2. reads the agents tokensave records as installed, checks which binary
#      the configs of Claude Code, Codex, Cursor and Copilot name, and prints
#      the commands that point the others at the fork. `tokensave reinstall`
#      doesn't: it keeps an MCP command that still works (upstream #161).
# It only prints: it changes no file. The printed commands carry no `#`
# comments, because zsh doesn't read them as comments at a prompt.
#
# Usage: sh scripts/fork-path.sh
set -eu

if [ -n "${CARGO_HOME:-}" ]; then
    cargo_bin="$CARGO_HOME/bin"
    shown_bin="$cargo_bin"
else
    cargo_bin="$HOME/.cargo/bin"
    # Written as is, so the startup file expands it.
    # shellcheck disable=SC2016
    shown_bin='$HOME/.cargo/bin'
fi
fork="$cargo_bin/tokensave"
marker='# tokensave2 fork: run tokensave from cargo, before any other install'

if [ ! -x "$fork" ]; then
    echo "The fork isn't installed in $cargo_bin yet. Build it first, from the checkout:"
    echo "  cargo install --locked --path ."
    exit 1
fi

# ── 1. The shell ─────────────────────────────────────────────────────────────

shell_path="${SHELL:-}"
shell_name=$(basename "${shell_path:-unknown}")
startup=""
case "$shell_name" in
    zsh)
        startup="${ZDOTDIR:-$HOME}/.zshrc"
        line="export PATH=\"$shown_bin:\$PATH\""
        ;;
    bash)
        # Terminal on macOS starts bash as a login shell, which reads only
        # the first of ~/.bash_profile, ~/.bash_login and ~/.profile that
        # exists. Creating ~/.bash_profile would hide the other two.
        if [ "$(uname -s)" = Darwin ]; then
            startup="$HOME/.bash_profile"
            for file in "$HOME/.bash_profile" "$HOME/.bash_login" "$HOME/.profile"; do
                if [ -f "$file" ]; then
                    startup="$file"
                    break
                fi
            done
        else
            startup="$HOME/.bashrc"
        fi
        line="export PATH=\"$shown_bin:\$PATH\""
        ;;
    fish)
        startup="${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish"
        line="fish_add_path --move --path \"$shown_bin\""
        ;;
esac

echo "== Shell"
echo "Login shell:  ${shell_path:-not set} ($shell_name)"
current=$(command -v tokensave 2>/dev/null || true)
echo "tokensave now: ${current:-not on PATH}"
echo

if [ "$current" = "$fork" ]; then
    echo "Nothing to do: tokensave already runs the fork."
elif [ -z "$startup" ]; then
    echo "This script knows zsh, bash and fish. Put $cargo_bin first on PATH in"
    echo "your shell's startup file, after any other PATH change."
else
    echo "Startup file: $startup"
    if [ -f "$startup" ] && grep -qF "$marker" "$startup"; then
        echo "It already has the fork's PATH line. Open a new terminal, or run:"
    else
        echo "Run these in your shell:"
        echo
        # Single quotes keep $HOME and $PATH for the startup file to expand.
        printf "printf '\\\\n%%s\\\\n%%s\\\\n' '%s' '%s' >> \"%s\"\n" "$marker" "$line" "$startup"
    fi
    echo "exec \"$shell_path\" -l"
    echo
    echo "Then check. The first prints $fork, the second 'fork: ok':"
    echo
    echo "which tokensave"
    echo "tokensave dashboard --help > /dev/null && echo 'fork: ok'"
fi

# ── 2. The agents ────────────────────────────────────────────────────────────

state="${TOKENSAVE_HOME:-$HOME/.tokensave}/state.toml"
echo
echo "== Agents"
agents=""
if [ -f "$state" ]; then
    # From the `installed_agents =` line to its closing `]`, which tokensave
    # writes on the same line for zero or one agent.
    agents=$(awk '
        /^[[:space:]]*installed_agents[[:space:]]*=/ { found = 1 }
        found {
            sub(/#.*/, "")
            print
            if (index($0, "]")) exit
        }
    ' "$state" | grep -o '"[^"]*"' | tr -d '"' | tr '\n' ' ')
fi
if [ -z "$agents" ]; then
    echo "No agent is recorded as installed in $state."
    echo "Install one with the fork: $fork install --agent <agent>"
    exit 0
fi

claude_dir="${CLAUDE_CONFIG_DIR:-}"
claude_dir="${claude_dir%%,*}"
# The VS Code folders tokensave writes (vscode_data_dir and
# vscode_insiders_data_dir in src/agents/fs.rs): XDG_CONFIG_HOME is not used.
if [ "$(uname -s)" = Darwin ]; then
    vscode_user="$HOME/Library/Application Support/Code/User"
    insiders_user="$HOME/Library/Application Support/Code - Insiders/User"
else
    vscode_user="$HOME/.config/Code/User"
    insiders_user="$HOME/.config/Code - Insiders/User"
fi

# The config files that hold an agent's MCP command or hooks, one per line.
# VS Code's settings.json can hold a pre-#266 entry that uninstall removes.
config_files() {
    case "$1" in
        claude)
            if [ -n "$claude_dir" ]; then
                printf '%s\n' "$claude_dir/.claude.json" "$claude_dir/settings.json"
            else
                printf '%s\n' "$HOME/.claude.json" "$HOME/.claude/settings.json"
            fi
            ;;
        codex) printf '%s\n' "$HOME/.codex/config.toml" ;;
        cursor) printf '%s\n' "$HOME/.cursor/mcp.json" "$HOME/.cursor/hooks.json" ;;
        copilot)
            printf '%s\n' "$HOME/.copilot/mcp-config.json" \
                "$vscode_user/mcp.json" "$vscode_user/settings.json" \
                "$insiders_user/mcp.json" "$insiders_user/settings.json" \
                "$HOME/.config/github-copilot/intellij/mcp.json"
            ;;
    esac
}

# Whether any of an agent's config files exists.
has_config() {
    config_files "$1" | while IFS= read -r file; do
        if [ -f "$file" ]; then
            echo yes
            break
        fi
    done | grep -q yes
}

# The tokensave programs an agent's configs run, one per line: the value of
# each JSON "command" key and TOML `command =` that is tokensave, a path
# ending in /tokensave, or one of those followed by a hook's subcommand.
# Other mentions (a project path, a permission rule) are not commands.
tokensave_commands() {
    config_files "$1" | while IFS= read -r file; do
        [ -f "$file" ] || continue
        grep -oE '"command"[[:space:]]*:[[:space:]]*"[^"]*"|^[[:space:]]*command[[:space:]]*=[[:space:]]*"[^"]*"' \
            "$file" 2>/dev/null || true
    done | sed -e 's/^[^:=]*[:=][[:space:]]*"//' -e 's/"$//' |
        sed -n 's/^\(\(.*\/\)\{0,1\}tokensave\)\( .*\)\{0,1\}$/\1/p' | sort -u
}

# Each program other than the fork, joined with ", ". A bare `tokensave` runs
# whatever the agent finds first on its own PATH.
other_commands() {
    tokensave_commands "$1" | while IFS= read -r cmd; do
        if [ "$cmd" = "$fork" ]; then
            continue
        elif [ "$cmd" = tokensave ]; then
            echo "tokensave (looked up on the agent's PATH)"
        elif [ -f "$cmd" ] && [ -x "$cmd" ]; then
            echo "$cmd"
        else
            echo "$cmd (missing)"
        fi
    done | awk 'NR > 1 { printf ", " } { printf "%s", $0 } END { if (NR) print "" }'
}

to_switch=""
for agent in $agents; do
    case "$agent" in
        claude | codex | cursor | copilot)
            if ! has_config "$agent"; then
                printf '  %-8s %s\n' "$agent" "no config found"
                continue
            fi
            others=$(other_commands "$agent")
            if [ -z "$(tokensave_commands "$agent")" ]; then
                printf '  %-8s %s\n' "$agent" "has no tokensave command in its config"
                to_switch="$to_switch $agent"
            elif [ -n "$others" ]; then
                printf '  %-8s %s\n' "$agent" "names $others"
                to_switch="$to_switch $agent"
            else
                printf '  %-8s %s\n' "$agent" "runs the fork"
            fi
            ;;
        *)
            printf '  %-8s %s\n' "$agent" "not checked by this script"
            ;;
    esac
done

if [ -z "$to_switch" ]; then
    echo
    echo "Nothing to switch among the agents checked."
    exit 0
fi

echo
echo "Run these to point those agents at the fork. tokensave's entries (MCP server,"
echo "hooks, rules) are removed and written again; the rest of each config stays."
echo
for agent in $to_switch; do
    echo "\"$fork\" uninstall --agent $agent --keep-git-hooks"
done
install="\"$fork\" install"
for agent in $to_switch; do
    install="$install --agent $agent"
done
echo "$install --git-hook no"
echo
echo "Then restart those agents, and run this script again: each should say it runs the fork."
