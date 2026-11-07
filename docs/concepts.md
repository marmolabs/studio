# Concepts

## The studio

The studio is a folder of config (`studio.toml`, `hires/`, `rules/`) plus a
data folder (the event store, run logs, evidence, releases). The daemon,
`studiod`, runs tasks. The CLI (`studio`) and the web UI (`studio-web`) are
clients that talk to it over a Unix socket. The web UI's home page is HQ, the
owner's round-up, with a team sidebar that has a lead for each project.

## The company

The studio works like a small company:

- **You are the owner.** You set the budget and the rules, and you decide what
  matters: dangerous actions, the questions agents put to you, shipping work,
  merging a goal into your branch and every release. You talk to one lead for
  each project.
- **A lead** runs a project, if you name one, and is who you talk to. It plans
  the project's goals, assigns its tasks, checks what comes back and answers
  its team's routine questions, and comes to you for what it can't decide. See
  [Project leads](#project-leads).
- **The makers** are the hires that do the work: engineers, designers, writers
  and the like. Each takes one task at a time, in its own worktree.
- **The review room and the QA lab are shared.** Reviewers and critics read the
  work of every project, and every project's checks run in the QA lab. A
  reviewer runs on a different model from the work's author.
- **The front desk** is the approver and the router. A project with no lead
  has them answer its routine questions and assign its tasks. There is
  nobody to talk to there.

On the web UI's office view, each project has a team area with a sign, and the
lounge holds the hires with nothing to do. Picking a person or their task opens
that hire's own card and direct conversation, including someone in the lounge.

## Projects

A project is a git repository the studio works on. Each project has a name, a
path, an integration branch that work starts from and, optionally, a lead. A
project can carry its own `.studio.toml` (checks, gates, capture, release
commands) and its own rules in `.studio/rules/`.

## Hires

A hire is an agent role, written as a Markdown file in `hires/`: a name, a
role, a runtime, a model and a prompt. The file name is the hire's id. Roles
that matter to the studio:

- `reviewer`: reads the diff of finished work. It runs on a different model
  from the one that wrote the work: its own model, or else the first of its
  `escalate` models, so an author that escalated onto the reviewer's model is
  still reviewed.
- `producer`: plans goals. Needs `tools: [file_ticket]`.
- `lead`: runs a project, once named as its lead in `studio.toml`. Needs
  `tools: [file_ticket]` to plan. See [Project leads](#project-leads).
- `approver` and `router`: see below.

Everything else is yours to name. A gate names its critic by hire, whatever
that hire's role is.

## Project leads

A project can have a lead: one hire, usually on a strong model, who runs it
like a manager and answers to you. Name it in `studio.toml`:

```toml
[[projects]]
name = "my-app"
path = "/Users/me/code/my-app"
lead = "manager"
```

or run `studio lead my-app manager` (`studio lead my-app` clears it). In the web
UI, a project's conversation does it too: the card of a project without a lead,
which can hire one, or **Change lead** in its header. A lead you hire from
**New project**, or from that card, is named for you. The lead takes over these
jobs in that project, and nothing else changes:

- Talking to you. The conversation with a lead is kept in the data folder
  (`talk/<project>.jsonl`), one message a line. See
  [Talking to a lead](#talking-to-a-lead).
- Planning: a goal given without a hire goes to the lead instead of the
  producer. Give the goal a version (`studio goal --milestone 0.4`) and it is
  a [milestone](#milestones).
- Assigning: a task filed without a hire is assigned by the lead, who picks
  from its team, instead of by the router. Its team is every hire that takes
  tasks: not the approver, router, reviewers, leads or gate critics, and not
  the lead itself.
- Routine questions: a tool call no rule covers is decided by the lead instead
  of the approver, under the same policy. The lead sends you exactly what the
  approver would: what is dangerous, irreversible or reaches outside the
  project. When it does, it also writes the question in a few plain words
  ("Send the launch email?"), which is what HQ shows.
- The standup: one or two sentences on the project's day, for HQ.

A lead never answers for its own tasks; those go to the approver. Owner
rules and questions from agents reach you as before. A project with no lead, or
whose lead isn't a hire, keeps the producer, router and approver. Each call the
lead makes to assign or to answer is capped at five cents and charged to the
task, like theirs. HQ says how many routine questions your leads answered
today.

## Talking to a lead

A message to a lead starts a turn of its own: the lead's hire runs once in the
project's repository with read-only tools (it reads files and searches them,
and never edits, runs commands or browses) and the studio's own tools for
acting on the project:

| Tool | What it does |
| --- | --- |
| `start_goal` | Starts a goal the lead plans itself, with a version if it is a milestone. |
| `send_back` | Sends a task back to its agent with a note. |
| `pause`, `resume`, `cancel` | Stops a task's agent and holds the task, continues a held task, or cancels one for good. |
| `tell` | Tells a task's agent something. |

The tools only reach the lead's own project: any other task is refused. The
lead's prompt is its hire's prompt, a short brief (answer in plain words, in at
most three short sentences, use the tools when the owner asks and say what you
did), the project's state (open goals with their tasks, what waits for you,
work ready to ship, releases, today's results and spend) and the last 20
messages. Its last words are its reply, and each tool it used leaves a card.

One turn runs at a time for each project; a message sent meanwhile is answered
by the next turn. A turn is capped at $0.50, counts against the project's daily
limit, and is charged to the project as `<project>/talk`. A reply costs about
$0.10 to $0.25 on a strong model.

The standup and a setup (below) run the same way, on the lead's hire: a
standup is one cheap call of at most $0.02, and a setup is capped at $0.60.

## Setting a project up through its lead

**New project** adds a project and has its lead set it up. The lead reads the
repository and proposes how the project runs: the checks (`verify`), a capture
command for screenshots if the project has one, the release commands and where
they were found, the paths to ask you about before an agent touches them, a
daily budget and a first goal. The studio does not take the lead's word for it:
it runs each check and the capture in a fresh checkout of the project's branch,
as a release's final checks run, and records whether each passed, how long it
took and what it came to ("112 pass"). Release commands are not run.

A setup goes from reading to checking to ready (or failed), and ends accepted;
its state is kept in the data folder (`setup/<project>.json`). When it is
ready, the lead's message has the card of how the project runs. **Use these**
writes the chosen checks, capture, release and protected paths to the
project's `.studio.toml` (keeping what else is in it) and the daily budget to
`studio.toml`. Nothing is written before that.

## Talking directly to an employee

Every hire has a persistent, read-only conversation at `/people/<hire>`, reached
by picking the person or one of their tasks in the office. The hire gets its
current and recent assignments, their briefs and results, the last 20 messages,
and read access to the project of its current work. It cannot edit files, run
commands, browse, or use the studio's action tools. To steer an active run, use
the separate message control beside its task.

Messages are kept in `<data>/people/<hire>.jsonl`. One reply runs at a time for
each hire; a message sent meanwhile is answered by the next turn. A reply is
capped at $0.25 and charged to `<project>/people/<hire>`, under the studio,
project and hire budget limits.

## Manager operations and approvals

A project lead can review its project without waiting for a conversation. Choose
**Run manager review** on HQ or the company page. The lead reads the project and
its assignments, writes a short assessment and proposes explicit actions:
starting a goal, delegating a task, requesting changes, pausing, resuming,
cancelling, or messaging an employee. Reviews and every action's outcome are
kept in `<data>/operations.json`; the company page shows the decision log, and
the project's room carries its reviews and completed actions.

A review runs with read-only tools, is capped at $0.75 and is charged to
`<project>/operations`. It cannot ship, publish, change budgets or edit files.
Pending actions appear on HQ with **Approve** and **Reject**. Approval executes
through the same project-scoped task and lead-action paths as a conversation.

**Studio → Manager autonomy** controls the review interval and what may execute
without individual approval:

- **Supervised** (the default): every action needs approval.
- **Balanced**: feedback and employee messages may go ahead; everything else
  waits.
- **Autonomous**: ordinary task management may go ahead; cancellation still
  waits.

Automatic reviews are off by default, run only while the office is open, and use
the configured interval (5 minutes to 24 hours). A review is told which
proposals of its project still wait for you and is asked not to repeat them; a
proposal identical to one that waits is dropped; at most 20 may wait in a
project, and beyond that new ones are not added. An automatic review is skipped
for a project that still has proposals waiting. Shipping and publishing always
need the owner at every level. Actions are claimed atomically so concurrent
approvals cannot execute them twice. An interrupted claim becomes failed on
restart. If `operations.json` is unreadable, it is preserved, the UI shows the
problem, and manager reviews and changes to these settings are blocked until it
is repaired. Task runs, conversations and everything else carry on in live mode.

## Simulation and live runtimes

**Studio → Manager autonomy** also switches new runs between configured runtimes
and local simulation. Configured runtimes are the hires' normal models.
Simulation returns scripted responses without calling a model and records zero
model cost. It applies only to the runs that talk to you: conversations with
leads and hires, room replies, standups, setup runs and manager reviews (a
review proposes no actions). Coding, checks, task reviews, critic gates, the
permission approver and the router always use the hire's configured runtime, so
a simulation can never allow a permission or pass work. Existing work and
history remain visible, and agents already running when the switch changes
finish their current run. The choice is kept with the other operations settings
in `<data>/operations.json`.

## Company rooms, work map and scorecard

`/rooms/company` is a persistent company-wide room; each project also has a
room at `/rooms/<project>`. Owner messages and announcements are stored in
`<data>/rooms/<room>.jsonl`. Mention a loaded hire by id, such as `@dev`, to ask
for a reply. A mentioned hire gets the room history, its assignments and
read-only access to the relevant project. It cannot act on tasks. Each reply is
capped at $0.25 and charged to `<project>/rooms/<room>`. Messages without a
valid mention start no model run and cost nothing. A mention that arrives during
a reply gets the next turn. Project-room prompts include only that project's
assignments; the company room can include cross-project assignments. Pages show
at most 200 messages, with **Earlier messages** for older history.

The company page (`/company`) shows each goal and the tasks under it, their
owners, states, next actions and ticket spend. Its scorecard uses ticket events
and the spend ledger for the last 24 hours: deliveries, median created-to-landed
time, failed checks and reviews, and cost per delivery. Project spend is the
recorded total for the displayed tickets. Blocked time is the accumulated age
of work still blocked now, not a historical estimate. Employee workload shows
active and queued assignments, deliveries and ticket spend.

## Messaging an agent

You, or a lead for you, can tell a task's agent something while it works. A
running agent hears it in its live session. A task that waits in checks, review
or ready goes back to its agent with the message as the note, a queued or held
one keeps it for its next run, and a finished one can't be told. The run log
gets a note, "Message from <who>: ...".

## Tasks

A task (the CLI and the engine call it a ticket, and `land` and `merge` are
how you ship it) is one piece of work for one hire in one project. It works in its own
git worktree on a branch named `studio/<ticket>`.

States:

| State | Meaning |
| --- | --- |
| `queued` | Waiting for a free slot. |
| `claimed` | Picked by the scheduler. |
| `working` | The agent is running. |
| `checking` | The project's checks are running. |
| `reviewing` | A reviewer or a gate is reading the change. |
| `ready` | Passed. Waiting for you to land it. |
| `landed` | Merged into the goal branch. |
| `cancelled` | Stopped by you. |
| `asking` | The agent is waiting on an ask. |
| `held` | Paused. `budget` and `quota` holds resume by themselves, a quota hold when the provider's limit resets; `auth` and `paused` wait for you. |
| `failed` | Ended with a class: `crash`, `timeout`, `checks`, `review`, `tooling` or `denied`. |

A task gets up to three attempts. When checks or review fail, the failure
goes back to the agent and it tries again. A hire can list stronger models in
`escalate` to use after each failure. "Attempts" on the task page counts every
run the task has had, including ones sent back or retried.

A task is a `work` task, or a `plan` task. A plan task files other
tasks instead of changing code, so it skips checks and review.

In a lead's conversation, a milestone's tasks show as ticks in the rail beside
the thread, and a goal's plan as a card of its tasks.

## Priorities and dependencies

Every task has a priority (50 by default, up to 1000); larger values run first,
with older tasks winning ties. Dependencies are other tickets in the same
project that must land before a task starts. Missing, queued, active, failed or
cancelled dependencies block it; only landed ones are satisfied. Self-links and
cycles are rejected when links are written, and cyclic legacy data simply cannot
be scheduled until corrected.

The scheduler still runs only one task per goal at a time. Changing priority does
not preempt active work. Dependencies can be changed while a task is queued or
failed, not after it starts. Both changes are events in the task's history.

Managers can propose **Change priority** and **Change dependencies** through the
same approval and autonomy flow as other actions. The company page shows unmet
blockers, priority badges, the dependency graph and the longest open critical
path.

## Goals

A goal is a line of text given to a planner: the project's lead, or else a
producer. The planner reads the code and files tasks with `file_ticket`.
Tasks with the same goal land on one branch, `studio/goal/<goal>`, so you
review and merge a goal as a unit.

The tasks of a goal run one after another: the next starts only when the
previous has landed, failed or been cancelled. Once the goal branch exists, a
new task's worktree starts from it, so each task sees the landed work of
the ones before. A task with no goal is its own goal. Different goals run
side by side.

With `auto_land = true` in a project's `.studio.toml`, a task that passes
review and gates lands on its goal branch by itself. Merging a goal into your
branch is always `studio merge`.

If the goal branch has moved on and conflicts with a task, landing merges
that work into the task's worktree instead and sends the task back to its
agent, which resolves the conflicts before its checks and review run again.
Nothing is committed while conflict markers remain. If merging a goal into
your branch conflicts, the studio files a task on the goal that merges your
branch into it and resolves the conflicts; merge again once it lands.

## Milestones

A goal given a version is a milestone: `studio goal --milestone 0.4`, or
by asking its lead for it ("Plan the checkout flow for 0.5"). The version is a
label for you and the planner, who is told the goal is a milestone towards it;
it changes nothing else about the goal. The lead's page, its line in the
sidebar and its sign on the office floor show the milestone it works towards,
with how many of its tasks have landed. Where a project has several
open goals, the first with a version is its milestone, else the oldest. A
release is a separate step, with its own version (see below).

## Phases

Each project is in one phase, worked out from its tasks and releases, and
shown beside its lead in the sidebar, on the lead's page and on its sign on the
office floor, in a word or two (a release that waits for you reads "v1.2 waits
for you", a live project "Live v1.0", and a project still being set up "Setting
up"). The first rule that fits wins:

| Phase | When |
| --- | --- |
| Releasing | A release is checking, waiting for your go-ahead or publishing, even while the studio is closed. |
| Paused | The studio is closed and the project has work open: it would be building, planning or testing. |
| Building | Any work task (not a plan) is claimed, working, asking or held. |
| Planning | A goal's plan task is still open, and nothing is building. |
| Testing | Anything is in checks, in review or ready, and nothing is building. |
| Live | Nothing is open (every task landed, failed or was cancelled) and the project has been released. |
| Idle | Anything else: nothing open and never released, or work that is queued and waiting its turn. |

A release that failed in the last day marks the project "Release failed".

## Checks before shipping

When every task of a goal is done, the studio runs the project's `verify`
commands once more on the goal's branch, in a fresh checkout, as a release's
final checks run, and keeps the result until the branch moves. A goal's **Look
closer** sheet shows it as the checks proof, and HQ says "checks passed" or what
failed. A goal whose checks failed can still be shipped (`studio merge`, or Ship
to main), but the web says so first.

## Releases

A project with `release` commands in its `.studio.toml` can be released from
the studio: `studio release <project> <version>`. A release goes in steps, which its lead
reports as they pass:

1. **Final checks.** The project's `verify` commands run on the tip of its
   integration branch, in a fresh checkout.
2. **Your go-ahead.** The release waits for you. `studio release approve
   <project>`, or Publish on the card in the web UI, gives it; `studio release
   cancel <project>`, or Not yet, stops it. The studio tells you when it is
   waiting, and the card is on HQ and in the lead's conversation.
3. **The release commands** run in order, in that same checkout, with
   `VERSION` set to the version.
4. **The tag.** The commit is tagged `v<version>`. The studio never pushes it.

A release whose checks or commands fail stops there as failed, and nothing is
tagged. Releases are kept in the data folder, and `studio releases` lists
them. See [configuration](configuration.md#releases).

## Asks

An ask is a request that waits for you. Agents raise one when an action needs
approval, or when they use `ask_owner` to put a question to you with options.
Approve or deny from `studio asks`, or press Allow or Deny in the web UI. A
permission you deny with a note refuses just that call: the agent is told "The owner declined this"
and your note, and carries on. A deny without a note stops the task. A denied
question or gate always stops it.

## The approver and the router

Two cheap hires work for the studio itself. In a project with a [lead](#project-leads),
the lead does both jobs for that project.

The approver decides tool calls that no rule covers, so only dangerous ones
reach you. It allows what is a normal part of the task inside its worktree,
denies what the task doesn't need, and sends to you what is dangerous,
irreversible or reaches outside the project. A denial does not stop the task:
the agent is told why and tries another way. Asks from your own rules (`protect`
rules and ask-level gates) and questions from agents always reach you, never the
approver. When the approver errors, has no budget or gives no clear answer even when asked again, the
call goes to you, with the reason shown.

The router picks the hire for a task created without one (`studio ticket`
without `--hire`, the New task form's "Pick for me", or a planner's `file_ticket`
without a hire). Without a router, the first engineer takes it.

Each call is capped at five cents and charged to the task. `studio init`
writes both hires; deleting `hires/approver.md` or `hires/router.md` turns that
one off.

## Rules

A rule is a Markdown file that tells the studio what agents may do. There are
six kinds:

- `protect`: paths agents can't change without a decision.
- `gate`: a tool, or a command prefix like `bash:git push`.
- `lint`: a pattern that must not appear in what an agent writes or commits.
- `check`: a command that must pass.
- `guide`: text added to the agent's instructions.
- `review`: text added as what the work is reviewed against.

Each has a level: `block`, `ask`, `warn` or `allow`. When rules disagree,
block beats ask beats allow. With no matching rule, edits inside the worktree
are allowed and everything else becomes an ask. Rules come from the studio, a
pack, or a project, and edits apply on the next run. See [rules](rules.md).

## Packs

A pack is a folder of shared hires, rules, skills and a project template. See
[packs](packs.md).

## Gates

A gate is a check in a project's `.studio.toml`. When a change touches files
that match the gate's globs, a critic hire reads it before you do. If the
critic fails the work, it goes back to the author. After the gate's `rounds`
are used up, the decision comes to you.

For each changed `png`, `jpg` or `gif` that matches the gate's globs, the
critic also gets the shipped version beside the new one, written as `before-*`
and `after-*` files in the attempt's evidence folder. A new file has only the
`after-` image. A deleted image is skipped, and so is a file reached through a
symlink, with a note in the run log. On the task, these files appear in the
"Images · shipped vs new" section, not among the screenshots.

## Evidence

If a project sets `capture`, the studio runs that command after checks pass.
It writes screenshots or clips into a per-attempt folder in the worktree.
Critics see the images, and you see them on the task. When the task
lands and its worktree goes, its evidence is kept in the data folder
(`evidence/<ticket>/`), so the task still shows it. Evidence files are
`png`, `jpg`, `gif` or `mp4`. Names starting `before-` or `after-` are reserved
for gates, so a capture must not use them.

## Budget

Every run gets an allowance that fits under the studio, project, hire, task
and run limits at once. A run that can't get one is held. See
[configuration](configuration.md).
