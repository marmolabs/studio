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

This writes `studio.toml`, five hires (engineer, producer, reviewer, approver,
router) and two rules to the studio folder: agents never `git push`, and they
may read the source of Rust dependencies without asking. It keeps any file that
already exists. The folder is `~/Library/Application Support/studio` on macOS
and `~/.config/studio` on Linux. Use `--home <dir>` or `STUDIO_HOME` to keep a
separate studio; its data folder is `<dir>/data`.

Open `studio.toml` and set the budget. The starter allows $10 a day and $2 per
ticket. See [configuration](configuration.md).

## Add a project

A project is a git repository. Add it from the company floor (the empty,
dashed team area asks for the folder and shows what it would add, with its
checks, before you confirm) or in a terminal:

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

## Start the studio

```sh
studio up
```

This starts `studiod`, the daemon that runs the agents, in the background,
then the web UI, and opens it in your browser. Ctrl-C stops the web UI; the
daemon keeps working. `studio up` again reopens the UI, and `studio down`
stops the daemon. Its log is `studiod.log` in the data folder. `studio status`
says what the studio is doing.

The daemon reads `studio.toml` and the hires again within a second of a
change, so adding a project or changing a budget needs no restart. Runtimes
and notifications are read once, at start.

## Your first ticket

From the web UI's New task form (the New work button makes a goal, below), or
in a terminal:

```sh
studio ticket -p my-app --hire engineer "Add a dark mode" \
  --detail "A switch in settings turns it on; it follows the system until then."
studio board
```

`--hire` is optional. Without it, the project's lead picks who gets the ticket
(see [Give the project a lead](#give-the-project-a-lead)), or the router hire
if the project has no lead.

The title is what the ticket is called. The detail is the full ask: context,
steps, what done looks like. For a long one, write it in a file and pass
`--detail-file brief.md`. `studio board` lists each ticket and its state.

When the agent wants to do something no rule allows, it asks. Answer in the
web UI (the For you tab, or the bell on a project's sign) or in a terminal:

```sh
studio asks
studio approve <ask>
studio deny <ask> -m "use the existing helper"
```

`deny` with a message refuses only that call and the agent carries on, told
your note. `deny` alone stops the ticket.

Follow what the agent did with `studio log <ticket>`. When the ticket is
ready, read the change (`studio diff <ticket>`), then:

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
resolves the conflicts; merge again once that ticket lands. Nothing is pushed.
A finished goal waits in the For you tab with a button to ship it; once its
branch is in the branch it started from, it leaves the tab.

## Give the project a lead

A lead is a hire who runs a project for you: it plans the project's goals,
hands its tickets to the team and answers the team's routine questions, and
sends you what it can't decide. It is optional; without one, the producer, the
router and the approver do those jobs for every project.

A lead is a hire like any other, so write its file in the studio's `hires/`
folder. Give it a strong model, and `tools: [file_ticket]` so it can file the
tickets of a plan:

```markdown
---
name: Manager
role: lead
runtime: claude-code
model: opus
tools: [file_ticket]
---
You lead this project. Plan its goals into small tickets with acceptance
criteria, give each to the teammate who suits it, and answer routine
questions. Pass up to the owner anything dangerous or irreversible.
```

Then name it, in a terminal or from the project's page in the web UI (pick the
project's sign, then Lead):

```sh
studio lead my-app manager
```

`studio lead my-app` with no hire clears it. Run `studio doctor` to check the
lead is a hire. See [Project leads](concepts.md#project-leads) for what changes.

## Your first goal

A goal goes to the project's lead, or to a producer hire if it has none. The
hire reads the code and files the tickets that reach it.

```sh
studio goal -p my-app --milestone 0.4 "Add a dark mode and a settings page"
```

The text names the goal and its branch, so keep it short. Give the planner the
full brief with `--detail` or `--detail-file brief.md`. `--milestone 0.4` makes
the goal a milestone towards version 0.4; in the web UI, the New work form has
a Version field for the same. The project's sign and page show the milestone
and how many of its tasks are done. Use `--hire` to name the planner yourself.

The tickets show up on the company floor, all landing on one goal branch. The
number of tickets one planner may file is `goals.max_filed` (default 8).

## Your first release

Tell the studio what publishes your project, in the project's `.studio.toml`:

```toml
verify = [["cargo", "test", "--locked"]]
release = [["./scripts/publish.sh"]]
```

Then release a version:

```sh
studio release my-app 0.4
```

The project's `verify` commands run first, in a fresh checkout of the tip of
its integration branch. When they pass, the release waits for your go-ahead
(the project's sign turns to "waits for you" and a card appears in For you):

```sh
studio release approve my-app
```

In the web UI that is the Approve button on the card or on the project's
release checklist. Each release command then runs in that checkout with
`VERSION=0.4` in its environment, and the commit is tagged `v0.4`. The studio
never pushes: pushing the tag is yours to do. `studio release cancel my-app`
stops a release before you approve it, and `studio releases` lists them all.
[Configuration](configuration.md#releases) has the details.

## The company floor

`studio up` opens it for you. To run the web UI alone, `studio-web` prints a
link like `http://127.0.0.1:4870/?token=...`. The token becomes a cookie, and
the page redirects without it. The link stays the same across restarts;
`studio up --new-token` makes a new one and the old link stops working.

`studio up --host <your LAN address>` lets a phone or another computer on
your network open the UI. Anyone on the network with the link can act as you,
and the link travels as plain HTTP, so use it only on a network you trust.

The page is one floor with a team area for each project, around the owner's
office, the lounge, and the review room and QA lab every project shares. Each
team area has a sign with the project's phase (Planning, Building, Testing,
Releasing, Live, Paused or Idle), its milestone with a bar, who is on it, and a
bell when it needs you. Pick anything on the floor and its panel opens beside
it. The bar on top counts projects, releases under way and what needs you, and
shows what you spent today; the arrows (or Q and E) turn the floor.

- **For you** is the first tab: a release that wants your go-ahead, asks, held
  work, work ready to ship and work that failed. **Projects** has a card for
  each project and, at its end, History: merged goals and tickets finished
  more than a day ago. **Company** shows who reports to whom and holds the
  desks (how many agents work at once), closing and opening the studio, rules,
  trophies and hiring. **Money** and **Log** hold spend, the limits, and what
  happened lately.
- **A project's page** opens from its sign or card. It shows the pipeline, the
  milestone, its tasks in To do, Doing, Checking and Done, the release
  checklist (Start a release, Approve, Cancel), the lead and a button for new
  work.
- **Closing the studio** (Company tab) stops anything new from starting while
  work already running carries on. `studio close` and `studio open` do the
  same in a terminal.
- **The hires and rules pages** (`/hires`, `/rules`) edit your hires and
  rules.
- **A ticket's page** (`/tickets/<id>`) shows its detail, any ask waiting on
  it, one live activity feed, and a rail with its status, hire, branch, spend
  against its cap, progress through checks, review and gates, and changed
  files.
- `/?tab=money` opens a tab, and `/?sel=tickets/<id>` or `/?sel=projects/<name>`
  opens a ticket's or a project's panel.

See [security](security.md) for how access works.

## Next

- [Concepts](concepts.md)
- [Configuration](configuration.md)
- [Packs](packs.md)
