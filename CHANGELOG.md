# Changelog

## Unreleased

### Autonomous company operations

- Project leads can run persistent, budget-capped manager reviews and propose
  goals, tasks and task-management actions. HQ is the approval inbox for those
  proposals, and every decision and outcome is kept in an audit log.
- Automatic reviews are opt-in, with configurable cadence and supervised,
  balanced and autonomous levels. Cancellation, shipping, publishing and budget
  changes always require the owner.
- Company and project rooms provide persistent announcements and cross-team
  conversations. Mentioning a hire starts a read-only reply; plain announcements
  cost nothing.
- The company page adds a live goal/task work map, owner and blocker details,
  ticket spend, a delivery and cost scorecard, employee workload, and the
  manager decision log.
- Look closer now includes each task's screenshots and clips beside its checks,
  review, diffs, shipping and request-changes controls.
- Studio settings bring office pause, desks, budgets, manager cadence and
  autonomy together. Reviews and actions also appear in their project rooms.
- A simulation/live switch lets new conversations and reviews use local scripted
  responses with zero model cost, while preserving company history and leaving
  already-running agents to finish.

## 0.1.0

The first release. studio is very early: expect bugs, rough edges and breaking
changes between releases. Try it on a repo you have backed up, and keep its
budget limits low.

### Work

- Give the studio a goal and a producer hire files the tasks that reach it,
  all landing on one goal branch; or file a task yourself. A router hire
  picks who takes a task filed without a hire.
- Each task runs a Claude Code agent in its own git worktree and branch.
  Then the project's checks run, gates send matching changes to a critic hire
  (changed images beside their shipped versions, plus a `measure` command's
  output), and a reviewer on a different model reads the diff. Work that fails
  goes back to the agent, up to three tries, and can move to a stronger model.
  A project's `capture` command adds screenshots to the task, kept after it
  lands. Checks and captures get 30 minutes each, and agents wait for long
  builds rather than leave them running in the background.
- A project's `.studio.toml` can give its agents, checks and captures an
  `[env]`, where `{repo}` is the project's checkout. `studio project add`
  uses it to give a Cargo project's worktrees one build folder, so a task
  doesn't build from scratch.
- Ready work lands on its goal branch, by itself with `auto_land`, and
  `studio merge` brings a finished goal into your branch. The studio commits
  as you and never pushes.
- A task that was running when the daemon stopped is held, not lost. Tasks
  held for budget start again by themselves, and so do tasks held for
  Claude's usage limit, once the limit resets.
- When newer work on the goal branch conflicts with a task, landing merges
  it into the task's worktree and sends the task back to its agent to
  resolve. When merging a goal into your branch conflicts, the studio files a
  task that resolves it.

### Rules and asks

- Rules are Markdown files that protect paths, gate tools, lint changes, add
  checks, guide the agent or add review criteria, at the levels block, ask,
  warn or allow. They come from the studio, packs and each project.
- Anything no rule allows becomes an ask, which you answer on HQ, in the
  project's conversation or in the CLI. A project's lead (the approver hire,
  if it has none) allows routine calls, denies what a task doesn't need and
  sends only risky ones to you, each with the question in a few plain words
  ("Send the launch email?"). Agents can also ask you a question.
- Denying an ask or sending work back drafts a rule, which you can test
  against past tool calls before saving it.

### Budget

- Limits over any rolling 24 hours for the studio, each project and each hire,
  plus limits per task and per run. A run starts only when its allowance fits
  under every limit, and live runs reserve theirs. What you spend talking to a
  lead counts against its project's limit.

### Leads, milestones and releases

- The studio works like a company. You are the owner; a project can have a
  lead: a hire you talk to, who plans its goals, assigns its tasks, answers its
  team's routine questions in place of the producer, router and approver, and
  sends you what it can't decide. Name one with `lead` in `studio.toml`, with
  `studio lead <project> [hire]`, by hiring one from New project in the web UI,
  or from the project's conversation (below). The lead needs
  `tools: [file_ticket]` to plan, and never answers for its own tasks.
- A goal can carry a version and become a milestone: `studio goal
  --milestone 0.4`, or ask its lead for it ("Plan the checkout flow for 0.5").
  The lead's page, its line in the sidebar and its sign on the office floor
  show the milestone and how many of its tasks are done.
- A project whose `.studio.toml` lists `release` commands can be released:
  `studio release <project> <version>` runs the project's final checks, waits
  for your go-ahead (`studio release approve <project>`, or Publish on its
  card in the web UI), runs the release commands with `VERSION` set, and tags
  `v<version>`. `studio release cancel` stops one before the go-ahead (Not
  yet, in the web UI), and `studio releases` lists them. The tag is never
  pushed.

### Talking to your leads

- A project with a lead is a conversation. In the web UI pick the lead in the
  sidebar; in a terminal, `studio talk <project> "How's it going?"` sends the
  message and waits for the reply, and `studio talk <project>` shows the last
  messages. The lead reads the project (its code, its tasks, what waits for
  you, what was done today and what was spent) and answers in a few short
  sentences, in the first person.
- A lead can also act, and says what it did, each as a card in the
  conversation: start a goal (with a version, to make it a milestone), send a
  task back with a note, pause, resume or cancel a task, or tell a running
  agent something. A lead only reads: it never edits a file, runs a command or
  browses, and it can act only on its own project's tasks. A project with no
  lead has nobody to talk to: HQ says so under its line, with a link, and its
  conversation has a card in place of the message box, with two ways to give it
  one. Hire a lead takes a name (the studio suggests one that nobody on the team
  has) and makes a new hire on a strong model, with the tool it needs to plan,
  the project's lead. Pick one you have lists the hires with the role `lead`.
  Change lead in the header of a project that has one picks another, hires a
  new one or leaves the project with none. The conversation turns into one
  with the new lead as soon as it is named, and a lead just given a project
  offers to look at how it runs, which sets the project up as New project does.
  What you type in any of these stays as you left it while the page keeps up.
- The thread also holds what waits for you in that project: questions, work
  ready to ship and a release waiting for your go-ahead, as cards. A rail
  beside it shows who is at work and for how long, the milestone with its
  tasks ticked off, and what the project spent today.
- A reply costs about $0.10 to $0.25 on a strong model, is capped at $0.50 and
  counts against the project's daily limit. One reply is written at a time for
  each project: a message sent meanwhile is answered next.
- You, or a lead for you, can tell a task's agent something while it works:
  the message button beside it in the rail, or "Tell Dev to use the existing
  helper". A running agent hears it in its live session. A task in checks,
  review or ready goes back to its agent with the message as the note, a
  queued or held one keeps it for when it next starts, and a finished one
  can't be told. The task's run log notes it, "Message from the owner: ...".
- Every hire also has a persistent, read-only conversation at `/people/<hire>`.
  Pick a person or their task in the office to talk to that person, see their
  current and recent work, and ask about their assignments and results. Direct
  replies are capped at $0.25 and count against the relevant budgets. Steering
  an active run remains a separate task-message action.
- HQ's Company activity feed shows the last 24 hours across every project,
  newest first, with links to the person and task behind each event.

### Looking closer and shipping

- Work that is ready to ship, a task or a whole goal, opens in Look closer: a
  sheet over the conversation with the lead's one-sentence summary of it,
  three proofs (the checks, the review, and whether protected paths were
  touched and you allowed it), the tasks with the lines each added and
  removed, and the files with their diffs. A goal's files are what its branch
  adds to the branch it ships to, read from git, so they are still there when
  its tasks are done and their worktrees are gone. From the sheet you ship the
  work, send it back with a note, copy the command that switches to its branch
  to try it first, or open it in your editor.
- When every task of a goal is done, the studio runs the project's checks once
  more on the goal's branch, in a fresh checkout, the way a release's final
  checks run, and keeps the result until the branch moves. Look closer shows it
  as the checks proof, and HQ says what failed. A goal whose checks failed can
  still be shipped, but the page says so first.

### Setting a project up

- New project takes a folder and a lead for it, either a new hire on a strong
  model with a name the studio suggests or a lead you already have. It adds
  the project and opens its conversation, where the lead greets you, reads the
  repository and shows how the project would run: the checks it proposes, the
  command that takes screenshots if there is one, the release steps and where
  it found them, the paths to ask you about first and a daily budget. The
  studio does not take its word for it: it tries each check and the capture in
  a fresh checkout and shows how each went ("112 pass", or "failed"). Release
  commands are not run.
- Use these writes the checks, the screenshot command, the release steps and
  the protected paths to the project's `.studio.toml` (keeping what else is in
  it) and the budget to `studio.toml`; none of it is written before. Change
  something has you tell the lead what to change, and See the file shows what
  would be written. The lead then offers a first milestone to plan. A lead
  that can't finish says why and can try again. A setup is capped at $0.60.

### Web UI

- A local web UI behind a token link: a team sidebar and a page for each thing
  you do. Dark or light, and usable on a phone, where the sidebar stacks above
  the page. `--host` serves it to your network; read docs/security.md first.
- The sidebar is on every page. It has the studio's name, HQ with a count of
  what waits for you, a lead for each project (its face, name, project, its
  phase in a word such as Building, Testing, "v1.2 waits for you", Live v1.0
  or Setting up, a thin bar for its milestone and a count when something waits
  for you), New project, and at the bottom a small picture of the office and
  what the studio has spent today of its daily limit.
- HQ (`/`) is your round-up: good morning (or afternoon or evening), the date
  and how many things need you. Each lead has a line with its standup, one or
  two short sentences on its project's day, and under it the cards that wait
  for you there: questions with Allow and Deny, work ready to ship with Ship
  and Look closer, and a release that wants your go-ahead with Publish. Leads
  with something for you come first, and a last line gives the day's totals:
  tasks done, sent back and spent.
- A standup is written by the lead in one cheap call of at most two cents, at
  most once every six hours and only when something happened. Without a lead,
  or if the call fails, HQ gives a plain line from the facts ("2 tasks done, 1
  sent back. 1 waits for you.").
- The office (`/office`) is the same team as a company floor: a team area for
  each project around your office, the lounge, and the review room and QA lab
  every project shares. Each project has a sign with its phase, its milestone
  and progress, who is on it and a bell when it needs you. Pick a sign or a
  person and a card for that project's lead floats up, with a button to talk
  to the lead and one for what is ready. The arrows (or Q and E) turn the
  floor.
- The studio settings (`/studio`, the gear in the sidebar) hold the money (the
  daily limit and what one task may spend), the desks (how many agents work at
  once, up to nine), the hires with Hire someone, a link to the rules page,
  and Close the office, which stops anything new from starting while the work
  already running carries on.
- The Hires and Rules pages edit your hires and rules, and each task has its
  own page with its detail, a live activity feed, its status, branch, spend
  and changed files. Every page keeps itself up to date from one read of the
  studio a second, shared by every open page.
- The addresses of the earlier pages still work: `/asks`, `/board` and
  `/goals` open HQ, `/budget` and the old Money and Company tabs open the
  studio settings, and the old links to a project and to a task
  (`/?sel=projects/<name>`, `/?sel=tickets/<id>`) open its lead's page and the
  task's page.

### CLI and the rest

- `studio up` and `down`, `init`, `project add`, `ticket`, `goal`, `lead`,
  `talk`, `release` and `releases`, `asks`, `approve`, `deny`, `land`, `merge`,
  `budget`, `close` and `open`, `doctor`, `export`, `gc` and more. `--json`
  prints the raw reply. `studio close` pauses the studio the way Close the
  office does in the studio settings; `max_runs` in `studio.toml` is from 1 to
  9.
- Packs bundle hires, rules, skills and a project template to reuse across
  projects.
- A project with `watch_ci` gets a fix task when CI fails on its integration
  branch. Desktop notifications when a task asks, is ready, is held or fails,
  and when a release waits for your go-ahead.
- Claude Code is the only runtime. It runs with your own `claude` login; the
  studio never reads your credentials.
