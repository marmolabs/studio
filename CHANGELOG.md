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
  A project's `capture` command adds screenshots to the ticket.
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

- A local web UI behind a token link. The Office shows the whole studio on one
  floor, with a panel to act on anything you pick; Board, Asks, Goals, Budget,
  Hires and Rules hold the lists and the detail, and each ticket has a live
  activity feed. Dark or light, and usable on a phone. `--host` serves it to
  your network; read docs/security.md first.

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
