# Conventions

- Rust 1.98.0 (pinned), Bevy 0.20, edition 2024. No image/audio assets; draw with shapes.
- Game logic goes in `src/rules.rs` as pure functions/structs with no Bevy types and an injected `Rng`. Every rule gets a unit test.
- Bevy code is one plugin per concern (`input`, `states`, `player`, `spawning`, `hud`) that calls into `rules`. Keep systems small. New tunables are constants at the top of `rules.rs`.
- Gameplay reads `input::Intent`, never the keyboard directly (the `--capture` script overrides it).
- No `unwrap`, `expect`, `panic!` or `unsafe` (denied by lints). Tests do not need them either; if one does, add a scoped `#[allow(...)]`.
- Do not add dependencies or Bevy features without a good reason. Keep `Cargo.lock` committed.

# Before a PR

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

For visible changes also run `cargo run --release -- --capture <dir>` and check the three PNGs.
