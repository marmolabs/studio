# Getting started

## Requirements

- macOS or Linux. Windows is not supported: the daemon talks to its clients
  over a Unix socket.
- `git`.
- The [Claude Code](https://claude.com/claude-code) CLI (`claude`), installed
  and logged in. The studio uses your login and never reads your credentials.
- Optional: `gh`, if you want the studio to watch CI.

## Install

With the install script. It downloads a release, checks its SHA-256 sum, and
puts `studio`, `studiod` and `studio-web` in `~/.local/bin`. It never uses
`sudo`.

```sh
curl -fsSL https://raw.githubusercontent.com/marmolabs/studio/master/install.sh | sh
```

Set `STUDIO_INSTALL_DIR` to install somewhere else, or `STUDIO_VERSION` to pick
a release.

Or build from source. This needs Rust 1.98; the repo pins it, so `rustup`
fetches it.

```sh
cargo install --git https://github.com/marmolabs/studio studio studiod studio-web
```

Check the setup at any time with `studio doctor`.

## Set up the studio

```sh
studio init
```

This writes `studio.toml`, five hires (engineer, producer, reviewer, approver, router) and two
rules to the studio folder: agents never `git push`, and they may read the
source of Rust dependencies without asking. It keeps any file that
already exists. The folder is `~/Library/Application Support/studio` on macOS
and `~/.config/studio` on Linux. Use `--home <dir>` or `STUDIO_HOME` to keep a
separate studio.

Open `studio.toml` and set the budget. The starter allows $10 a day and $2 per
ticket. See [configuration](configuration.md).

## Add a project

A project is a git repository.

```sh
studio project add ~/games/my-game
```

This adds the project to `studio.toml` and, if the repo has none, writes a
`.studio.toml` with the commands that check its work. Read that file and fix
the `verify` commands if they are wrong.

To start from a pack instead, add the pack first, then name it:

```sh
studio pack add packs/bevy-ios
studio project add ~/games/my-game --pack bevy-ios
```

Start the studio:

```sh
studio up
```

This starts `studiod`, the daemon that runs the agents, in the background,
then the web UI, and opens it in your browser. Ctrl-C stops the web UI; the
daemon keeps working. `studio up` again reopens the UI, and `studio down`
stops the daemon. Its log is `studiod.log` in the data folder.

The daemon reads `studio.toml` again within a second of a change, so adding a
project or changing a budget needs no restart. Runtimes and notifications are
read once, at start.

Each ticket works in its own git worktree, so a Rust project builds from
scratch in every one. To share one build cache across them, start the studio
with a shared target folder. Agents and checks inherit it:

```sh
CARGO_TARGET_DIR=~/.cache/studio-target studio up
```

## Your first ticket

From the board's New ticket form, or in a terminal:

```sh
studio ticket -p my-game --hire engineer "Add a pause menu" \
  --detail "Escape opens it; it has Resume and Quit."
studio board
```

`--hire` is optional. Without it, the router hire picks who gets the ticket.

The title is what the ticket is called. The detail is the full ask: context,
steps, what done looks like. For a long one, write it in a file and pass
`--detail-file brief.md`.

The board lists each ticket and its state, under a strip that counts what is
running, what needs you and what is ready, and shows today's spend. Tickets
that need you or are ready to land come first; empty groups are left out. When the agent wants to do
something no rule allows, it asks:

```sh
studio asks
studio approve <ask>
studio deny <ask> -m "use the existing helper"
```

`deny` with a message refuses only that call and the agent carries on, told
your note. `deny` alone stops the ticket.

Follow what the agent did with `studio log <ticket>`. When the ticket is
ready, read the change in the worktree, then:

```sh
studio land <ticket>        # merge into the goal branch
studio changes <ticket> "the pause key should be Escape"   # or send it back
```

`land` merges into `studio/goal/<goal>`, so a goal's tickets collect on one
branch. When you're happy with it, bring it into the branch the work started
from:

```sh
studio merge <ticket>
```

That merges in your own checkout when the branch is checked out there, and
only if the checkout has no uncommitted changes. A conflict is backed out and
left to you. Nothing is pushed.

## Your first goal

A goal goes to a producer hire, which reads the code and files the tickets
that reach it.

```sh
studio goal -p my-game "Add a pause menu and a settings screen"
```

The text names the goal and its branch, so keep it short. Give the producer
the full brief with `--detail` or `--detail-file brief.md`.

The tickets show up on the board, all landing on one goal branch. The number
of tickets one planner may file is `goals.max_filed` (default 8).

## The web UI

`studio up` opens it for you. To run it alone, `studio-web` prints a link like
`http://127.0.0.1:4870/?token=...`. The token becomes a cookie, and the page
redirects without it. Each start makes a new token.

The UI has the board, the office, goals, asks, budget, hires and rules. Each
ticket has its facts as chips (with a live estimate and elapsed time while it
runs), a live run log that folds repeated tool calls into rows you can open, a
timeline and its evidence. See
[security](security.md) for how access works.

## Next

- [Concepts](concepts.md)
- [Configuration](configuration.md)
- [Packs](packs.md)
