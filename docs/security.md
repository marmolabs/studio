# Security

The studio runs AI agents on your machine, with your privileges. This page
says what it does to limit that, and what it does not.

## Credentials

The studio never reads, stores or sends your Claude credentials. Agents run
through the `claude` CLI, which uses your own login or API key. The studio only
starts that program and reads what it prints.

## The web UI

`studio-web` can approve what agents do, so it is locked down:

- It listens on `127.0.0.1` only. It does not bind to other interfaces.
- Each start makes a new random token (32 bytes from the system's random
  source). You open the link it prints, `http://127.0.0.1:<port>/?token=...`.
- The token becomes a cookie that is `HttpOnly` and `SameSite=Strict`. The page
  then redirects, so the token leaves the address bar and history.
- Every request without the cookie gets a 401. Tokens are compared in constant
  time.
- A POST with an `Origin` header that is not the UI's own address is refused.

Do not forward the port or put it behind a proxy.

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

Two cheap hires work for the studio itself.

The approver decides tool calls that no rule covers, so only dangerous ones
reach you. It allows what is a normal part of the ticket inside its worktree,
denies what the ticket doesn't need, and sends to you what is dangerous,
irreversible or reaches outside the project. A denial does not stop the ticket:
the agent is told why and tries another way. Asks from your own rules (`protect`
rules and ask-level gates) and questions from agents always reach you, never the
approver. When the approver errors, has no budget or gives no clear answer, the
call goes to you, with the reason shown.

The router picks the hire for a ticket created without one (`studio ticket`
without `--hire`, the board's "Pick for me", or a producer's `file_ticket`
without a hire). Without a router, the first engineer takes it.

Each call is capped at $0.05 and charged to the ticket. `studio init` writes
both hires; deleting `hires/approver.md` or `hires/router.md` turns that one off.

## What is not protected

- Anything that can reach the `studiod` Unix socket acts as you. The socket
  is in the data folder with mode `0600`, so other accounts can't use it. But
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
