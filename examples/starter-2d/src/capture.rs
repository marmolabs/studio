//! `--capture <dir>`: run with a fixed seed, scripted input and a fixed frame
//! clock, save three screenshots (title, mid-play, game over) and exit.

use crate::convert::xy;
use crate::input::{Intent, IntentSet};
use crate::player::{Player, player_pos};
use crate::rules::V2;
use crate::spawning::Spark;
use crate::states::{BestScore, CurrentRun, GameState};
use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::time::TimeUpdateStrategy;
use bevy::window::{PrimaryWindow, WindowLevel};
use std::path::PathBuf;
use std::time::Duration;

/// Seed used for every capture run, so results are reproducible.
pub const CAPTURE_SEED: u64 = 0x1A47_E2B5;
/// Simulated frame rate (the clock is manual, independent of real time).
pub const FPS: f64 = 60.0;
/// Frame at which the title screenshot is taken.
pub const TITLE_SHOT_FRAME: u32 = 15;
/// Frame at which the scripted Space press starts the run.
pub const START_FRAME: u32 = 30;
const _: () = assert!(TITLE_SHOT_FRAME < START_FRAME);
/// Frames of play before the mid-play screenshot.
pub const PLAY_FRAMES_BEFORE_SHOT: u32 = 360;
/// Frames to wait after a state change before the game-over screenshot.
pub const SETTLE_FRAMES: u32 = 15;
/// Give up (exit with an error) if the run has not finished by this frame.
pub const MAX_FRAMES: u32 = 6000;

pub const TITLE_FILE: &str = "title.png";
pub const PLAYING_FILE: &str = "playing.png";
pub const GAME_OVER_FILE: &str = "game-over.png";

/// Scripted input for a given frame while the run has not started or has
/// ended: Space at [`START_FRAME`], nothing otherwise. During the play window
/// the bot in [`autopilot`] supplies the direction instead.
pub fn script(frame: u32) -> Intent {
    Intent {
        dir: V2::ZERO,
        confirm: frame == START_FRAME,
    }
}

/// True while the scripted bot should be steering.
pub fn bot_active(frame: u32) -> bool {
    frame > START_FRAME && frame <= START_FRAME + PLAY_FRAMES_BEFORE_SHOT
}

/// A simple bot: head for the nearest spark, or stay put if there is none.
/// This gives the playtest screenshots some real gameplay in them.
pub fn autopilot(player: V2, sparks: &[V2]) -> V2 {
    sparks
        .iter()
        .min_by(|a, b| player.distance(**a).total_cmp(&player.distance(**b)))
        .map_or(V2::ZERO, |s| (*s - player).normalized_or_zero())
}

#[derive(Resource)]
struct Capture {
    dir: PathBuf,
    frame: u32,
    over_since: Option<u32>,
    shots: Vec<PathBuf>,
}

pub struct CapturePlugin {
    pub dir: PathBuf,
}

impl Plugin for CapturePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / FPS,
        )))
        .insert_resource(Capture {
            dir: self.dir.clone(),
            frame: 0,
            over_since: None,
            shots: Vec::new(),
        })
        .add_systems(Startup, keep_on_top)
        .add_systems(PreUpdate, override_intent.after(IntentSet))
        .add_systems(Last, drive_capture);
    }
}

/// macOS stops drawing a window that other windows cover, and a covered
/// window's screenshots come out black. A capture keeps its window on top.
fn keep_on_top(mut window: Single<&mut Window, With<PrimaryWindow>>) {
    window.window_level = WindowLevel::AlwaysOnTop;
}

fn override_intent(
    capture: Res<Capture>,
    mut intent: ResMut<Intent>,
    player: Query<&Transform, With<Player>>,
    sparks: Query<&Transform, With<Spark>>,
) {
    *intent = script(capture.frame);
    if bot_active(capture.frame)
        && let Some(p) = player_pos(&player)
    {
        let targets: Vec<V2> = sparks.iter().map(|t| xy(t.translation)).collect();
        intent.dir = autopilot(p, &targets);
    }
}

fn shoot(commands: &mut Commands, capture: &mut Capture, file: &str) {
    let path = capture.dir.join(file);
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path.clone()));
    capture.shots.push(path);
}

fn drive_capture(
    mut commands: Commands,
    mut capture: ResMut<Capture>,
    state: Res<State<GameState>>,
    run: Res<CurrentRun>,
    best: Res<BestScore>,
    mut exit: MessageWriter<AppExit>,
) {
    capture.frame += 1;
    let frame = capture.frame;

    if frame == TITLE_SHOT_FRAME {
        shoot(&mut commands, &mut capture, TITLE_FILE);
    }
    if frame == START_FRAME + PLAY_FRAMES_BEFORE_SHOT {
        shoot(&mut commands, &mut capture, PLAYING_FILE);
    }
    if *state.get() == GameState::GameOver && frame > START_FRAME {
        let since = *capture.over_since.get_or_insert(frame);
        if frame == since + SETTLE_FRAMES {
            shoot(&mut commands, &mut capture, GAME_OVER_FILE);
        }
    }

    let all_taken = capture.shots.len() == 3;
    if all_taken && capture.shots.iter().all(|p| p.exists()) {
        println!(
            "capture complete: {} (score {}, best {})",
            capture.dir.display(),
            run.score,
            best.0.0
        );
        exit.write(AppExit::Success);
    } else if frame >= MAX_FRAMES {
        eprintln!("capture failed: run did not finish within {MAX_FRAMES} frames");
        exit.write(AppExit::error());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_fires_once_at_start() {
        let presses: Vec<u32> = (0..MAX_FRAMES).filter(|f| script(*f).confirm).collect();
        assert_eq!(presses, vec![START_FRAME]);
    }

    #[test]
    fn bot_window_matches_the_run() {
        assert!(!bot_active(START_FRAME));
        assert!(bot_active(START_FRAME + 1));
        assert!(bot_active(START_FRAME + PLAY_FRAMES_BEFORE_SHOT));
        assert!(!bot_active(START_FRAME + PLAY_FRAMES_BEFORE_SHOT + 1));
    }

    #[test]
    fn autopilot_heads_for_the_nearest_spark() {
        let sparks = [V2::new(100.0, 0.0), V2::new(0.0, 10.0)];
        assert_eq!(autopilot(V2::ZERO, &sparks), V2::new(0.0, 1.0));
        assert_eq!(autopilot(V2::ZERO, &[]), V2::ZERO);
    }
}
