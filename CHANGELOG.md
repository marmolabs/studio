# Changelog

## 0.1.0

The first release. studio is very early: expect bugs, rough edges and breaking
changes between releases. Try it on a repo you have backed up, and keep its
budget limits low.

### Work

- Give the studio a goal and a producer hire files the tickets that reach it,
  all landing on one goal branch; or file a ticket yourself. A router hire
  picks who takes a ticket filed without a hire.
- Each ticket runs a Claude Code agent in its own git worktree and branch.
  Then the project's checks run, gates send matching changes to a critic hire
  (changed images beside their shipped versions, plus a `measure` command's
  output), and a reviewer on a different model reads the diff. Work that fails
  goes back to the agent, up to three tries, and can move to a stronger model.
  A project's `capture` command adds screenshots to the ticket, kept after it
  lands. Checks and captures get 30 minutes each, and agents wait for long
  builds rather than leave them running in the background.
- A project's `.studio.toml` can give its agents, checks and captures an
  `[env]`, where `{repo}` is the project's checkout. `studio project add`
  uses it to give a Cargo project's worktrees one build folder, so a ticket
  doesn't build from scratch.
- Ready work lands on its goal branch, by itself with `auto_land`, and
  `studio merge` brings a finished goal into your branch. The studio commits
  as you and never pushes.
- A ticket that was running when the daemon stopped is held, not lost.
  Tickets held for budget start again by themselves, and so do tickets held
  for Claude's usage limit, once the limit resets.
- When newer work on the goal branch conflicts with a ticket, landing merges
  it into the ticket's worktree and sends the ticket back to its agent to
  resolve. When merging a goal into your branch conflicts, the studio files a
  ticket that resolves it.

### Rules and asks

- Rules are Markdown files that protect paths, gate tools, lint changes, add
  checks, guide the agent or add review criteria, at the levels block, ask,
  warn or allow. They come from the studio, packs and each project.
- Anything no rule allows becomes an ask, which you answer in the web UI or
  the CLI. An approver hire allows routine calls, denies what a ticket doesn't
  need and sends only risky ones to you. Agents can also ask you a question.
- Denying an ask or sending work back drafts a rule, which you can test
  against past tool calls before saving it.

### Budget

- Limits over any rolling 24 hours for the studio, each project and each hire,
  plus limits per ticket and per run. A run starts only when its allowance
  fits under every limit, and live runs reserve theirs.

### Web UI

- A local web UI behind a token link. It opens on the Office: the whole studio
  as one isometric floor on a single page, with tickets at desks, in checks, in
  review and ready to land, goals on the whiteboard and idle hires in the
  lounge. Every object on the floor is a control that opens its panel beside
  it, where you can act on it; the panel's tabs are Needs you, Projects, Team,
  Money, Rules and Log, and `/?tab=` and `/?sel=` links open any of them.
  Board, Asks, Goals, Budget, Hires and Rules hold the lists and the detail,
  and each ticket has a live activity feed. Dark or light, and usable on a
  phone. `--host` serves it to your network; read docs/security.md first.
- From the Office you can close or reopen the studio (nothing new starts while
  it is closed), add or remove desks (how many agents work at once, up to
  nine), change the daily and per-ticket limits, focus the floor on one
  project, and see each project's checks.
- Trophies come from the studio's history, and each hire's level grows with
  the work it has landed.
- The office turns to show each corner (the arrows, or Q and E) and can be
  dragged around when zoomed in. A new studio starts as a bare room that grows
  as you add: projects and hires can be added from the office itself, and a
  "Start here" list leads the way.

### CLI and the rest

- `studio up` and `down`, `init`, `project add`, `ticket`, `goal`, `asks`,
  `approve`, `deny`, `land`, `merge`, `budget`, `doctor`, `export`, `gc` and
  more. `--json` prints the raw reply.
- Packs bundle hires, rules, skills and a project template to reuse across
  projects.
- A project with `watch_ci` gets a fix ticket when CI fails on its integration
  branch. Desktop notifications when a ticket asks, is ready, is held or fails.
- Claude Code is the only runtime. It runs with your own `claude` login; the
  studio never reads your credentials.
