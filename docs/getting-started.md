# Getting started

## Requirements

- macOS or Linux. Windows is not supported: the daemon talks to its clients
  over a Unix socket. The Linux release builds need glibc 2.34 or newer
  (Ubuntu 22.04, Debian 12, RHEL 9 or later).
- `git`.
- The [Claude Code](https://claude.com/claude-code) CLI (`claude`), installed
  and logged in. The studio uses your login and never reads your credentials.
- Optional: `gh`, if you want the studio to watch CI.

## Install

With the install script. It downloads a release, checks its SHA-256 sum, and
puts `studio`, `studiod` and `studio-web` in `~/.local/bin`. It never uses
`sudo`.

```sh
curl -fsSL https://raw.githubusercontent.com/marmolabs/studio/master/install.sh | sh
```

Set `STUDIO_INSTALL_DIR` to install somewhere else, or `STUDIO_VERSION` to pick
a release.

Or build from source. This needs Rust 1.98 or newer (`rustup update`).

```sh
cargo install --locked --git https://github.com/marmolabs/studio studio studiod studio-web
```

Check the setup at any time with `studio doctor`.

## Set up the studio

```sh
studio init
```

This writes `studio.toml`, five hires (engineer, producer, reviewer, approver,
router) and two rules to the studio folder: agents never `git push`, and they
may read the source of Rust dependencies without asking. It keeps any file that
already exists. The folder is `~/Library/Application Support/studio` on macOS
and `~/.config/studio` on Linux. Use `--home <dir>` or `STUDIO_HOME` to keep a
separate studio; its data folder is `<dir>/data`.

Open `studio.toml` and set the budget. The starter allows $10 a day and $2 per
task. See [configuration](configuration.md).

## Add a project

A project is a git repository. In the web UI, choose **New project** in the
sidebar ("Hire a lead for it"), give the folder, and pick the lead: a new hire
with a name the studio suggests, on a strong model, or a lead you already have.
Submitting adds the project and opens its conversation. The lead greets you,
reads the repository and shows **How my-app runs**: the checks it proposes,
each one tried by the studio in a fresh checkout with its result ("112 pass",
or "failed"), the command that takes screenshots if there is one, the release
steps and where it found them, the paths it would ask you about first, and a
daily budget. Then:

- **Use these** writes the checks, release steps and protected paths to the
  project's `.studio.toml` (keeping what else is in it) and the daily budget to
  `studio.toml`.
- **Change something** opens the composer, so you can tell the lead what to
  change.
- **See the file** shows the `.studio.toml` it would write.

The lead then offers a first milestone: **Plan it**, or **Not yet**. If a lead
can't finish the setup, the card says why and has **Try again**.

The same in a terminal:

```sh
studio project add ~/code/my-app
```

This adds the project to `studio.toml` and, if the repo has none, writes a
`.studio.toml` with the commands that check its work. For a Cargo project it
also gives every task's worktree one build folder, so tasks after the first
don't build from scratch. Read that file and fix the `verify` commands if they
are wrong.

To start from a pack instead, add the pack first, then name it:

```sh
studio pack add ~/packs/my-pack
studio project add ~/code/my-app --pack my-pack
```

## Start the studio

```sh
studio up
```

This starts `studiod`, the daemon that runs the agents, in the background,
then the web UI, and opens it on HQ. Ctrl-C stops the web UI; the
daemon keeps working. `studio up` again reopens the UI, and `studio down`
stops the daemon. Its log is `studiod.log` in the data folder. `studio status`
says what the studio is doing.

The daemon reads `studio.toml` and the hires again within a second of a
change, so adding a project or changing a budget needs no restart. Runtimes
and notifications are read once, at start.

## Talk to a lead

A project with a lead is a conversation. In the web UI, pick the lead in the
sidebar, or open `/leads/my-app`. In a terminal:

```sh
studio talk my-app "How's it going?"     # sends it, waits for the reply
studio talk my-app                       # shows the last messages
```

The lead reads the project (its code, its tasks, what waits for you, what was
done today and what was spent) and answers in at most three short sentences. It
can also act, and says what it did, each as a card in the conversation:

- start a goal (and later show its plan as a card of tasks), with a version if
  it is a milestone;
- send a task back with a note on what to change;
- pause, resume or cancel a task;
- tell a running agent something.

A lead only reads. It never edits a file or runs a command itself, and it only
acts on its own project's tasks. A project with no lead has nobody to talk to:
its conversation has a card in place of the message box, and `studio talk` says
to give it one (see [Give the project a lead](#give-the-project-a-lead)).

A message costs about $0.10 to $0.25 on a strong model. Each reply is capped at
$0.50 and counts against the project's daily limit, so a project that has spent
its day's budget stops answering until the limit resets. Only one reply is
written at a time per project: a message sent meanwhile is answered next.

The conversation page has quick questions to start from ("How's it going?",
"What's next?", "Plan something new"), and a rail on the right, **In the shop
now**: each person at work on the project, what they are doing and for how
long, the milestone with its tasks ticked off, and what the project spent today
against its cap.

## Message a running agent

To change what an agent is doing without waiting for it to finish, tell it. In
the rail of a lead's page, each running agent has a message button; type the
message and send it. Or ask the lead: "Tell Dev to use the existing helper".

What happens depends on where the task is:

- **Running**: the agent hears it in its live session, in the same turn if it
  can, or as a turn of its own right after.
- **In checks, review or ready**: the task goes back to its agent with your
  message as the note.
- **Queued or held**: it keeps the message; the agent hears it first when it
  next starts.
- **Finished**: there is nobody to tell; this is an error.

The task's run log gets a note, "Message from the owner: ...".

## Your first task

From a terminal, or by asking the lead:

```sh
studio ticket -p my-app --hire engineer "Add a dark mode" \
  --detail "A switch in settings turns it on; it follows the system until then."
studio board
```

`--hire` is optional. Without it, the project's lead picks who gets the task
(see [Give the project a lead](#give-the-project-a-lead)), or the router hire
if the project has no lead.

The title is what the task is called. The detail is the full ask: context,
steps, what done looks like. For a long one, write it in a file and pass
`--detail-file brief.md`. `studio board` lists each task and its state.

When the agent wants to do something no rule allows, it asks. The question
shows up on HQ, under its lead, and in the project's conversation, with Allow
and Deny. Or in a terminal:

```sh
studio asks
studio approve <ask>
studio deny <ask> -m "use the existing helper"
```

`deny` with a message refuses only that call and the agent carries on, told
your note. `deny` alone stops the task.

Follow what the agent did with `studio log <task>`. When the task is ready,
**Look closer** shows the change in the web UI (see [Look closer and
ship](#look-closer-and-ship)), or in a terminal read it with `studio diff
<task>`, then:

```sh
studio land <task>        # ship it to the goal branch
studio changes <task> "the pause key should be Escape"   # or send it back
```

`land` merges into `studio/goal/<goal>`, so a goal's tasks collect on one
branch. When you're happy with it, bring it into the branch the work started
from:

```sh
studio merge <task>
```

That merges in your own checkout when the branch is checked out there, and
only if the checkout has no uncommitted changes. A conflict is backed out, and
the studio files a task on the goal that merges your branch into it and
resolves the conflicts; merge again once that task lands. Nothing is pushed.
A finished goal waits on HQ, and in its lead's conversation, with a button to
ship it; once its branch is in the branch it started from, it goes away.

## Look closer and ship

Work that is ready to ship, a task or a whole goal, has a **Look closer**
button on HQ and in the conversation. It slides a sheet over the conversation
(`/leads/my-app?look=<id>`) with:

- the lead's one-sentence summary of the work;
- three proofs: the **checks** (passed or not, and how many), the **review**
  (agreed or not) and **protected paths** (whether any were touched, and that you
  allowed it);
- the tasks, with the lines each added and removed;
- the files, the first one opened with its diff and the rest folded. For a goal
  they are what its branch adds to the branch it ships to, read from git, so
  they are still there when its tasks are done and their worktrees are gone.
  The lines on HQ's card are the same count;
- the buttons: **Ship to main**, **Send back with a note** (opens a field for
  the note), and **Try it first**, which shows the command to switch to the
  work's branch with a copy button, and **Open in editor**, which opens the
  worktree.

### The checks a goal gets before it ships

When every task of a goal is done, the studio runs the project's `verify`
commands once more on the goal's branch, in a fresh checkout, the way a
release's final checks run. It keeps the result until the branch moves. Look
closer shows it as the checks proof, and HQ says "checks passed" or what
failed. A goal whose checks failed can still be shipped, but the page says so
first.

## Give the project a lead

A lead is a hire who runs a project for you: you talk to it, and it plans the
project's goals, hands its tasks to the team, checks what comes back and
answers the team's routine questions, and sends you what it can't decide. It is
optional; without one, the producer, the router and the approver do those jobs
for every project, and there is nobody to talk to. (A lead you hire from
**New project** is set up for you.)

In the web UI, a project that has no lead says so on HQ, under its line, with a
link, **Give my-app a lead**, to its conversation. There a card takes the place
of the message box, with two ways to give it one: **Hire a lead**, a new hire
on a strong model with the name the studio suggests (change it if you like) and
the tool to plan goals, or **Pick one you have**, which lists the hires with the
role `lead` and is left out when there are none. The conversation turns into
one with the new lead as soon as it is named. Once a project has a lead,
**Change lead** in the conversation's header picks another lead, hires a new
one, or leaves the project with none.

A lead you have just given a project offers, beside the message box, to look at
how the project runs: **Ask Mira to look at how my-app runs**. It sets the
project up as **New project** does, reading the repository and showing the
checks it proposes, each tried in a fresh checkout. Nothing in the project's
files changes until you click **Use these**.

A lead is a hire like any other, so you can write its file yourself in the
studio's `hires/` folder. Give it a strong model, and `tools: [file_ticket]` so
it can file the tasks of a plan:

```markdown
---
name: Manager
role: lead
runtime: claude-code
model: opus
tools: [file_ticket]
---
You lead this project. Plan its goals into small tasks with acceptance
criteria, give each to the teammate who suits it, and answer routine
questions. Pass up to the owner anything dangerous or irreversible.
```

Then name it, in a terminal or in the project's conversation in the web UI (the
card for a project without a lead, or **Change lead**):

```sh
studio lead my-app manager
```

`studio lead my-app` with no hire clears it. Run `studio doctor` to check the
lead is a hire. See [Project leads](concepts.md#project-leads) for what changes.

## Your first goal

Ask the lead ("Plan a dark mode and a settings page"), or from a terminal. A
goal goes to the project's lead, or to a producer hire if it has none. The hire
reads the code and files the tasks that reach it.

```sh
studio goal -p my-app --milestone 0.4 "Add a dark mode and a settings page"
```

The text names the goal and its branch, so keep it short. Give the planner the
full brief with `--detail` or `--detail-file brief.md`. `--milestone 0.4` makes
the goal a milestone towards version 0.4. The lead's page shows the milestone
and how many of its tasks are done. Use `--hire` to name the planner yourself.

The tasks show up in the lead's conversation as a card, all landing on one goal
branch. The number of tasks one planner may file is `goals.max_filed` (default
8).

## Your first release

Tell the studio what publishes your project, in the project's `.studio.toml`:

```toml
verify = [["cargo", "test", "--locked"]]
release = [["./scripts/publish.sh"]]
```

Then release a version:

```sh
studio release my-app 0.4
```

The project's `verify` commands run first, in a fresh checkout of the tip of
its integration branch. When they pass, the release waits for your go-ahead
(its lead's phase reads "v0.4 waits for you", and a card appears on HQ and in
the conversation):

```sh
studio release approve my-app
```

In the web UI that is the Publish button on the card (Not yet cancels it). Each
release command then runs in that checkout with
`VERSION=0.4` in its environment, and the commit is tagged `v0.4`. The studio
never pushes: pushing the tag is yours to do. `studio release cancel my-app`
stops a release before you approve it, and `studio releases` lists them all.
[Configuration](configuration.md#releases) has the details.

## The web UI

`studio up` opens it for you. To run the web UI alone, `studio-web` prints a
link like `http://127.0.0.1:4870/?token=...`. The token becomes a cookie, and
the page redirects without it. The link stays the same across restarts;
`studio up --new-token` makes a new one and the old link stops working.

`studio up --host <your LAN address>` lets a phone or another computer on
your network open the UI. Anyone on the network with the link can act as you,
and the link travels as plain HTTP, so use it only on a network you trust.

Every page has the team sidebar: the studio's name, **HQ** with a count of what
waits for you, **Your leads** (each lead's face, name, project, its phase in a
word such as Building, Testing, "v1.2 waits for you", Live v1.0 or Setting up,
a thin bar for the milestone and a yellow count when something waits for you),
**New project**, and at the bottom a small picture of the office and what the
studio spent today of its daily limit. On a phone the sidebar stacks above the
page and the rail is hidden.

- **HQ** (`/`) says good morning (or afternoon or evening) with the date and
  how many things need you. Under it each lead has a line with its **standup**:
  one or two short sentences on the project's day, written by the lead at most
  once every six hours and only when something happened (without a lead, or if
  the call fails, a plain line such as "2 tasks done, 1 sent back. 1 waits for
  you."). Below a lead's line are the cards that wait for you in that project:
  questions, work ready to ship and a release that wants your go-ahead. Leads
  with something for you come first. A last line gives the day's totals: tasks
  done, sent back, spent.
- **A conversation** (`/leads/<project>`) is the thread with one lead, newest
  at the bottom. See [Talk to a lead](#talk-to-a-lead).
- **Look closer** (`?look=<id>` on a conversation) is the sheet over it. See
  [Look closer and ship](#look-closer-and-ship).
- **The office** (`/office`) is the company floor: a team area for each
  project, around the owner's office, the lounge, and the review room and QA lab
  every project shares. Click a sign or a person and a card for that project's
  lead floats up, with **Talk to** the lead and **What's ready**. The arrows (or
  Q and E) turn the floor.
- **New project** (`/new`) hires a lead for a project. See
  [Add a project](#add-a-project).
- **Studio** (`/studio`, the gear in the sidebar) holds the money limits (the
  daily limit and what one task may spend), the desks (how many agents work at
  once), the hires with **Hire someone**, a link to the rules page, and open or
  close the office. **Closing the office** stops anything new from starting
  while work already running carries on; `studio close` and `studio open` do the
  same in a terminal.
- **The hires and rules pages** (`/hires`, `/rules`) edit your hires and
  rules.
- **A task's page** (`/tickets/<id>`) shows its detail, any ask waiting on it,
  one live activity feed, and a rail with its status, hire, branch, spend
  against its cap, progress through checks, review and gates, and changed
  files.

Addresses from older versions still work: `/asks`, `/board` and `/goals` go to
HQ, `/budget` to `/studio`, and a link to a project to its lead's page.

See [security](security.md) for how access works.

## Next

- [Concepts](concepts.md)
- [Configuration](configuration.md)
- [Packs](packs.md)
