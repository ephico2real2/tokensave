#!/bin/sh
# Prints the commands that make `tokensave` run this fork in your login shell.
#
# `cargo install --path .` puts the fork in ~/.cargo/bin (or $CARGO_HOME/bin),
# and an upstream install (Homebrew, a release download) usually comes first
# on PATH. This script detects your login shell from $SHELL, finds its startup
# file, and prints the lines to add there, last, so they run after Homebrew's
# setup. It only prints: it changes no file.
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
marker='# tokensave2 fork: run tokensave from cargo, before any other install'

shell_path="${SHELL:-}"
shell_name=$(basename "${shell_path:-unknown}")
case "$shell_name" in
    zsh)
        startup="${ZDOTDIR:-$HOME}/.zshrc"
        line="export PATH=\"$shown_bin:\$PATH\""
        ;;
    bash)
        # Terminal on macOS starts bash as a login shell, which reads
        # ~/.bash_profile and not ~/.bashrc.
        if [ "$(uname -s)" = Darwin ]; then
            startup="$HOME/.bash_profile"
        else
            startup="$HOME/.bashrc"
        fi
        line="export PATH=\"$shown_bin:\$PATH\""
        ;;
    fish)
        startup="${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish"
        line="fish_add_path --move --path \"$shown_bin\""
        ;;
    *)
        echo "Login shell: ${shell_path:-not set} (not zsh, bash or fish)."
        echo "Put $cargo_bin first on PATH in its startup file, after any other PATH change."
        exit 1
        ;;
esac

echo "Login shell:  $shell_path ($shell_name)"
echo "Startup file: $startup"

if [ ! -x "$cargo_bin/tokensave" ]; then
    echo
    echo "The fork isn't installed in $cargo_bin yet. Build it first, from the checkout:"
    echo "  cargo install --locked --path ."
    exit 1
fi

current=$(command -v tokensave 2>/dev/null || true)
echo "tokensave now: ${current:-not on PATH}"

if [ "$current" = "$cargo_bin/tokensave" ]; then
    echo
    echo "Nothing to do: tokensave already runs the fork."
    exit 0
fi

echo
if [ -f "$startup" ] && grep -qF "$marker" "$startup"; then
    echo "$startup already has the fork's PATH line. Open a new terminal, or run:"
else
    echo "Run these in your shell:"
    echo
    # Single quotes keep $HOME and $PATH for the startup file to expand.
    printf "printf '\\\\n%%s\\\\n%%s\\\\n' '%s' '%s' >> \"%s\"\n" "$marker" "$line" "$startup"
fi
echo "exec \"$shell_path\" -l"
echo
echo "Then check:"
echo "which tokensave    # $cargo_bin/tokensave"
echo "tokensave dashboard --help > /dev/null && echo 'fork: ok'"
