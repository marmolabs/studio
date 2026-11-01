<h1 align="center">studio</h1>

<p align="center">
  <b>Run a small team of AI coding agents on your projects, on your machine, within a budget and your rules.</b>
</p>

<p align="center">
  <a href="https://github.com/marmolabs/studio/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/marmolabs/studio/actions/workflows/ci.yml/badge.svg"></a>
  <a href="LICENSE"><img alt="MIT licence" src="https://img.shields.io/badge/license-MIT-blue.svg"></a>
  <img alt="Rust 1.98" src="https://img.shields.io/badge/rust-1.98-orange.svg">
  <img alt="macOS and Linux" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg">
  <img alt="Status: very early" src="https://img.shields.io/badge/status-very%20early-red.svg">
</p>

studio is a local orchestrator for AI coding agents. You give it a goal; it
splits the goal into tickets, runs an agent on each in its own git worktree,
runs your checks, has the change reviewed, and collects the result on a branch
for you to merge. Nothing is pushed, and agents never see your credentials.

> [!WARNING]
> **Very early. Use with caution.** studio runs AI agents with real tools on
> your repositories. Expect bugs and breaking changes between versions. Try it
> on a repo you have backed up, keep its budget low, set a spending limit in
> your Claude account too, and read what it lands before you merge.

## The office

<p align="center"><img src="docs/office.png" alt="The office: tickets at their desks, in checks, in review and ready to land, with what needs you beside the floor" width="900"></p>

The web UI opens on the office: the whole studio as one isometric floor.
Tickets sit at their desks, in the checks lab, in review and ready to land.
Goals are on the whiteboard, idle hires are in the lounge, and the approver and
router stand at the front desk. A bar on top shows what needs you, what is at
work, what shipped today and what you have spent.

Everything on the floor is a control. Pick a ticket, a hire, a goal or a piece
of furniture and its panel opens beside the floor, where you answer a question,
land a ticket, merge a goal, resume or cancel work, hire someone, or change the
budget. The panel's tabs hold the lists: Needs you, Projects (ending with
History), Team, Money, Rules and Log. The coffee machine closes the office so
nothing new starts. Each ticket also has its own page with a live activity
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

Then give it work, from the office or a terminal:

```sh
studio goal -p my-app "Add a dark mode with a switch in settings"
studio ticket -p my-app "Fix the login timeout"   # the router picks the hire
studio status                                     # what the studio is doing
```

Answer questions with `studio asks`, `studio approve` and `studio deny`, land a
ready ticket with `studio land <ticket>`, and bring a finished goal into your
branch with `studio merge <ticket>`. `studio doctor` checks the setup, and
`studio down` stops the daemon. [Getting started](docs/getting-started.md)
walks through it.

## How it works

- **Goals** go to a producer hire, which reads the code and files tickets. A
  goal's tickets run one after another and land on one goal branch.
- **Tickets** are one piece of work for one hire in one project. Each runs in
  its own worktree and branch. You can also file one yourself; a router hire
  picks who takes it.
- **Hires** are agent roles written as Markdown files: a model, a prompt and
  optional limits. The starter has an engineer, a producer, a reviewer, an
  approver and a router.
- **Rules** are Markdown files too: protect paths, gate commands, lint
  changes, require checks, guide the agent, or add review criteria. The
  approver (a cheap model) decides the tool calls no rule covers, so only
  dangerous ones reach you.
- **Checks and review.** When the agent is done, the project's checks run, then
  critics judge screenshots and design where a gate asks, then a reviewer on a
  different model reads the diff. A ticket that fails goes back to its agent,
  up to three tries, and can move to a stronger model.
- **Landing and merging.** A ticket that passes waits for you to land it on its
  goal branch (or lands by itself with `auto_land`). You merge the goal branch
  into your own branch when you like it.
- **Budgets** cap the studio, a project, a hire, a ticket and a run. A run only
  starts if its allowance fits under all of them.
- **Packs** bundle hires, rules, skills and a project template to reuse across
  projects.

## Docs

- [Getting started](docs/getting-started.md)
- [Concepts](docs/concepts.md)
- [Configuration](docs/configuration.md)
- [Packs](docs/packs.md)
- [Security](docs/security.md)
- [Contributing](CONTRIBUTING.md) and [Changelog](CHANGELOG.md)

## Status

0.1.0, very early: expect bugs, rough edges and breaking changes. Claude Code
is the only agent runtime for now. Linux release builds need glibc 2.34 or
newer (Ubuntu 22.04, Debian 12, RHEL 9 or later).

## License

MIT. See [LICENSE](LICENSE).
