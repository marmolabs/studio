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
and saves a screenshot of every page at desktop and phone widths. It uses a
Chromium browser (`$STUDIO_BROWSER` if set, else Chrome, Brave or Chromium),
falls back to Quick Look on macOS, and with neither prints a line and exits 0.
[crates/studio-web/design/README.md](crates/studio-web/design/README.md) says
what the pages should look like.

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
  gallery, `vendor/` the fonts and icons.
- `docs/`: the guides. `install.sh`: the release installer.
  `.github/workflows/`: CI and releases.

### One path through it

You open `http://127.0.0.1:4870/`. `studio-web` serves the office page
(`office_page` in `crates/studio-web/src/pages/office/mod.rs`). It asks
`studiod` for one `Office` snapshot, a single `Request::Office` over the Unix
socket (`client::call`), and draws the floor (`scene.rs`), the top bar (`hq.rs`)
and the side panel (`panel/`, `inspector.rs`) from it. The page then opens
`/office/stream`, which asks again every second (open pages share one read),
re-renders and sends only what changed; Datastar morphs it into the page.

Clicking a control posts to a `studio-web` route, which sends a `Request`
(`Land`, `Decide`, `CreateTicket` and so on). `studiod` reads it in
`server.rs` and `Studio::handle` in `studio.rs` runs it. State changes are
`Event`s: the store (`studio-store`) applies each one to the ticket with the
state machine in `studio-core/src/machine.rs` and saves it. The scheduler and
`worker.rs` claim queued tickets, make a worktree (`studio-git`), start the
hire's `Runtime`, judge its tool calls with the rules (`studio-core/src/policy.rs`),
run the checks and review, and append events as they go. The next snapshot
shows them. The CLI takes the same path with no page in between.

## Where to start

- **A new `studiod` request.** Add a variant to `Request` in
  `crates/studiod/src/api.rs` (and a reply type there if it needs one), a
  match arm in `Studio::handle` in `crates/studiod/src/studio.rs`, and the work
  in a method under `crates/studiod/src/studio/`. Test it in
  `crates/studiod/tests/e2e.rs`. A CLI command goes in
  `crates/studio/src/main.rs`; a page reads it with `client::call`. If the
  office needs it every second, add it to `OfficeView` in `api.rs` and fill it
  in `studio/snapshot.rs` instead.
- **A new element in the office.** Add a variant to `Element` in
  `crates/studio-web/src/pages/office/model.rs` (`ALL`, `key`, `hint`), draw it
  with `el` in `pages/office/scene.rs`, and give it a panel in
  `pages/office/panel/elements.rs` (its title and icon, and its view). Run the gallery.
- **A new tab.** Add it to `TABS` in `pages/office/panel/mod.rs`, with a pane in
  `tabs` there and the view that fills it. Anything it needs to read goes in the
  panel's `Extra` or in the `Office` snapshot.
- **A new hire role.** A role is free text in a hire's file. A role with
  meaning to the daemon is matched by name where it is used (search
  `crates/studiod/src` for `first_with_role` and `.role ==`): `reviewer`,
  `producer`, `approver` and `router`. To show it in the office, add it to `Role` in
  `pages/office/model.rs` (and its colour in `src/app.css`) and to `ROLES` in
  `pages/office/mod.rs`, which the Hire form offers. Starter hires live in
  `crates/studio-config/src/starter.rs`.
- **A new rule kind.** Add it to `RuleKind` in `crates/studio-core/src/rule.rs`
  with what it requires, make `crates/studio-core/src/policy.rs` act on it,
  and check `studiod/src/studio/files.rs` (saving) and `studiod/src/ruletest.rs`
  (replaying). Describe it in `docs/configuration.md`.
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
