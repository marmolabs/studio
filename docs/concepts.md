# Concepts

## The studio

The studio is a folder of config (`studio.toml`, `hires/`, `rules/`) plus a
data folder (the event store, run logs, evidence, releases). The daemon,
`studiod`, runs tickets. The CLI (`studio`) and the web UI (`studio-web`) are
clients that talk to it over a Unix socket. The web UI's home page is the
company floor: every project as a team area on one floor.

## The company

The studio works like a small company:

- **You are the owner.** You set the budget and the rules, and you decide what
  matters: dangerous actions, the questions agents put to you, landing work,
  merging a goal into your branch and every release.
- **A lead** runs a project, if you name one. It plans the project's goals,
  assigns its tickets and answers its team's routine questions, and escalates
  to you what it can't decide. See [Project leads](#project-leads).
- **The makers** are the hires that do the work: engineers, designers, writers
  and the like. Each takes one ticket at a time, in its own worktree.
- **The review room and the QA lab are shared.** Reviewers and critics read the
  work of every project, and every project's checks run in the QA lab. A
  reviewer runs on a different model from the work's author.
- **The front desk** is the approver and the router. A project with no lead
  has them answer its routine questions and assign its tickets.

On the web UI's floor, each project has a team area with a sign, and the
lounge holds the hires with nothing to do.

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

or run `studio lead my-app manager` (`studio lead my-app` clears it), or pick
one on the project's page in the web UI. The lead takes over three jobs in
that project, and nothing else changes:

- Planning: a goal given without a hire goes to the lead instead of the
  producer. Give the goal a version (`studio goal --milestone 0.4`) and it is
  a [milestone](#milestones).
- Assigning: a ticket filed without a hire is assigned by the lead, who picks
  from its team, instead of by the router. Its team is every hire that takes
  tickets: not the approver, router, reviewers, leads or gate critics, and not
  the lead itself.
- Routine questions: a tool call no rule covers is decided by the lead instead
  of the approver, under the same policy. The lead sends you exactly what the
  approver would: what is dangerous, irreversible or reaches outside the
  project.

A lead never answers for its own tickets; those go to the approver. Owner
rules and questions from agents reach you as before. A project with no lead, or
whose lead isn't a hire, keeps the producer, router and approver. Each call the
lead makes to assign or to answer is capped at five cents and charged to the
ticket, like theirs. The For you tab says how many routine questions your
leads answered today.

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

On a project's page, a milestone's tasks sit in four columns: To do (queued),
Doing (claimed, working, asking, held or failed), Checking (checking, reviewing
or ready) and Done (landed).

## Goals

A goal is a line of text given to a planner: the project's lead, or else a
producer. The planner reads the code and files tickets with `file_ticket`.
Tickets with the same goal land on one branch, `studio/goal/<goal>`, so you
review and merge a goal as a unit.

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

## Milestones

A goal given a version is a milestone: `studio goal --milestone 0.4`, or the
Version field of the New work form. The version is a label for you and the
planner, who is told the goal is a milestone towards it; it changes nothing
else about the goal. A project's sign and page show the milestone it works
towards, with how many of its tasks have landed. Where a project has several
open goals, the first with a version is its milestone, else the oldest. A
release is a separate step, with its own version (see below).

## Phases

Each project is in one phase, worked out from its tickets and releases, and
shown on its sign and page. The first rule that fits wins:

| Phase | When |
| --- | --- |
| Releasing | A release is checking, waiting for your go-ahead or publishing, even while the studio is closed. |
| Paused | The studio is closed and the project has work open: it would be building, planning or testing. |
| Building | Any work ticket (not a plan) is claimed, working, asking or held. |
| Planning | A goal's plan ticket is still open, and nothing is building. |
| Testing | Anything is in checks, in review or ready, and nothing is building. |
| Live | Nothing is open (every ticket landed, failed or was cancelled) and the project has been released. |
| Idle | Anything else: nothing open and never released, or work that is queued and waiting its turn. |

The project's page lays the phases out as a pipeline, Planning to Live. A
release that failed in the last day marks the sign "Release failed".

## Releases

A project with `release` commands in its `.studio.toml` can be released from
the studio: `studio release <project> <version>`, or Start a release on the
project's page. A release goes in steps, and you see them as a checklist:

1. **Final checks.** The project's `verify` commands run on the tip of its
   integration branch, in a fresh checkout.
2. **Your go-ahead.** The release waits for you. `studio release approve
   <project>`, or Approve in the web UI, gives it; Cancel stops it. The
   studio tells you when it is waiting, and the sign shows a bell.
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
and your note, and carries on. A deny without a note stops the ticket. A denied
question or gate always stops it.

## The approver and the router

Two cheap hires work for the studio itself. In a project with a [lead](#project-leads),
the lead does both jobs for that project.

The approver decides tool calls that no rule covers, so only dangerous ones
reach you. It allows what is a normal part of the ticket inside its worktree,
denies what the ticket doesn't need, and sends to you what is dangerous,
irreversible or reaches outside the project. A denial does not stop the ticket:
the agent is told why and tries another way. Asks from your own rules (`protect`
rules and ask-level gates) and questions from agents always reach you, never the
approver. When the approver errors, has no budget or gives no clear answer even when asked again, the
call goes to you, with the reason shown.

The router picks the hire for a ticket created without one (`studio ticket`
without `--hire`, the New task form's "Pick for me", or a planner's `file_ticket`
without a hire). Without a router, the first engineer takes it.

Each call is capped at five cents and charged to the ticket. `studio init`
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
symlink, with a note in the run log. On the ticket, these files appear in the
"Images · shipped vs new" section, not among the screenshots.

## Evidence

If a project sets `capture`, the studio runs that command after checks pass.
It writes screenshots or clips into a per-attempt folder in the worktree.
Critics see the images, and you see them on the ticket. When the ticket
lands and its worktree goes, its evidence is kept in the data folder
(`evidence/<ticket>/`), so the ticket still shows it. Evidence files are
`png`, `jpg`, `gif` or `mp4`. Names starting `before-` or `after-` are reserved
for gates, so a capture must not use them.

## Budget

Every run gets an allowance that fits under the studio, project, hire, ticket
and run limits at once. A run that can't get one is held. See
[configuration](configuration.md).
