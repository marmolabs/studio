//! Score / best / light meter, plus the title and game-over overlays.

use crate::palette;
use crate::states::{BestScore, CurrentRun, GameState};
use bevy::prelude::*;

#[derive(Component)]
struct ScoreText;
#[derive(Component)]
struct BestText;
#[derive(Component)]
struct MeterFill;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_hud)
            .add_systems(OnEnter(GameState::Title), spawn_title)
            .add_systems(OnEnter(GameState::GameOver), spawn_game_over)
            .add_systems(Update, update_hud);
    }
}

pub fn score_label(score: u32) -> String {
    format!("Sparks: {score}")
}

pub fn best_label(best: u32) -> String {
    format!("Best: {best}")
}

fn label(text: impl Into<String>, size: f32) -> impl Bundle {
    (
        Text::new(text),
        TextFont::from_font_size(size),
        TextColor(palette::TEXT),
    )
}

fn spawn_hud(mut commands: Commands) {
    commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            top: px(10),
            left: px(12),
            flex_direction: FlexDirection::Column,
            row_gap: px(4),
            ..default()
        })
        .with_children(|p| {
            p.spawn((ScoreText, label(score_label(0), 22.0)));
            p.spawn((BestText, label(best_label(0), 18.0)));
            p.spawn((
                Node {
                    width: px(160),
                    height: px(12),
                    ..default()
                },
                BackgroundColor(palette::METER_BACK),
            ))
            .with_children(|m| {
                m.spawn((
                    MeterFill,
                    Node {
                        width: percent(0),
                        height: percent(100),
                        ..default()
                    },
                    BackgroundColor(palette::METER_FILL),
                ));
            });
        });
}

fn update_hud(
    run: Res<CurrentRun>,
    best: Res<BestScore>,
    mut score: Query<&mut Text, (With<ScoreText>, Without<BestText>)>,
    mut best_text: Query<&mut Text, (With<BestText>, Without<ScoreText>)>,
    mut fill: Query<&mut Node, With<MeterFill>>,
) {
    for mut t in &mut score {
        t.0 = score_label(run.score);
    }
    for mut t in &mut best_text {
        t.0 = best_label(best.0.0);
    }
    for mut n in &mut fill {
        n.width = percent(run.light_fraction() * 100.0);
    }
}

fn overlay(commands: &mut Commands, state: GameState, lines: &[(&str, f32)]) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: px(14),
                ..default()
            },
            DespawnOnExit(state),
        ))
        .with_children(|p| {
            for (text, size) in lines {
                p.spawn(label(*text, *size));
            }
        });
}

fn spawn_title(mut commands: Commands) {
    overlay(
        &mut commands,
        GameState::Title,
        &[
            ("LANTERN", 64.0),
            ("Collect sparks. Avoid moths. Keep the light alive.", 20.0),
            ("Move: WASD or arrows", 18.0),
            ("Press Space to start", 26.0),
        ],
    );
}

fn spawn_game_over(mut commands: Commands, run: Res<CurrentRun>, best: Res<BestScore>) {
    let result = format!("{}   {}", score_label(run.score), best_label(best.0.0));
    overlay(
        &mut commands,
        GameState::GameOver,
        &[
            ("The light went out", 48.0),
            (&result, 24.0),
            ("Press Space to try again", 26.0),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_format() {
        assert_eq!(score_label(3), "Sparks: 3");
        assert_eq!(best_label(10), "Best: 10");
    }
}
