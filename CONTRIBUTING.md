# Contributing

Thanks for helping. This page covers how to build, test and change the studio.

## Build and test

You need Rust 1.98 (the repo pins it in `rust-toolchain.toml`, so `rustup`
installs it for you), `git`, and a network connection for the first build of
`studio-web`, which downloads a checksum-pinned Tailwind binary.

```sh
cargo build --workspace
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

`cargo test` runs everything with fake runtimes. You do not need API keys, a
`claude` login, or the `claude` binary installed.

The sample game in `examples/starter-2d` is its own Cargo workspace. Run its
checks from that folder. On Linux it needs `pkg-config`, `libudev-dev`,
`libwayland-dev` and `libxkbcommon-dev` to compile.

CI runs the same commands on Ubuntu and macOS. Please run them before you open
a pull request.

## Page gallery

`studio-web` has an example that serves the real web UI against a stand-in
`studiod` full of sample data and saves a screenshot of every page:

```sh
cargo run -q -p studio-web --example gallery -- /tmp/gallery
```

It uses a Chromium browser: `$STUDIO_BROWSER` if set, else Chrome, Brave or
Chromium. When none can run, it falls back to Quick Look on macOS; with
neither it prints a line and exits 0. Run it after a change to
the web UI and look at the images.

## Lint rules

The workspace forbids `unsafe` code and denies `unwrap`, `expect` and `panic!`
(see `[workspace.lints]` in `Cargo.toml`). Handle the error or return it.

Tests may use them. Opt out where the tests live:

```rust
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
```

Put that at the top of an integration test file, or on the `mod tests` block
of a unit test. Use the narrowest list that compiles. Do not add it to
non-test code.

## Crate map

- `studio-core`: pure logic with no I/O. Tickets, the state machine, rules,
  the policy that judges agent actions, budget arithmetic.
- `studio-config`: reads and writes `studio.toml`, `.studio.toml`, hires, rules
  and packs. Also writes what `studio init` creates.
- `studio-git`: worktrees, goal branches, diffs and owner commits.
- `studio-runtime`: the `Runtime` trait, the Claude Code adapter and a scripted
  fake for tests.
- `studio-store`: the SQLite event store and spend ledger.
- `studiod`: the daemon. Scheduler, workers, budget, checks, watchers, and the
  Unix socket API.
- `studio`: the CLI. A client of `studiod`, plus the `studio mcp` server.
- `studio-web`: the local web UI (Topcoat, Datastar, Tailwind). Also a client
  of `studiod`.

## Add a runtime adapter

A runtime turns an agent program's protocol into the studio's events.

1. In `studio-runtime`, add a module with a type that implements `Runtime`:
   `id`, `capabilities`, `check_env` and `start`. `start` returns a `Run` that
   yields `RunEvent`s and accepts `Control`s. `check_env` must never read
   credentials.
2. Export it from `crates/studio-runtime/src/lib.rs`.
3. In `crates/studiod/src/runtimes.rs`, add a provider kind: one more arm in
   the `match provider.kind.as_str()` in `build`. Users then name it with
   `[providers.<name>] kind = "..."` in `studio.toml`.
4. Test it with a scripted fake: a script or fixture that plays back the
   program's output, as `crates/studio-runtime/tests/claude.rs` does. Do not
   write tests that need the real program or a network.

## Write a pack

A pack is a folder with a `pack.toml` and optional `hires/`, `rules/`,
`skills/` and `project.toml`. See [docs/packs.md](docs/packs.md). Test it by
loading it, as `crates/studio-config/tests/bevy_ios.rs` does for `bevy-ios`.

## Commits and pull requests

- Write the subject in the imperative, under about 72 characters: "Add the
  goals page", not "Added" or "Adds".
- Say why in the body when the reason is not obvious.
- No AI attribution trailers (no `Co-Authored-By` for a model, no tool
  footers) on commits or pull requests.
- Keep a change to one topic. Add or update tests with it.

Agents write code for this project too. Their code is reviewed like anyone's:
it needs the same checks, the same tests, and a human who has read the diff.
