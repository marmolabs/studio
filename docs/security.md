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
asks, land tickets and spend your budget, and the page is plain HTTP, so the
token crosses the network unencrypted where others on it could read it. Use it
only on a network you trust, never on public Wi-Fi, and run
`studio up --new-token` when you are done sharing the link.

## Agents

- Each ticket runs in its own git worktree, on its own branch. The agent's
  working directory is that worktree, not your main checkout.
- Permission requests go through your rules. The studio answers each one
  with your `protect`, `gate` and `lint` rules. With no matching rule, edits
  inside the worktree are allowed and everything else becomes an ask for you.
- A permission you deny with a note refuses that one call and the agent
  carries on; a deny with no note stops the ticket.
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
for a ticket. Your own `protect` rules, ask-level gates and agents' questions
always reach you, never the approver. When the approver errors, has no budget
or gives no clear answer, the call goes to you. See
[concepts](concepts.md#the-approver-and-the-router).

## What is not protected

- Anything that can reach the `studiod` Unix socket acts as you. The socket
  has mode `0600` (it is in the data folder, or in `/tmp` when that path is too
  long for a socket), so other accounts can't use it. But
  any process running as your user can approve asks, land tickets and spend
  your budget. Don't share the machine account.
- Bash commands that you allow run with your privileges, outside any sandbox.
  A rule that allows `Bash` lets an agent run anything you can. Prefer
  prefix rules like `bash:cargo test`, and keep the default of asking. An
  allowed prefix approves the command it starts, with any arguments: allowing
  `bash:cargo` also allows `cargo run` and `cargo install`.
- An allowed command can read files outside the worktree, use the network and
  call other tools.
- The checks in `.studio.toml` and in `check` rules run on your machine, as
  you. Review a project's `.studio.toml` before adding a project you did not
  write, and a pack before adding it.
- Hire prompts and pack skills are instructions to an agent. A bad one can
  steer it. Read them.
- Budget limits cap spend as the studio sees it. They are not a billing
  control. Set limits in your Claude account too.

## Reporting a problem

Open a private security advisory on the repository, or contact the
maintainers. Please don't file public issues for vulnerabilities.
