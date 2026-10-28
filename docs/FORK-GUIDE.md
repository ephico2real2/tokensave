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

## 2. Run the fork, not upstream

An upstream tokensave (Homebrew, a release download) can stay installed, and it usually comes first on `PATH`.
From the checkout, run:

```sh
sh scripts/fork-path.sh
```

It detects your login shell (zsh, bash or fish) from `$SHELL`, finds its startup file, and prints the commands to
run: one that appends the `PATH` line to the end of that file (after Homebrew's setup), one that restarts the
shell, and two checks. It changes no file itself. Run it again afterwards: it says "Nothing to do" once
`tokensave` is the fork.

The checks it prints:

```sh
which tokensave                                            # ~/.cargo/bin/tokensave
tokensave dashboard --help > /dev/null && echo 'fork: ok'  # only the fork has this command
```

`tokensave --version` doesn't tell them apart: the fork reports the version of the upstream release it is
based on.

## 3. Point your agents at the fork

`install` and `reinstall` write the path of the binary that runs them into each agent's settings (hooks and MCP
server). Run them with the fork:

```sh
~/.cargo/bin/tokensave reinstall
grep -o '"command": *"[^"]*tokensave[^"]*"' ~/.claude/settings.json   # each line names ~/.cargo/bin/tokensave
```

Agent sessions that were already open keep running the build they started with until you restart them.
`tokensave servers` lists the MCP servers that are running.

## 4. First run

Back up the global database first. The fork migrates it to a newer schema the first time it opens it, and
upstream builds haven't been tested against a migrated database.

```sh
sqlite3 ~/.tokensave/global.db ".backup $HOME/.tokensave/global.db.before-fork"
tokensave cost                            # migrates, then reads Claude Code, Codex, Pi and Droid sessions
tokensave accounting backfill --dry-run   # what it would fill in older Claude Code turns
tokensave accounting backfill
```

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
tokensave cursor sync --use-login --dry-run   # fetch and count, write nothing
tokensave cursor sync --use-login
```

The first sync fetches all history. Later ones start an hour before the newest Cursor event recorded. Use
`--since YYYY-MM-DD` to choose the start. The credential is never printed or stored.

## 6. The dashboard

```sh
tokensave dashboard enable    # switch it on in ~/.tokensave/dashboard.toml and start it
tokensave dashboard open      # open http://127.0.0.1:7311/ in the browser
tokensave dashboard status
tokensave dashboard stop
```

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

## Going back to upstream

```sh
cargo uninstall tokensave
tokensave reinstall          # with the upstream binary, so the agents point at it again
```

The database keeps the fork's schema. Upstream hasn't been tested with it, so restore your
`global.db.before-fork` backup if upstream misbehaves. Turns recorded since the backup are lost when you restore it.
