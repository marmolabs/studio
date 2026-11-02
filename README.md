<h1 align="center">studio</h1>

<p align="center">
  <b>Run your projects like a company of AI coding agents, on your machine, within a budget and your rules.</b>
</p>

<p align="center">
  <a href="https://github.com/marmolabs/studio/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/marmolabs/studio/actions/workflows/ci.yml/badge.svg"></a>
  <a href="LICENSE"><img alt="MIT licence" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="Rust 1.98" src="https://img.shields.io/badge/rust-1.98-orange.svg">
  <img alt="macOS and Linux" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg">
  <img alt="Status: very early" src="https://img.shields.io/badge/status-very%20early-red.svg">
</p>

studio is a local orchestrator for AI coding agents, run like a small company.
You are the owner. Each project can have a lead: an agent that plans the
project's goals, hands the tasks to its team and answers the routine questions,
and passes up to you only what it can't decide. Under the lead are the makers.
Each takes one ticket in its own git worktree, runs your checks, gets the
change reviewed and leaves the result on a branch for you to merge. When a
version is ready, the studio runs the project's release commands after your
go-ahead and tags it. Nothing is pushed, and agents never see your credentials.

> [!WARNING]
> **Very early. Use with caution.** studio runs AI agents with real tools on
> your repositories. Expect bugs and breaking changes between versions. Try it
> on a repo you have backed up, keep its budget low, set a spending limit in
> your Claude account too, and read what it lands before you merge.

## The company floor

<p align="center"><img src="docs/office.png" alt="The company floor: a team area with a sign for each project, around the owner's office, the lounge, the review room and the checks lab, with the panel beside it" width="900"></p>

The web UI opens on the company floor: one isometric floor with a team area for
each project, around the owner's office, the lounge, and the review room and QA
lab every project shares. Each team area has a sign that tells you the
project's **phase** (Planning, Building, Testing, Releasing, Live, Paused while
the studio is closed with work open, or Idle), its **milestone** and its
version with a bar for the tasks done, **who is on it** (the lead first), and a
**bell** when something in that project needs you.

Everything on the floor is a control that opens its panel beside it, and the
bar on top counts projects, releases and what needs you. The panel's tabs are
**For you** (what waits for you across the projects: a release that wants your
go-ahead, questions, held work, work ready to ship, work that failed),
**Projects** (a card for each, with History), **Company** (who reports to whom,
desks, closing the studio, rules, trophies, hiring), **Money** and **Log**.

Pick a sign to open the **project's page**: its pipeline from Planning to
Live, its milestone, the tasks in four columns (To do, Doing, Checking and
Done), the release checklist with Start, Approve and Cancel, and its lead,
whom you can change there. Each task has its own page with a live activity
feed.

## Install

You need macOS or Linux (not Windows), `git`, and the
[Claude Code](https://claude.com/claude-code) CLI (`claude`), installed and
logged in. The studio runs it with your login and never reads your credentials.

From a release (checks `SHA256SUMS`, installs to `~/.local/bin`, never uses
`sudo`):

```sh
curl -fsSL https://raw.githubusercontent.com/marmolabs/studio/master/install.sh | sh
```

Or from source, with Rust 1.98 or newer:

```sh
cargo install --locked --git https://github.com/marmolabs/studio studio studiod studio-web
```

## Quick start

```sh
studio init                         # studio.toml, five hires, two rules
studio project add ~/code/my-app    # register a git repo
studio up                           # start the daemon and open the web UI
```

Then give it work, from the web UI or a terminal:

```sh
studio lead my-app manager                       # optional: a lead for the project
studio goal -p my-app --milestone 0.4 "Add a dark mode with a switch in settings"
studio ticket -p my-app "Fix the login timeout"  # the lead or router picks the hire
studio status                                    # what the studio is doing
```

Answer questions with `studio asks`, `studio approve` and `studio deny`, land a
ready ticket with `studio land <ticket>`, and bring a finished goal into your
branch with `studio merge <ticket>`. To release, list the commands in the
project's `.studio.toml` (`release = [["./publish.sh"]]`), run
`studio release my-app 0.4`, then `studio release approve my-app`.
[Getting started](docs/getting-started.md) walks through it, including how
to write a lead's hire file.

## How it works

- **Leads** are hires you name for a project (in `studio.toml`, with
  `studio lead`, or on the project's page). A lead plans the project's goals,
  assigns its tickets and decides the routine tool calls its team asks about.
  Without one, a producer, a router and an approver do those jobs.
- **Goals** are planned into tickets that run one after another and land on one
  goal branch. A goal with a version is a milestone, and its sign shows how far
  along it is.
- **Tickets** are one piece of work for one hire in one project, each in its
  own worktree and branch. **Hires** are agent roles written as Markdown files:
  a model, a prompt and optional limits.
- **Rules** are Markdown files too: protect paths, gate commands, lint
  changes, require checks or guide the agent. The lead (or the approver)
  decides the tool calls no rule covers, so only dangerous ones reach you.
- **Checks and review.** The project's checks run, critics judge screenshots
  and design where a gate asks, then a reviewer on a different model reads the
  diff. A ticket that fails goes back to its agent, up to three tries.
- **Landing and merging.** A ticket that passes waits for you to land it on its
  goal branch (or lands by itself with `auto_land`). You merge the goal branch
  into your own branch when you like it.
- **Releases.** The final checks run, the release waits for your go-ahead, the
  project's release commands run with `VERSION` set, and the commit is tagged
  `v<version>`. The tag is never pushed.
- **Budgets** cap the studio, a project, a hire, a ticket and a run. **Packs**
  bundle hires, rules and a project template to reuse.

## Docs

- [Getting started](docs/getting-started.md)
- [Concepts](docs/concepts.md)
- [Configuration](docs/configuration.md) and [Rules](docs/rules.md)
- [Packs](docs/packs.md)
- [Security](docs/security.md)
- [Contributing](CONTRIBUTING.md) and [Changelog](CHANGELOG.md)

## Status

0.1.0, very early: expect bugs, rough edges and breaking changes. Claude Code
is the only agent runtime for now. Linux release builds need glibc 2.34 or
newer (Ubuntu 22.04, Debian 12, RHEL 9 or later).

## License

MIT. See [LICENSE](LICENSE).
