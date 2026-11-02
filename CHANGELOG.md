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

### Leads, milestones and releases

- The studio works like a company. You are the owner; a project can have a
  lead, a hire who plans its goals, assigns its tickets and answers its team's
  routine questions in place of the producer, router and approver, and sends
  you what it can't decide. Name one with `lead` in `studio.toml`, with
  `studio lead <project> [hire]`, or from the project's page in the web UI.
  The lead needs `tools: [file_ticket]` to plan, and never answers for its own
  tickets.
- A goal can carry a version and become a milestone: `studio goal
  --milestone 0.4`, or the Version field of New work. The project's sign and
  page show the milestone and how many of its tasks are done.
- A project whose `.studio.toml` lists `release` commands can be released:
  `studio release <project> <version>` runs the project's final checks, waits
  for your go-ahead (`studio release approve <project>`, or Approve in the web
  UI), runs the release commands with `VERSION` set, and tags `v<version>`.
  `studio release cancel` stops one before the go-ahead, and `studio releases`
  lists them. The tag is never pushed.

### Web UI

- A local web UI behind a token link. It opens on the company floor: one
  isometric floor with a team area for each project, around the owner's
  office, the lounge, and the review room and QA lab every project shares. Each
  project has a sign with its phase (Planning, Building, Testing, Releasing,
  Live, Paused or Idle), its milestone and progress, who is on it and a bell
  when it needs you. Everything on the floor is a control that opens its panel
  beside it, where you can act on it. Dark or light, and usable on a phone.
  `--host` serves it to your network; read docs/security.md first.
- The panel's tabs are For you, Projects, Company, Money and Log, and
  `/?tab=` and `/?sel=` links open any of them. For you gathers what waits for
  you across the projects: a release that wants your go-ahead, questions, held
  work, work ready to ship, work that failed, and how many routine questions
  your leads answered. Projects has a card for each project, with History at
  the end for merged goals and tickets that finished more than a day ago.
  Company shows who reports to whom and holds the desks, closing and opening
  the studio, rules, trophies and hiring.
- Each project has its own page: its pipeline, its milestone, the tasks in To
  do, Doing, Checking and Done, the release checklist with Start, Approve and
  Cancel, and its lead, which you can change there. The Hires and Rules pages
  edit your hires and rules, and each ticket has its own page with a live
  activity feed.
- You can close or reopen the studio (nothing new starts while it is closed),
  add or remove desks (how many agents work at once, up to nine), change the
  daily and per-ticket limits, and see each project's checks. Adding a project
  shows what it would add before you confirm.
- Trophies come from the studio's history, and each hire's level grows with
  the work it has landed.
- The floor turns to show each corner (the arrows, or Q and E) and can be
  dragged around when zoomed in. A new studio starts as a bare floor that grows
  as you add: projects and hires can be added from the web UI itself, and a
  "Start here" list leads the way.

### CLI and the rest

- `studio up` and `down`, `init`, `project add`, `ticket`, `goal`, `lead`,
  `release` and `releases`, `asks`, `approve`, `deny`, `land`, `merge`,
  `budget`, `close` and `open`, `doctor`, `export`, `gc` and more. `--json`
  prints the raw reply. `studio close` pauses the studio the way the web UI's
  Company tab does; `max_runs` in `studio.toml` is from 1 to 9.
- Packs bundle hires, rules, skills and a project template to reuse across
  projects.
- A project with `watch_ci` gets a fix ticket when CI fails on its integration
  branch. Desktop notifications when a ticket asks, is ready, is held or fails,
  and when a release waits for your go-ahead.
- Claude Code is the only runtime. It runs with your own `claude` login; the
  studio never reads your credentials.
