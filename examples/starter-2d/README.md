# Lantern (starter-2d)

A tiny 2D game built with [Bevy](https://bevy.org) 0.20, used as a sample
project for AI coding agents. You steer a small lantern-bearer around one dark
screen collecting sparks that drift in from the edges. Moths are drawn to your
light and drain it on contact. Your light shrinks over time and grows with each
spark; when it goes out the run ends. Score is sparks collected; the best score
is kept for the session. Everything is drawn with simple shapes (no assets).

Controls: WASD or arrow keys to move, Space to start / restart.

## Run

```sh
cargo run --release
```

The toolchain is pinned to Rust 1.98.0 in `rust-toolchain.toml`.

## Tests and checks

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
```

Game logic lives in `src/rules.rs` and has no Bevy types, so most behaviour is
covered by plain unit tests. The Bevy plugins also have small headless tests.

## Layout

| File | Role |
| --- | --- |
| `src/rules.rs` | Pure rules: light, scoring, collisions, spawning, seeded RNG, phase transitions |
| `src/states.rs` | `GameState` (Title, Playing, GameOver), current run, best score |
| `src/input.rs` | Keyboard to an `Intent` resource (gameplay reads only this) |
| `src/player.rs` | Player movement and the light halo |
| `src/spawning.rs` | Sparks and moths: spawn, move, collide |
| `src/hud.rs` | Score, best, light meter, title and game-over screens |
| `src/capture.rs` | The `--capture` playtest mode |

## Capture mode

```sh
cargo run --release -- --capture <dir>
```

Runs with a fixed seed, a manual 60 fps clock and scripted input, saves
`title.png`, `playing.png` and `game-over.png` into `<dir>` using Bevy's
screenshot API, then exits with code 0 (non-zero if the run did not finish).
It needs a real window, so run it on a desktop session.

## Good first tickets

1. Add a pause menu: Escape toggles a `Paused` state with a resume prompt.
2. Add a second enemy type, for example a fast "wisp" that ignores the light, with its own rules and tests.
3. Add a power-up spark that briefly makes the light radius max out.
4. Add screen shake and a brief flash when a moth hits the lantern.
5. Add mouse or touch follow as an alternative to keyboard movement (feed it into `Intent`).
6. Persist the best score to disk between sessions.
