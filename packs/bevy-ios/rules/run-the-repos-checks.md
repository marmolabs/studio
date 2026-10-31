---
kind: guide
level: warn
---
Before you finish, run the repo's own checks exactly as its CLAUDE.md or
AGENTS.md lists them (usually `cargo fmt --check`, `cargo clippy --all-targets
-- -D warnings` and `cargo test --locked`) and fix every failure at its cause.
Keep engine and rules logic in plain Rust with no Bevy types so it stays
testable headless; Bevy systems should only read input, call into the rules and
draw the result.
