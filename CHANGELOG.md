# Changelog

## 0.1.0 — unreleased

First release. Early; expect breaking changes.

### Board and tickets

- Web UI redesign, part one: the design tokens (dark by default, light under
  `prefers-color-scheme`) in the Tailwind theme, a 216px sidebar with the nav,
  asks badge, live status (with the queued count) and spend meter, a 52px top
  bar with the page title or `Board / GAME-3` and the page's actions (New
  ticket, New goal, New, Edit, Cancel, Land, Resume, Retry), and shared
  components (buttons, status dot and pill, progress track, section heading,
  lists and tables, rail card) used across the pages, which now take every
  colour from the tokens. The old top nav is gone; below 760px the sidebar
  becomes a top bar with a scrolling nav.
- Web UI design pass: shared text tones, type scale, buttons (with a quiet
  variant), badges and a tighter header with a "N running" cue, stronger
  contrast in light and dark, and no sideways scroll at phone width.
- Budget and Hires pages: the budget leads with spent plus held-by-live-runs
  against the daily limit (with what is left), one line of limits instead of
  stat cards, and dense spend rows by project and hire; held tickets link with
  their titles. Each hire card shows what it is doing now (ticket and latest
  activity, or "Idle") and what it has done: tickets done, spend today and
  pass rate, or "No tickets yet".
- Ticket page: the facts (project, hire, base, attempts, spend, goal) are
  labelled chips; a running ticket shows its estimate as "Est. spend", the
  elapsed time and the ETA; the ask card no longer repeats the ticket id. The
  run log folds runs of the same tool into one row ("Read ×6") that opens to
  the calls, folds the "allowed ..." rule lines into the group they let
  through, sets the agent's words apart from muted tool lines, opens at the
  newest entry, and scrolls a long command inside its own line.
- Ticket page: the agent's messages in the run log, feedback, and review notes
  in the timeline are rendered as Markdown on the server (bold, lists, code,
  links), keeping line breaks; raw HTML in the text is shown escaped.
- Ticket page: an "Art · shipped vs new" section above the screenshots pairs
  `before-<key>` and `after-<key>` evidence files, shipped on the left and new
  on the right, with "new file" for an after with no before. Those files no
  longer appear among the screenshots.
- Board: a live summary strip (running, need you, ready, spend vs the daily
  limit), "Needs you" and "Ready to land" on top in amber and emerald, empty
  columns left out, and an ask's command scrolls instead of wrapping mid-word.
- Goals, Asks and Rules pages: each goal is a compact card with its state,
  tickets landed of total with a thin bar, spend, the goal branch as a small
  chip and a link to its plan; asks show the ticket once, the command or path
  in a block that scrolls inside itself, and Allow/Deny stacked full width on
  a phone; rules are a list with a text preview and Edit and Delete actions.
  Empty lists are one quiet line.
- A page gallery (`cargo run -p studio-web --example gallery -- <dir>`)
  screenshots every web page against sample data with a Chromium browser, or
  with Quick Look on macOS when no browser can run.

- Tickets move through a fixed state machine: queued, claimed, working,
  checking, reviewing, ready, landed. They can also be asking, held, failed
  or cancelled.
- Each ticket works in its own git worktree on `studio/<ticket>`.
- A ticket has a title and an optional detail with the full ask
  (`--detail`, `--detail-file`, or the board's form).
- The studio commits as you. Nothing is pushed.
- Tickets with the same goal land on one branch, `studio/goal/<goal>`.
  They run one at a time, and each starts from the goal branch once it exists.
- `auto_land = true` in `.studio.toml` lands a ready ticket on its goal branch
  by itself. Merging into your branch stays `studio merge`.
- Tickets held for budget resume on their own when a run is funded again,
  checked once a minute.
- Denying a permission with a note refuses that call and lets the agent carry
  on; without a note the ticket stops.
- Ask ids (`a1`, `a2`, ...) continue after a restart instead of repeating.
- A failed ticket gets up to three attempts. A hire can name stronger models
  to move to after each failure (`escalate`).
- Quotes and finish-time estimates come from past runs.
- Events live in a SQLite store. A ticket that was running when the daemon
  stopped is held, not lost. `max_runs = 0` pauses the studio, and SIGTERM
  stops agents.
- Agents get a small tool server (`studio mcp`) to read their rules, load
  skills, ask the owner a question and, for producer hires, file tickets.

### Budget

- Limits for the studio, project and hire over any rolling 24 hours, plus
  limits per ticket and per run.
- A run starts only with an allowance that fits under every limit at once.
  Live runs reserve their allowance.
- A model that can't be priced doesn't run, unless the runtime reports cost.
- `studio budget` shows limits, 24-hour spend and reservations.

### Rules and asks

- Rules are Markdown files with frontmatter: `protect`, `gate`, `lint`,
  `check`, `guide` and `review`, at levels `block`, `ask`, `warn` and `allow`.
- Rules load from the studio, from packs and from each project. Block beats
  ask beats allow. Pack rules can be disabled by id.
- Anything no rule allows becomes an ask. By default, edits inside the
  worktree are allowed and everything else asks.
- Lint and protect rules also check what is about to be committed.
- A `bash:` rule that allows is checked against every part of a chained
  command, and anything with substitutions or redirects asks. Block and ask
  rules match anywhere in the command.
- A project's `verify` commands run without asking, and so do `head`, `tail`,
  `wc` and `grep` reading their piped output.
- Gates can name several tools (`Read|Grep`) and be limited to paths, with
  `~/` for the home folder. `studio init` allows reading Rust dependency
  sources.
- `protected` paths in `.studio.toml` ask before an agent changes them.
- Approve or deny asks from the CLI or the web UI. Agents can ask the owner
  a question with options.
- An approver hire decides tool calls no rule covers: it allows normal work,
  denies what the ticket doesn't need (the ticket keeps going) and sends only
  dangerous calls to the owner. Owner rules and agent questions always reach
  the owner. Errors, no budget or an unclear answer fall back to the owner.
  Each call is capped at $0.05 and charged to the ticket. Delete
  `hires/approver.md` to turn it off.
- The approver's decision line is read through Markdown dressing
  (`**DECISION:** ALLOW`). A reply with no clear decision gets one more ask
  for just the line; if that is unclear too, the call goes to the owner with
  the first reply quoted.
- A router hire picks the hire for a ticket made without one (`--hire` is now
  optional, the board has "Pick for me", and `file_ticket` may omit the hire).
  Without a router, the first engineer takes it.
- The router never gives a ticket to a hire that a project's gate names as
  its critic.
- `studio init` writes `approver` and `router` hires (Haiku, low effort).
- A hire's `budget_per_ticket_usd` caps only that hire's runs on a ticket,
  so a reviewer with a small cap can still review work that cost more.
- A reviewer or gate whose verdict came in before its cap stopped the run
  is taken at its word instead of holding the ticket, also when the verdict
  only came in the agent's streamed text.
- A reviewer runs on a model other than the one that wrote the attempt it
  reviews. If the author escalated onto the reviewer's model, it reviews on
  the first of its own `escalate` models instead of being skipped. The
  starter reviewer falls back to Sonnet.
  Work with no reviewer at all is never auto-landed.
- Editing a hire file on disk applies within a second, like `studio.toml`.
- `studio goal` takes a brief for the producer (`--detail`, `--detail-file`),
  and long goal names are cut so their branch names stay valid.
- The approver's calls for a running ticket are covered by that ticket's own
  reservation, so a busy day no longer sends every ask to the owner.
- Denying an ask or sending a ticket back drafts a rule you can edit and
  save: "Make this a rule" in the web UI, `--rule` on `studio deny` and
  `studio changes`.
- Test a rule against past tool calls before saving it: the Test button in
  the rules editor, or `studio rule test`. A rule with a bad regex or path
  glob can't be tested.
- Saving a rule checks its path globs as well as its regex, so a rule with a
  bad glob no longer saves.
- A denied `Grep` or `Glob` call drafts a gate for the tool, not a `protect`
  rule. `studio deny --rule` on an agent's question is an error.

### Goals

- `studio goal` gives a producer hire a goal. It reads the code and files the
  tickets that reach it, up to `goals.max_filed`.

### Review and gates

- A project's `verify` commands, and any `check` rules, run after each
  attempt. Failures go back to the agent.
- A reviewer hire on a different model reads the diff. Failed reviews go back
  to the author.
- Gates in `.studio.toml` send changes that touch matching files to a critic
  hire before the owner sees them.
- A gate's critic avoids only the model that wrote the latest attempt. If that
  is its own model, it judges on the first of its `escalate` models the author
  didn't use, and the gate is skipped only when it has none.
- A gate's critic gets each changed png, jpg or gif beside its shipped version
  (or marked as a new file), up to 12 per gate.
- A gate can name a `measure` command. It runs on up to 12 changed files that
  match the gate (`{file}` becomes each path) and its output goes to the critic.
- A project can name a `capture` command. Its screenshots are kept as
  evidence on the ticket.
- `studio land` merges a ready ticket into its goal branch.
  `studio changes` sends it back with feedback. `studio diff` shows the change.
- `studio merge` brings a landed goal into the branch its work started from,
  in your checkout only when it has no uncommitted changes.
- Commits use the ticket title, with a `Studio-Ticket:` trailer.

### Packs

- A pack bundles hires, rules, skills and a `.studio.toml` template.
  `studio pack add` registers one; `studio project add --pack` uses it.
- Ships `packs/bevy-ios`: engineer, producer, reviewer, artist, art critic and
  store hires, rules that protect signing files and the Xcode project, and
  skills for capture mode and iOS releases.
- Ships `examples/starter-2d`, a small Bevy game ("Lantern") with a capture
  mode, to try the studio on.

### Web UI

- `studio-web` serves a local UI on 127.0.0.1. Pages: the office (the home
  page), board, goals, asks, budget, hires and rules.
- Asks show the exact command or file, and why they asked. Approve, deny or
  "always allow" a command from the board, the ticket or the asks page.
- Each ticket shows its asks, what it is doing now, its screenshots, its diff
  file by file, a timeline and a run log. Land and merge from the page.
- A goal already merged into its base branch shows a quiet "merged" label
  instead of the merge button (new `MergedTickets` request). Spend that rounds
  to zero reads "$0.00", never "$-0.00". "Attempts" counts every run a ticket
  has had, sent back or retried, and is hidden until it has run.
- The office shows the whole studio as one floor: tickets at desks (a free
  desk for each run slot no run holds) with their ask over their head, in
  checks with the command running now, in review with the reviewer or critic
  at the table, ready to land, up next and shipped in the last 24 hours; open
  goals on the planning whiteboard; idle hires in the lounge; the approver
  (calls decided, and those sent to you) and router (tickets routed) at the
  front desk. Pick anything to see its panel beside the floor and act on it
  there (allow, deny, land, merge, cancel, resume, give a hire work); the
  floor, the top bar's figures and the panel stay live. The sidebar folds to
  an icon rail on this page, and an Office/Board switch sits in the top bar
  of both. The old pixel-art office and its layouts are gone.
- The office is the home page (`/`), so the token link lands on it; the board
  moved to `/board`. `/office` (keeping `?sel=`) and `/live` lead home.
- The Live page is folded into the office: its last hour, one lane per
  ticket (working, checks, critic or review, waiting for you, on hold, sent
  back, ready to land) under a legend, fills the office's "Last hour" drawer
  and stays live. The floor and the inspector cover what its cards and feed
  showed. studiod's `Activity` request gives the ticket histories the lanes
  draw.
- An ask, and an agent's latest action, read the same on the office, the
  board, the ticket page and the asks page ("Wants to run `cargo test`",
  "Reading src/a.rs").
- Hires and rules can be edited in the browser. Hire edits apply without a
  restart.
- Access needs a token link, which becomes a SameSite=Strict cookie.
  POSTs from other origins are refused.
- New design, part one: the Radix-based tokens of `design/prototype.html`
  (dark by default, light by the system or forced with the sidebar's theme
  switch, kept in a `theme` cookie), IBM Plex Sans and Mono served by the app,
  Lucide icons and state glyphs inlined once per page, a 232px sidebar with
  icons and counts, a sticky top bar, and shared components
  (state glyphs and rings, pills, avatars, chips, KPI strip, grouped rows,
  step bar, meters, rail properties, ask card, popover menu and dialog).
- studiod: `SpendByDay` gives spend per 24-hour window for the last days;
  `Budget`'s 24-hour spend also splits by model and by ticket; `Status` counts
  goals and rule files.
- The board, rebuilt to the new design: four figures (what needs you and the
  oldest wait, what is running by stage, what is ready to land with its diff
  size, and the last 24 hours' spend with 14 days of it as a sparkline), then
  one list grouped as Needs you (each ask in one line with its reason, and
  Deny and Allow; held tickets with Resume), Running (the latest action, the
  step and try, who is at work, time and spend), Ready to land (what passed,
  the diff size, the goal, spend and Land) and Up next (what each ticket waits
  for), then what finished in the last 24 hours as a table. New ticket and New
  goal open as dialogs from the top bar; a goal takes a brief for the
  producer.
- Tickets held for budget or quota start again by themselves, so every page
  draws them as "On hold" in grey, apart from what needs you; the board lists
  them under Up next.
- Ticket and Asks pages rebuilt to the new design. A ticket shows who filed
  it and from which goal, its detail as Markdown, the ask card (the command or
  the edit as a diff, what the agent said just before asking, why it reached
  you with a link to the rule, Allow, Deny, Always allow and a note), and one
  live Activity feed of studio events, agent messages and grouped tool calls
  with each call's result. The rail has status, hire and model, goal, branch,
  try, spend of the ticket cap, time, the ticket's own stages (checks, review,
  each gate, land) and the changed files. Finished tickets state the outcome
  and keep only the actions that still apply; Pause, Resume and Cancel are in
  the "more" menu, and "Export as Markdown" downloads what `studio export`
  prints. The Asks page lists full cards, newest first, with their tickets.
- `studiod`: a `Pause` request stops a running ticket's agent and holds it
  until it is resumed, and each ask says when it was asked.
- Goals and Budget pages in the new design. Goals is one table: each goal
  with its project and branch (or what it was merged into), progress as n of
  m landed, spend, state (planning, running, needs you, queued, done,
  merged), when it last changed, and Merge only once it is done and not yet
  merged; a goal opens to its plan and tickets as nested rows, and New goal
  opens a dialog with a brief for the producer. Budget shows spent today of
  the daily limit, what live runs hold, the last 7 days with the daily
  average and the other limits, a 14-day bar chart of daily spend on a dollar
  axis with the daily limit, and today's spend by hire (with runs), by ticket
  (with its outcome) and by model. studiod reports runs per hire in
  `Budget` and when each ticket last changed (`UpdatedAt`).
- New design: Hires is a table of the team (prompt's first line, model and
  escalation, effort, the ticket it is on now, what it did today, spend
  today, Edit) with the approver and router apart, showing their calls and
  routings; editing a hire fills the page. Rules lists every rule file by
  where it comes from (the studio, each pack, each project) with its kind and
  level, and opens the first in an editor beside the list; Delete asks first.
  Pack and project rules are read-only there.
- studiod: `RuleFiles` lists the rule files of the studio, its packs and its
  projects (`Status` counts all of them); `HiresToday` counts what each hire
  did in the last 24 hours from events and run logs. The approver's run-log
  note now also records calls it left to the owner.
- studiod: `Status` gives the run slots (`max_runs`) and each hire's role and
  model (`team`); `RecentEvents` gives every ticket's events since a time,
  optionally only the newest few or only some kinds; `Diffstats` gives
  several tickets' diffstats in one request; the run log names each project
  check as it starts (`check: <command>`).
- The token is kept in `web-token` in the data folder (mode 0600), so the
  link stays the same across restarts. `--new-token` on `studio-web` and
  `studio up` makes a new one and retires the old link.
- `--host` on `studio-web` and `studio up` listens on another address, such
  as your LAN address, to reach the UI from another device. Anyone on the
  network with the link can act as you; see docs/security.md.

### CLI

- `studio up` starts the daemon in the background and opens the web UI;
  `studio down` stops it. `studiod` reloads `studio.toml` when it changes.
- `studio init`, `project add`, `pack add`, `status`, `ticket`, `goal`,
  `board`, `asks`, `approve`, `deny`, `land`, `changes`, `cancel`, `retry`,
  `resume`, `budget`, `log`, `diff`, `merge`, `doctor`, `export` and `gc`.
  Replies are readable text; `--json` prints the raw reply.
- `studio doctor` checks the home folder, projects and runtimes, and works
  without the daemon.
- `studio export` prints a ticket as Markdown for a pull request.
- `studio gc` removes worktrees and branches that finished tickets left behind.
- `--home` or `STUDIO_HOME` keeps a separate studio.

### Watchers

- With `watch_ci = true` on a project, `studiod` polls CI with `gh` and files
  a fix ticket when a run fails on the integration branch.
- Desktop notifications (`[notify] desktop = true`, on in the starter).

### Runtimes

- Claude Code, using your own `claude` login. The studio never reads your
  credentials. Hires can set `effort`. Agents get only the studio's MCP tools
  unless a hire sets `user_mcp: true`.
