# Contributing

Thanks for helping. This page covers how to build, run, test and change the
studio. To use it instead, read the [README](README.md).

## Build and run

You need Rust 1.98 (the repo pins it in `rust-toolchain.toml`, so `rustup`
installs it for you), `git`, and a network connection for the first build of
`studio-web`, which downloads a checksum-pinned Tailwind binary. To run agents
you also need the `claude` CLI, logged in.

```sh
cargo build --workspace
```

Try your build in a throwaway studio, so it never touches your own:

```sh
export STUDIO_HOME=$(mktemp -d)
target/debug/studio init
target/debug/studio project add ~/code/some-git-repo
target/debug/studio up --no-open --port 4899   # prints the link to open
target/debug/studio status
target/debug/studio down                       # stops the daemon
```

`studio up` starts the `studiod` and `studio-web` that sit next to the `studio`
binary, so build the whole workspace first. Ctrl-C stops the web UI; `studio down`
stops the daemon. Its log is `$STUDIO_HOME/data/studiod.log`.

## Checks

CI (`.github/workflows/ci.yml`) runs these on every pull request and push to
`master`; the tests run on Ubuntu and macOS. Run them before you open a pull
request:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The tests use scripted runtimes. You do not need API keys, a `claude` login or
the `claude` binary.

The workspace forbids `unsafe` code and denies `unwrap`, `expect` and `panic!`
(see `[workspace.lints]` in `Cargo.toml`). Handle the error or return it. Tests
may use them: put

```rust
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
```

at the top of an integration test file, or on the `mod tests` block of a unit
test, using the narrowest list that compiles. Do not add it to other code.

## The gallery

For a change to the web UI, run the gallery and look at the pictures:

```sh
cargo run -q -p studio-web --example gallery -- /tmp/gallery
```

It starts real studios in temporary homes, with scripted agents doing the work,
and saves a screenshot of every page at desktop and phone widths: HQ, a
conversation for each phase a project can be in, Look closer on ready work and
on a ready goal, the office (and with a project picked), New project, a project
being set up in each state of its setup, the studio settings, and the task,
hires and rules pages. Lead turns can't run in a gallery, so what the leads said
(the shop's conversation, every lead's standup) is written into the studio's
data folder before it is served. It uses a Chromium browser (`$STUDIO_BROWSER`
if set, else Chrome, Brave or Chromium), falls back to Quick Look on macOS, and
with neither prints a line and exits 0. These pictures are the reference for the
team pages: [crates/studio-web/design/README.md](crates/studio-web/design/README.md)
says what the pages should look like, and when a page changes on purpose, render
the gallery again and read every picture.

## Layout

- `crates/studio-core`: pure logic with no I/O. Tickets, the state machine,
  rules, the policy that judges agent actions, budget arithmetic.
- `crates/studio-config`: reads and writes `studio.toml`, `.studio.toml`,
  hires, rules and packs, and writes what `studio init` creates.
- `crates/studio-git`: worktrees, goal branches, diffs and owner commits.
- `crates/studio-runtime`: the `Runtime` trait, the Claude Code adapter and a
  scripted fake for tests.
- `crates/studio-store`: the SQLite event store and spend ledger.
- `crates/studiod`: the daemon. Scheduler, workers, budget, checks, watchers
  and the Unix socket API.
- `crates/studio`: the CLI, a client of `studiod`, plus the `studio mcp`
  server an agent uses to reach the studio.
- `crates/studio-web`: the web UI (Topcoat, Datastar, Tailwind), also a client
  of `studiod`. `design/` holds the design notes and prototype, `examples/` the
  gallery (and its studios), `vendor/` the fonts and icons.
- `docs/`: the guides. `install.sh`: the release installer.
  `.github/workflows/`: CI and releases.

### One path through it

You open `http://127.0.0.1:4870/`. `studio-web` serves HQ, the first of the team
pages in `crates/studio-web/src/pages/team/` (`hq.rs`). Every team page (HQ, a
lead's conversation in `talk.rs` and `talk/`, the Look closer sheet in
`sheet.rs`, the office in `office.rs`, New project in `new.rs` and the studio
settings in `settings.rs`) draws itself whole inside `shell.rs`, the team
sidebar. It asks `studiod` for one `Office` snapshot, a single
`Request::Office` over the Unix socket (`client::call`), and works the team
out of it (`mod.rs`: a lead for each project, what waits for the owner, what
was spent). The page then opens `/team/stream` (`stream.rs`), which asks again
every second (open pages share one read), draws the page again and sends only
what changed; Datastar morphs it into the page. A conversation also reads its
messages from `studiod` (`Request::Conversation`).

Clicking a control posts to a `studio-web` route, which sends a `Request`
(`Land`, `Decide`, `Talk`, `Tell` and so on). `studiod` reads it in
`server.rs` and `Studio::handle` in `studio.rs` runs it. State changes are
`Event`s: the store (`studio-store`) applies each one to the task with the
state machine in `studio-core/src/machine.rs` and saves it. The scheduler and
`worker.rs` claim queued tasks, make a worktree (`studio-git`), start the
hire's `Runtime`, judge its tool calls with the rules (`studio-core/src/policy.rs`),
run the checks and review, and append events as they go. The next snapshot
shows them. The CLI takes the same path with no page in between.

A message to a lead is `Request::Talk`. `studiod/src/studio/talk.rs` stores it
in `<data>/talk/<project>.jsonl` and starts a lead turn through
`studiod/src/leadrun.rs`, which runs the lead's hire with read-only tools and
the studio's own tools (`studio mcp --talk <project>`, in
`crates/studio/src/mcp.rs`); each tool the lead uses comes back as a request
and posts a card to the conversation. `studio/setup.rs` does the same for a
new project (`studio mcp --setup <project>`, whose one tool proposes how the
project runs, which the studio then tries), `studio/standup.rs` writes the
standups and the one-line results, `studio/shipcheck.rs` runs a finished goal's
checks, and `studio/tell.rs` gets a message to a running agent.

## Where to start

- **A new `studiod` request.** Add a variant to `Request` in
  `crates/studiod/src/api.rs` (and a reply type there if it needs one), a
  match arm in `Studio::handle` in `crates/studiod/src/studio.rs`, and the work
  in a method under `crates/studiod/src/studio/`. Test it in
  `crates/studiod/tests/e2e.rs`. A CLI command goes in
  `crates/studio/src/main.rs`; a page reads it with `client::call`. If a page
  needs it every second, add it to `OfficeView` in `api.rs` and fill it in
  `studio/snapshot.rs` instead.
- **A new team page.** Add a file under `pages/team/` that draws itself with
  `shell::page` as a function of the team and the office, a route for it in
  `pages/team/mod.rs`, and its address to the stream in `stream.rs`. Words are
  plain and from the owner's side: task, ship, the lead. Run the gallery, add
  the page to its list in `examples/gallery/main.rs`, and look at it.
- **What a lead can do.** A tool of a lead's conversation is in
  `crates/studio/src/mcp.rs` (the `--talk` mode), with the request it makes in
  `api.rs` (`LeadActs`) and the work in `studio/talk.rs`. Its card is a
  `TalkCard` in `api.rs`, drawn in `pages/team/talk/cards.rs`.
- **The company floor.** The drawing is in `pages/office/` (`hq_scene.rs`, on
  `iso.rs`'s isometric drawing; `plan.rs` and `facts.rs` work out each team
  area and its phase; `pins.rs` the signs) and the office page in
  `pages/team/office.rs` shows it. A new shared room or prop is a variant of
  `Element` in `pages/office/model.rs`, drawn with `el` in `hq_scene.rs`. A new
  phase is a variant of `Phase`, a rule in `facts.rs`'s `phase`, and its word
  in the sidebar and the conversation's header (`pages/team/`).
- **A face.** A hire's portrait is drawn in `pages/team/faces.rs` from the
  hire id, with the skin, hair and shirt the floor's figures have.
- **The release code.** `crates/studiod/src/studio/releases.rs` runs a release
  (final checks, the go-ahead, the commands, the tag) and keeps its record in
  the data folder; its requests (`StartRelease`, `ApproveRelease`,
  `CancelRelease`, `Releases`) are in `api.rs`. The git side, the release
  checkout and the tag, is `crates/studio-git/src/releases.rs`; the commands
  are in `crates/studio/src/main.rs`. A project's `release` key is in
  `crates/studio-config/src/project.rs`. Describe a change in
  `docs/configuration.md`.
- **A new hire role.** A role is free text in a hire's file. A role with
  meaning to the daemon is matched by name where it is used (search
  `crates/studiod/src` for `first_with_role` and `.role ==`): `reviewer`,
  `producer`, `approver` and `router`, and `lead`, which only keeps the router
  from offering that hire tickets (a project's lead is named in `studio.toml`,
  and `lead_of` in `studiod/src/studio.rs` finds it). To show it on the
  floor, add it to `Role` in `pages/office/model.rs` (and its colour in `src/app.css`) and to `ROLES` in
  `pages/office/mod.rs`, which the Hire form offers. Starter hires live in
  `crates/studio-config/src/starter.rs`.
- **A new rule kind.** Add it to `RuleKind` in `crates/studio-core/src/rule.rs`
  with what it requires, make `crates/studio-core/src/policy.rs` act on it,
  and check `studiod/src/studio/files.rs` (saving) and `studiod/src/ruletest.rs`
  (replaying). Describe it in `docs/rules.md`.
- **A new runtime.** See the next section.

## Add a runtime adapter

A runtime turns an agent program's protocol into the studio's events.

1. In `studio-runtime`, add a module with a type that implements `Runtime`:
   `id`, `capabilities`, `check_env` and `start`. `start` returns a `Run` that
   yields `RunEvent`s and accepts `Control`s. `check_env` must never read
   credentials.
2. Export it from `crates/studio-runtime/src/lib.rs`.
3. In `crates/studiod/src/runtimes.rs`, add it to the list `build` returns.
   Users then name it with `runtime: <name>` in a hire.
4. Test it with a scripted fake: a script or fixture that plays back the
   program's output, as `crates/studio-runtime/src/claude/recorded.rs` does. Do
   not write tests that need the real program or a network.

## Write a pack

A pack is a folder with a `pack.toml` and optional `hires/`, `rules/`,
`skills/` and `project.toml`. See [docs/packs.md](docs/packs.md). Test it by
adding it to a test studio with `studio pack add` and running `studio doctor`.

## Commits and pull requests

- Write the subject in the imperative, under about 72 characters: "Add the
  goals page", not "Added" or "Adds".
- Say why in the body when the reason is not obvious.
- No AI attribution trailers (no `Co-Authored-By` for a model, no tool
  footers) on commits or pull requests.
- Keep a change to one topic. Add or update tests with it.
- Code from an agent is reviewed like anyone's: the same checks, the same
  tests, and a person who has read the diff.

## Releases

1. Set the version in `[workspace.package]` in `Cargo.toml` and give it a
   section in `CHANGELOG.md` (`## 0.2.0`). The release notes come from that
   section.
2. Once that is on `master`, push the tag (`v0.2.0`).
3. The release workflow checks that the tag matches the version, builds the
   macOS and Linux archives, and creates a draft release with them and
   `SHA256SUMS`. Read it, then publish it: `install.sh` installs the latest
   published release.

A pull request that changes the release workflow, `install.sh`, `Cargo.lock`,
the toolchain or a build script runs the same builds, without publishing.
