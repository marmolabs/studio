# Concepts

## The studio

The studio is a folder of config (`studio.toml`, `hires/`, `rules/`) plus a
data folder (the event store, run logs, the daemon's socket). The daemon,
`studiod`, runs tickets. The CLI (`studio`) and the web UI (`studio-web`) are
clients that talk to it.

## Projects

A project is a git repository the studio works on. Each project has a name, a
path and an integration branch that work starts from. A project can carry its
own `.studio.toml` (checks, gates, capture) and its own rules in
`.studio/rules/`.

## Hires

A hire is an agent role, written as a Markdown file in `hires/`: a name, a
role, a runtime, a model and a prompt. The file name is the hire's id. Roles
that matter to the studio:

- `reviewer`: reads the diff of finished work. It runs on a different model
  from the one that wrote the work: its own model, or else the first of its
  `escalate` models, so an author that escalated onto the reviewer's model is
  still reviewed.
- `producer`: plans goals. Needs `tools: [file_ticket]`.
Everything else is yours to name. A gate names its critic by hire, whatever
that hire's role is.

## Tickets

A ticket is one piece of work for one hire in one project. It works in its own
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

A ticket gets up to three attempts. When checks or review fail, the failure
goes back to the agent and it tries again. A hire can list stronger models in
`escalate` to use after each failure. "Attempts" on the ticket page counts every
run the ticket has had, including ones sent back or retried.

A ticket is a `work` ticket, or a `plan` ticket. A plan ticket files other
tickets instead of changing code, so it skips checks and review.

## Goals

A goal is a line of text given to a producer. The producer reads the code and
files tickets with `file_ticket`. Tickets with the same goal land on one
branch, `studio/goal/<goal>`, so you review and merge a goal as a unit.

The tickets of a goal run one after another: the next starts only when the
previous has landed, failed or been cancelled. Once the goal branch exists, a
new ticket's worktree starts from it, so each ticket sees the landed work of
the ones before. A ticket with no goal is its own goal. Different goals run
side by side.

With `auto_land = true` in a project's `.studio.toml`, a ticket that passes
review and gates lands on its goal branch by itself. Merging a goal into your
branch is always `studio merge`.

If the goal branch has moved on and conflicts with a ticket, landing merges
that work into the ticket's worktree instead and sends the ticket back to its
agent, which resolves the conflicts before its checks and review run again.
Nothing is committed while conflict markers remain. If merging a goal into
your branch conflicts, the studio files a ticket on the goal that merges your
branch into it and resolves the conflicts; merge again once it lands.

## Asks

An ask is a request that waits for you. Agents raise one when an action needs
approval, or when they use `ask_owner` to put a question to you with options.
Approve or deny from `studio asks`, or press Allow or Deny in the web UI. A
permission you deny with a note refuses just that call: the agent is told "The owner declined this"
and your note, and carries on. A deny without a note stops the ticket. A denied
question or gate always stops it.

## The approver and the router

Two cheap hires work for the studio itself.

The approver decides tool calls that no rule covers, so only dangerous ones
reach you. It allows what is a normal part of the ticket inside its worktree,
denies what the ticket doesn't need, and sends to you what is dangerous,
irreversible or reaches outside the project. A denial does not stop the ticket:
the agent is told why and tries another way. Asks from your own rules (`protect`
rules and ask-level gates) and questions from agents always reach you, never the
approver. When the approver errors, has no budget or gives no clear answer even when asked again, the
call goes to you, with the reason shown.

The router picks the hire for a ticket created without one (`studio ticket`
without `--hire`, the board's "Pick for me", or a producer's `file_ticket`
without a hire). Without a router, the first engineer takes it.

Each call is capped at $0.05 and charged to the ticket. `studio init` writes
both hires; deleting `hires/approver.md` or `hires/router.md` turns that one off.

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
pack, or a project, and edits apply on the next run.

## Packs

A pack is a folder of shared hires, rules, skills and a project template. See
[packs](packs.md).

## Gates

A gate is a check in a project's `.studio.toml`. When a change touches files
that match the gate's globs, a critic hire reads it before you do. If the
critic fails the work, it goes back to the author. After the gate's `rounds`
are used up, the decision comes to you.

For each changed `png`, `jpg` or `gif` that matches the gate's globs (up to
12), the critic also gets the shipped version beside the new one, written as
`before-*` and `after-*` files in the attempt's evidence folder. A new file has
only the `after-` image. A deleted image is skipped, and so is a file reached
through a symlink, with a note in the run log. On the ticket, these files
appear in the "Art · shipped vs new" section, not among the screenshots.

## Evidence

If a project sets `capture`, the studio runs that command after checks pass.
It writes screenshots or clips into a per-attempt folder in the worktree.
Critics see the images, and you see them on the ticket. Evidence files are
`png`, `jpg`, `gif` or `mp4`. Names starting `before-` or `after-` are reserved
for gates, so a capture must not use them.

## Budget

Every run gets an allowance that fits under the studio, project, hire, ticket
and run limits at once. A run that can't get one is held. See
[configuration](configuration.md).
