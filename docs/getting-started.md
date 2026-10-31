# Getting started

## Requirements

- macOS or Linux. Windows is not supported: the daemon talks to its clients
  over a Unix socket. The Linux release builds need glibc 2.34 or newer
  (Ubuntu 22.04, Debian 12, RHEL 9 or later).
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

Or build from source. This needs Rust 1.98 or newer (`rustup update`).

```sh
cargo install --locked --git https://github.com/marmolabs/studio studio studiod studio-web
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
studio project add ~/code/my-app
```

This adds the project to `studio.toml` and, if the repo has none, writes a
`.studio.toml` with the commands that check its work. For a Cargo project it
also gives every ticket's worktree one build folder, so tickets after the
first don't build from scratch. Read that file and fix the `verify` commands
if they are wrong.

To start from a pack instead, add the pack first, then name it:

```sh
studio pack add ~/packs/my-pack
studio project add ~/code/my-app --pack my-pack
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
studio ticket -p my-app --hire engineer "Add a dark mode" \
  --detail "A switch in settings turns it on; it follows the system until then."
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
only if the checkout has no uncommitted changes. A conflict is backed out, and
the studio files a ticket on the goal that merges your branch into it and
resolves the conflicts; merge again once that ticket lands. Nothing is pushed. Once a goal branch is in the branch it started
from, the web UI shows "merged" in place of the merge button. Clients can ask
the daemon with the `MergedTickets` request, which lists the landed tickets
whose goal is merged.

## Your first goal

A goal goes to a producer hire, which reads the code and files the tickets
that reach it.

```sh
studio goal -p my-app "Add a dark mode and a settings page"
```

The text names the goal and its branch, so keep it short. Give the producer
the full brief with `--detail` or `--detail-file brief.md`.

The tickets show up on the board, all landing on one goal branch. The number
of tickets one planner may file is `goals.max_filed` (default 8).

## The web UI

`studio up` opens it for you. To run it alone, `studio-web` prints a link like
`http://127.0.0.1:4870/?token=...`. The token becomes a cookie, and the page
redirects without it. The link stays the same across restarts;
`studio up --new-token` makes a new one and the old link stops working.

`studio up --host <your LAN address>` lets a phone or another computer on
your network open the UI. Anyone on the network with the link can act as you,
and the link travels as plain HTTP, so use it only on a network you trust.

The link opens the Office: the whole studio on one floor, with tickets at
their desks, in checks, in review and ready to land, goals on the planning
whiteboard, idle hires in the lounge, and the approver and router at the front
desk. Pick anything to see it beside the floor and act on it there: allow or
deny an ask, land, resume, cancel, or give a hire work. "Last hour" under the
floor shows what each ticket did, minute by minute. The board, goals, asks,
budget, hires and rules pages have the lists and the detail. Each ticket page shows the ticket's detail, any ask waiting on it, and one live
activity feed: the studio's events, the agent's messages as Markdown, and its
tool calls folded into groups you can open. A rail beside it has the status,
hire and model, branch, try, spend against the ticket's cap, time, progress
through checks, review and gates, and the changed files. Ready and finished
tickets say how they ended and offer only the actions that still apply. See
[security](security.md) for how access works.

## Next

- [Concepts](concepts.md)
- [Configuration](configuration.md)
- [Packs](packs.md)
