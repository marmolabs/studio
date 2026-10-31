# Changelog

## 0.1.0 — unreleased

First release. Early; expect breaking changes.

### Board and tickets

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
  screenshots every web page against sample data. It needs a Chromium browser.

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
- A router hire picks the hire for a ticket made without one (`--hire` is now
  optional, the board has "Pick for me", and `file_ticket` may omit the hire).
  Without a router, the first engineer takes it.
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

- `studio-web` serves a local UI on 127.0.0.1. Pages: board, office, goals,
  asks, budget, hires and rules.
- Asks show the exact command or file, and why they asked. Approve, deny or
  "always allow" a command from the board, the ticket or the asks page.
- Each ticket shows its asks, what it is doing now, its screenshots, its diff
  file by file, a timeline and a run log. Land and merge from the page.
- The office draws the hires at their desks, live, with layouts to edit.
- Hires and rules can be edited in the browser. Hire edits apply without a
  restart.
- Access needs a one-time token link, which becomes a SameSite=Strict cookie.
  POSTs from other origins are refused.

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
