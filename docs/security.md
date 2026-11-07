# Security

The studio runs AI agents on your machine, with your privileges. This page
says what it does to limit that, and what it does not.

## Credentials

The studio never reads, stores or sends your Claude credentials. Agents run
through the `claude` CLI, which uses your own login or API key. The studio only
starts that program and reads what it prints.

## The web UI

`studio-web` can approve what agents do, so it is locked down:

- It listens on `127.0.0.1` unless you pass `--host` (see below).
- The link it prints, `http://127.0.0.1:<port>/?token=...`, carries a random
  token (32 bytes from the system's random source). The token is kept in
  `web-token` in the data folder, readable only by you (mode `0600`), so the
  link stays the same across restarts. `studio up --new-token` (or
  `studio-web --new-token`) makes a new one, and the old link stops working.
  Do that if the link leaks.
- The token becomes a cookie that is `HttpOnly` and `SameSite=Strict`. The page
  then redirects, so the token leaves the address bar and history.
- Every request without the cookie gets a 401. Tokens are compared in constant
  time.
- A POST with an `Origin` header that is not the UI's own address is refused.

Do not forward the port or put it behind a proxy.

### Reaching it from another device

`studio up --host <address>` (or `studio-web --host <address>`) listens on
another address, for example your LAN address `192.168.1.20`, so a phone or
another computer on the network can open the UI. Give the address the other
device will use: the printed link and the origin check are built from it.

This exposes the UI to your network. Anyone who has the link can approve
asks, land tasks, approve releases and spend your budget, and the page is plain HTTP, so the
token crosses the network unencrypted where others on it could read it. Use it
only on a network you trust, never on public Wi-Fi, and run
`studio up --new-token` when you are done sharing the link.

## Agents

- Each task runs in its own git worktree, on its own branch. The agent's
  working directory is that worktree, not your main checkout.
- Permission requests go through your rules. The studio answers each one
  with your `protect`, `gate` and `lint` rules. With no matching rule, edits
  inside the worktree are allowed and everything else becomes an ask for you.
- A permission you deny with a note refuses that one call and the agent
  carries on; a deny with no note stops the task.
- Changes are checked against your rules again before they are committed.
- Agents never push by default (`studio init` writes a rule that blocks
  `git push`). Landing merges locally. You push.
- Agents get only the studio's own MCP tools, not the MCP servers in your
  Claude settings, unless a hire sets `user_mcp: true`. A server that reads
  other people's messages (chat, email, issues) is a way to steer an agent.
- Run logs and rules are plain files you can read.

## The approver and the router

Two cheap hires work for the studio itself: the approver decides the tool calls
no rule covers, so only dangerous ones reach you, and the router picks a hire
for a task. Your own `protect` rules, ask-level gates and agents' questions
always reach you, never the approver. When the approver errors, has no budget
or gives no clear answer, the call goes to you. See
[concepts](concepts.md#the-approver-and-the-router).

## Project leads

A project's lead (see [concepts](concepts.md#project-leads)) plans, hands out
and checks work. It does not decide tool calls: the approver does, for every
project, so a lead's strong model never answers routine questions. What a lead
may not decide stays with you:

- Your own `protect` rules and ask-level gates always come to you.
- Questions an agent puts to you (`ask_owner`) always come to you.
- Landing a task, merging a goal into your branch and giving a release its
  go-ahead are yours. A lead plans and assigns; it does not land, merge or
  release.

## Talking to a lead

When you message a lead, its hire runs once in the project's repository with
read-only tools: it can read and search files, and can't edit them, run
commands, fetch pages or start other agents. The only way it acts is the
studio's own tools: start a goal, send a task back, pause, resume or cancel a
task, and tell a task's agent something. The studio refuses any of these on a
task that isn't the lead's own project's, and each one leaves a card in the
conversation, so you can see what the lead did. A lead can't ship work, merge a
goal or give a release its go-ahead; those stay with you.

A reply is capped at $0.50 and counts against the project's daily limit, as a
lead's other calls do. The conversation is kept in plain files in the data
folder (`talk/`). A lead's turn reads the repository, so what it reads may try
to steer it; like any lead, choose its model and prompt with that in mind.

A message to a running agent, from you or a lead, goes into that agent's
session as coming from the owner or the lead, by name, and is written to the
task's run log.

## Setting up a project

A lead that sets a project up reads the repository and proposes its checks. The
studio runs each proposed check, and the screenshot command, in a fresh
checkout of the project's branch, so a check's commands do run on your machine
as you: read what the lead proposes before you choose **Use these**. The
release commands it proposes are not run. Nothing is written to the project's
`.studio.toml` or `studio.toml` until you choose.

## Releases

A release runs the project's own commands, `release` in its `.studio.toml`, on
your machine as you, with the project's `env` and `VERSION` set. Nothing runs
until the project's final checks pass and you give the go-ahead
(`studio release approve`, or Publish in the web UI); the checks themselves
are the project's `verify` commands, which also run as you. A release that is
waiting can be cancelled, and one that has begun publishing cannot be stopped.
The tools the studio gives agents (`read_rules`, `load_skill`, `ask_owner` and
`file_ticket`) cannot start, approve or cancel a release. As with everything
else, a process that can reach the daemon's socket can; see below.

The studio tags the released commit `v<version>` and never pushes the tag or
anything else. Review a project's `.studio.toml` before you add a project you
did not write: its release commands are code that runs when you release.

## What is not protected

- Anything that can reach the `studiod` Unix socket acts as you. The socket
  has mode `0600` (it is in the data folder, or in `/tmp` when that path is too
  long for a socket), so other accounts can't use it. But
  any process running as your user can approve asks, land tasks, approve
  releases and spend your budget. Don't share the machine account.
- Bash commands that you allow run with your privileges, outside any sandbox.
  A rule that allows `Bash` lets an agent run anything you can. Prefer
  prefix rules like `bash:cargo test`, and keep the default of asking. An
  allowed prefix approves the command it starts, with any arguments: allowing
  `bash:cargo` also allows `cargo run` and `cargo install`.
- An allowed command can read files outside the worktree, use the network and
  call other tools.
- The checks in `.studio.toml` and in `check` rules, and a project's release
  commands, run on your machine, as you. Review a project's `.studio.toml`
  before adding a project you did not write, and a pack before adding it.
- Hire prompts and pack skills are instructions to an agent. A bad one can
  steer it. Read them.
- Budget limits cap spend as the studio sees it. They are not a billing
  control. Set limits in your Claude account too.

## Reporting a problem

Open a private security advisory on the repository, or contact the
maintainers. Please don't file public issues for vulnerabilities.
