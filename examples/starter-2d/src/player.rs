//! The lantern-bearer: movement and the glowing halo.

use crate::convert::xy;
use crate::input::Intent;
use crate::palette;
use crate::rules::{self, V2};
use crate::states::{CurrentRun, GameState};
use bevy::prelude::*;

#[derive(Component)]
pub struct Player;

/// The translucent circle showing the lit area; child of the player.
#[derive(Component)]
pub struct Halo;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(GameState::Playing), spawn_player)
            .add_systems(
                Update,
                (move_player, update_halo).run_if(in_state(GameState::Playing)),
            );
    }
}

fn spawn_player(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    commands
        .spawn((
            Player,
            Mesh2d(meshes.add(Circle::new(rules::PLAYER_RADIUS))),
            MeshMaterial2d(materials.add(palette::PLAYER)),
            Transform::from_xyz(0.0, 0.0, 3.0),
            // Stays visible on the game-over screen; cleared on restart.
            DespawnOnExit(GameState::GameOver),
        ))
        .with_children(|p| {
            p.spawn((
                Halo,
                Mesh2d(meshes.add(Circle::new(1.0))),
                MeshMaterial2d(materials.add(palette::HALO)),
                Transform::from_xyz(0.0, 0.0, -2.0),
            ));
        });
}

fn move_player(
    time: Res<Time>,
    intent: Res<Intent>,
    mut player: Query<&mut Transform, With<Player>>,
) {
    for mut tf in &mut player {
        let next = rules::step_player(xy(tf.translation), intent.dir, time.delta_secs());
        tf.translation.x = next.x;
        tf.translation.y = next.y;
    }
}

fn update_halo(run: Res<CurrentRun>, mut halo: Query<&mut Transform, With<Halo>>) {
    let r = run.light_radius();
    for mut tf in &mut halo {
        tf.scale = Vec3::new(r, r, 1.0);
    }
}

/// Current player position, if a player exists.
pub fn player_pos(player: &Query<&Transform, With<Player>>) -> Option<V2> {
    player.iter().next().map(|t| xy(t.translation))
}
