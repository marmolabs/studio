---
name: bevy-capture
description: Add or use a --capture mode that plays a Bevy game with scripted input and saves screenshots.
---
# Bevy capture mode

A capture mode lets a human or an agent see what the game looks like without
playing it. `cargo run --release -- --capture <dir>` runs the game with a fixed
seed and scripted input, saves a few screenshots into `<dir>`, and exits.

## Using it

1. Run `cargo run --release -- --capture <dir>` with `<dir>` outside tracked
   files (the studio passes its own folder as `{out}`).
2. Open each PNG and look at it. Check that the change you made is visible and
   that nothing is cut off, overlapping or unreadable.
3. A non-zero exit or a missing PNG means the game crashed or hung. Fix that
   first; it counts as a failed check.

It needs a real window, so it runs on a desktop session, not a headless server.

## Adding it to a game

- Parse `--capture <dir>` in `main` before building the app. Without the flag the
  game runs normally.
- Use a fixed seed for every random choice, and a manual clock
  (`TimeUpdateStrategy::ManualDuration`, 1/60 s per frame) so runs are
  reproducible whatever the machine's speed.
- Gameplay reads one input resource (an "intent") instead of the keyboard, so
  the capture script can set it frame by frame.
- Take the shot with Bevy's `Screenshot::primary_window()` and
  `save_to_disk(path)` at fixed frames: one for each screen that matters (title,
  mid-play, game over, and any screen the change touches). Name files for the
  screen: `title.png`, `playing.png`, `game-over.png`.
- Wait a few frames after a state change before shooting so the UI has settled.
- Exit with `AppExit::Success` after the last file is written, and with an error
  if the script has not finished after a frame limit, so a hang cannot go
  unnoticed.
- Keep the capture code in its own module and plugin, and its frame numbers as
  named constants.
