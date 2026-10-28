# Running the tokensave2 fork

This fork ([ephico2real2/tokensave2](https://github.com/ephico2real2/tokensave2)) is tokensave with token accounting
for more agents and a local dashboard. It installs beside an upstream tokensave and reports the same version
number, so this guide also shows how to tell the two apart.

## What the fork adds

| Feature | Command | Issue |
|---|---|---|
| Per-turn detail for Claude Code: effort, requested model, thinking tokens and time, subagent runs | recorded by the hooks and `tokensave cost` | [inkcast#246](https://github.com/inkcasthq/inkcast/issues/246) |
| Fill that detail into turns recorded before it | `tokensave accounting backfill` | [inkcast#249](https://github.com/inkcasthq/inkcast/issues/249) |
| Codex CLI sessions in the accounting | `tokensave cost` | [inkcast#260](https://github.com/inkcasthq/inkcast/issues/260) |
| Cursor usage, fetched from Cursor | `tokensave cursor sync` | [inkcast#261](https://github.com/inkcasthq/inkcast/issues/261) |
| A local web dashboard of spend and savings | `tokensave dashboard` | [inkcast#264](https://github.com/inkcasthq/inkcast/issues/264), [inkcast#265](https://github.com/inkcasthq/inkcast/issues/265) |

The plan and the open work are in [inkcast#227](https://github.com/inkcasthq/inkcast/issues/227).

## 1. Build and install

You need git and [rustup](https://rustup.rs). Node is not needed: the dashboard page is committed, built, in
`dashboard/dist/`.

```sh
git clone https://github.com/ephico2real2/tokensave2.git ~/gitRepos/tokensave
cd ~/gitRepos/tokensave
rustup toolchain install 1.98.1
cargo install --locked --path .
```

Run `cargo install` from inside the checkout. rustup picks the toolchain from `rust-toolchain.toml` in the
current folder, not from `--path`. The binary goes to `~/.cargo/bin/tokensave`.

## 2. Point your shell and your agents at the fork

An upstream tokensave (Homebrew, a release download) can stay installed. It usually comes first on `PATH`, and the
agents' configs may still name it. From the checkout, run:

```sh
sh scripts/fork-path.sh
```

It changes no file. It prints what it found and the commands to run, in two parts.

- **Shell.** It detects your login shell (zsh, bash or fish) from `$SHELL` and finds its startup file. It prints
  the command that adds the `PATH` line to the end of that file (after Homebrew's setup), the command that
  restarts the shell, and two checks: `which tokensave` prints `~/.cargo/bin/tokensave`, and
  `tokensave dashboard --help` succeeds, because only the fork has that command.
- **Agents.** It reads the agents tokensave recorded as installed (`~/.tokensave/state.toml`) and checks which
  binary the configs of Claude Code, Codex, Cursor and Copilot name. For each one still naming another install,
  it prints `uninstall --agent <agent> --keep-git-hooks` and one `install --agent ... --git-hook no`, run with the
  fork. tokensave's entries (MCP server, hooks, rules) are removed and written again, and the rest of each config
  stays, except comments in VS Code's `mcp.json`: tokensave writes that file back without them.

`tokensave reinstall` alone isn't enough. It keeps an MCP server command that still works (upstream #161,
`preserve_mcp_command` in `src/agents/mod.rs`), so an MCP entry that names Homebrew's binary keeps naming it, and
it skips Claude Code hooks that are already present, whatever binary they name.

Run the script again afterwards. It says "Nothing to do" for the shell, and "runs the fork" for each agent.
Agent sessions that were already open keep the build they started with until you restart them. `tokensave servers`
lists the MCP servers that are running.

`tokensave --version` doesn't tell the two apart: the fork reports the version of the upstream release it is
based on.

Copy the commands without comments. At a zsh prompt, `#` starts a comment only when `interactivecomments` is
set, so `cmd  # note` passes `#` and `note` to `cmd` as arguments.

## 3. Agents that aren't installed on this machine

The agents list can name one whose program isn't installed here, for example `omp` (Oh My Pi) with no `omp`
command. Then `reinstall` ends with `warning: could not refresh tokensave config for: omp`, because the OMP
integration asks `omp config path` for its folder. The warning is harmless. You don't need OMP for the fork, the
dashboard or any other agent. `tokensave uninstall --agent omp` fails the same way, so it can't remove the entry
yet.

## 4. First run

Back up the global database first. The fork migrates it to a newer schema the first time it opens it, and
upstream builds haven't been tested against a migrated database.

```sh
sqlite3 ~/.tokensave/global.db ".backup $HOME/.tokensave/global.db.before-fork"
tokensave cost
tokensave accounting backfill --dry-run
tokensave accounting backfill
```

`tokensave cost` migrates the database, then reads Claude Code, Codex, Pi and Droid sessions. The backfill's dry
run counts what it would fill in older Claude Code turns, and writes nothing.

The Claude Code Stop hook records Claude Code turns as you work. Codex, Pi and Droid sessions are read by
`tokensave cost`, `status` and `monitor`. Run one of them to bring those up to date.

The backfill is needed once. It never changes a token count or a cost, and a second run changes nothing.

## 5. Cursor usage

Cursor keeps no token counts on disk, so `tokensave cursor sync` fetches them from Cursor. It runs only when you
run it, never from a hook. There are two sources:

- **Admin API** (Teams and Enterprise plans): set `TOKENSAVE_CURSOR_ADMIN_KEY`, or save the key in
  `~/.tokensave/cursor-admin-key` with `chmod 600`.
- **The Cursor app's login** (any plan), with `--use-login`. It calls the undocumented endpoint Cursor's own
  dashboard uses. Cursor's terms (section 1.5) restrict extracting data from the service, so this source is off
  unless you ask for it.

```sh
tokensave cursor sync --use-login --dry-run
tokensave cursor sync --use-login
```

The dry run fetches and counts, and writes nothing.

The first sync fetches all history. Later ones start an hour before the newest Cursor event recorded. Use
`--since YYYY-MM-DD` to choose the start. The credential is never printed or stored.

## 6. The dashboard

```sh
tokensave dashboard enable
tokensave dashboard open
tokensave dashboard status
tokensave dashboard stop
```

`enable` switches it on in `~/.tokensave/dashboard.toml` and starts it. `open` opens `http://127.0.0.1:7311/` in
the browser.

It binds `127.0.0.1` only and opens `global.db` read only. It never runs an ingest itself, so its figures are as
recent as the last `tokensave cost`, `cursor sync` or agent hook. Settings (port, theme, default range) are
described in the README's [Dashboard](../README.md#dashboard) section. Its log is `~/.tokensave/dashboard.log`.

It doesn't start at login yet ([inkcast#270](https://github.com/inkcasthq/inkcast/issues/270)).

## 7. Update the fork

```sh
cd ~/gitRepos/tokensave
git pull --ff-only
cargo install --locked --path .
tokensave dashboard stop && tokensave dashboard start
```

A `--path` install always rebuilds and replaces the binary. Then restart your agent sessions.

**Don't run `tokensave upgrade`.** It downloads the latest upstream release and writes it over the binary that ran
it, which replaces the fork.

## Troubleshooting

| Message | What to do |
|---|---|
| `error: unrecognized subcommand 'dashboard'` (or `'cursor'`) | An upstream build ran. Run `sh scripts/fork-path.sh` (step 2). |
| `cargo install` warns `be sure to add .../.cargo/bin to your PATH` | The same: step 2. |
| `cargo install` warns `stripping debug info with rust-objcopy failed` and `Library not loaded: @rpath/libLLVM.dylib` (macOS) | Harmless: the binary is installed, with its symbols left in. The toolchain's `rust-objcopy` looks for `libLLVM.dylib` in `lib/rustlib/<host>/lib`, and the toolchain installs it in `lib/`. |
| `port 7311 on 127.0.0.1 is in use` | Another program holds the port. Use `--port N`, or `port` in `dashboard.toml`. |
| The page says "dashboard page not built" | The binary was built without `dashboard/dist/`. Build from a full checkout of the fork. |
| `Cursor login expired: sign in to the Cursor app again` | Sign in to Cursor, then sync again. |
| `the Cursor app's data was not found: install Cursor and sign in` | The Cursor app isn't installed for this user. Install it and sign in. |
| `the Cursor app is not signed in` | Sign in to the Cursor app, then sync again. |
| `No Cursor usage source is set up; nothing was fetched.` | Add `--use-login`, or set an Admin API key (step 5). |
| `warning: could not refresh tokensave config for: omp.` after `reinstall` | Harmless when OMP isn't installed (step 3). |
| `grep: #: No such file or directory` (or another command given `#` as an argument) | A command was pasted with a `# comment` into zsh. Paste it without the comment (step 2). |

## Going back to upstream

```sh
cargo uninstall tokensave
tokensave uninstall --agent claude --keep-git-hooks
tokensave install --agent claude --git-hook no
tokensave reinstall
```

With the fork gone, `tokensave` is the upstream binary. In bash, run `hash -r` first, or bash runs the removed path.
`reinstall` points the MCP entries and Cursor's hooks at upstream, because the fork's path no longer resolves. It
skips Claude Code's hooks, which still name the fork, so the two lines before it write them again. Leave those two
out if you don't use Claude Code. Then remove the fork's `PATH` line from your shell's startup file.

The database keeps the fork's schema. Upstream hasn't been tested with it, so restore your
`global.db.before-fork` backup if upstream misbehaves. Turns recorded since the backup are lost when you restore it.
