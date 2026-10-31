# Agent guide

This is a sample project fixture for proposal tests.

## Before a PR

Run the local verification block before opening a pull request.

```sh
cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace
```

## Owner-only files

Only the owner may edit `crates/engine/src/rules.rs` and `release.toml`.
These paths are out of bounds for autonomous changes.
Out of bounds without the owner: data formats, the app id `com.example.Shop` (never renamed), and the tuning in `rules.rs`.
