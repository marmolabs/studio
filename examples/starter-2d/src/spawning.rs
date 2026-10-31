//! Sparks and moths: spawning, drifting, and collisions with the lantern.

use crate::convert::xy;
use crate::palette;
use crate::player::{Player, player_pos};
use crate::rules::{self, Rng, SpawnKind, Spawner, V2};
use crate::states::{CurrentRun, GameState};
use bevy::prelude::*;

/// A drifting spark. Collect it for score and light.
#[derive(Component)]
pub struct Spark {
    pub vel: V2,
}

/// A moth chasing the lantern. Touching it drains light.
#[derive(Component)]
pub struct Moth;

/// Seeded RNG for all gameplay randomness.
#[derive(Resource)]
pub struct GameRng(pub Rng);

#[derive(Resource, Default)]
struct SpawnTimers(Spawner);

pub struct SpawningPlugin {
    pub seed: u64,
}

impl Plugin for SpawningPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(GameRng(Rng::new(self.seed)))
            .init_resource::<SpawnTimers>()
            .add_systems(OnEnter(GameState::Playing), reset_timers)
            .add_systems(
                Update,
                (
                    spawn_entities,
                    drift_sparks,
                    chase_with_moths,
                    collect_sparks,
                    moth_contact,
                    despawn_strays,
                )
                    .chain()
                    .run_if(in_state(GameState::Playing)),
            );
    }
}

fn reset_timers(mut timers: ResMut<SpawnTimers>) {
    timers.0 = Spawner::default();
}

fn spawn_entities(
    mut commands: Commands,
    time: Res<Time>,
    run: Res<CurrentRun>,
    mut timers: ResMut<SpawnTimers>,
    mut rng: ResMut<GameRng>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    for s in timers.0.tick(time.delta_secs(), run.elapsed, &mut rng.0) {
        let (radius, color, z) = match s.kind {
            SpawnKind::Spark => (rules::SPARK_RADIUS, palette::SPARK, 1.0),
            SpawnKind::Moth => (rules::MOTH_RADIUS, palette::MOTH, 2.0),
        };
        let mut entity = commands.spawn((
            Mesh2d(meshes.add(Circle::new(radius))),
            MeshMaterial2d(materials.add(color)),
            Transform::from_xyz(s.pos.x, s.pos.y, z),
            DespawnOnExit(GameState::GameOver),
        ));
        match s.kind {
            SpawnKind::Spark => entity.insert(Spark { vel: s.vel }),
            SpawnKind::Moth => entity.insert(Moth),
        };
    }
}

fn drift_sparks(time: Res<Time>, mut sparks: Query<(&Spark, &mut Transform)>) {
    for (spark, mut tf) in &mut sparks {
        tf.translation.x += spark.vel.x * time.delta_secs();
        tf.translation.y += spark.vel.y * time.delta_secs();
    }
}

fn chase_with_moths(
    time: Res<Time>,
    run: Res<CurrentRun>,
    player: Query<&Transform, With<Player>>,
    mut moths: Query<&mut Transform, (With<Moth>, Without<Player>)>,
) {
    let Some(target) = player_pos(&player) else {
        return;
    };
    let speed = rules::moth_speed(run.elapsed);
    for mut tf in &mut moths {
        let next = rules::step_moth(xy(tf.translation), target, speed, time.delta_secs());
        tf.translation.x = next.x;
        tf.translation.y = next.y;
    }
}

fn collect_sparks(
    mut commands: Commands,
    mut run: ResMut<CurrentRun>,
    player: Query<&Transform, With<Player>>,
    sparks: Query<(Entity, &Transform), With<Spark>>,
) {
    let Some(p) = player_pos(&player) else {
        return;
    };
    for (entity, tf) in &sparks {
        if rules::circles_overlap(
            p,
            rules::PLAYER_RADIUS,
            xy(tf.translation),
            rules::SPARK_RADIUS,
        ) {
            run.collect_spark();
            commands.entity(entity).despawn();
        }
    }
}

fn moth_contact(
    mut commands: Commands,
    mut run: ResMut<CurrentRun>,
    player: Query<&Transform, With<Player>>,
    moths: Query<(Entity, &Transform), With<Moth>>,
) {
    let Some(p) = player_pos(&player) else {
        return;
    };
    for (entity, tf) in &moths {
        if rules::circles_overlap(
            p,
            rules::PLAYER_RADIUS,
            xy(tf.translation),
            rules::MOTH_RADIUS,
        ) {
            run.hit_by_moth();
            commands.entity(entity).despawn();
        }
    }
}

fn despawn_strays(mut commands: Commands, sparks: Query<(Entity, &Transform), With<Spark>>) {
    for (entity, tf) in &sparks {
        if rules::is_far_outside(xy(tf.translation)) {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::LIGHT_START;

    fn world_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<CurrentRun>()
            .add_systems(
                Update,
                (collect_sparks, moth_contact, despawn_strays).chain(),
            );
        app
    }

    fn spawn_at(app: &mut App, x: f32, y: f32, bundle: impl Bundle) -> Entity {
        app.world_mut()
            .spawn((bundle, Transform::from_xyz(x, y, 0.0)))
            .id()
    }

    #[test]
    fn touching_a_spark_scores_and_removes_it() {
        let mut app = world_app();
        spawn_at(&mut app, 0.0, 0.0, Player);
        let spark = spawn_at(&mut app, 5.0, 0.0, Spark { vel: V2::ZERO });
        let far = spawn_at(&mut app, 200.0, 0.0, Spark { vel: V2::ZERO });
        app.update();
        assert_eq!(app.world().resource::<CurrentRun>().score, 1);
        assert!(app.world().get_entity(spark).is_err());
        assert!(app.world().get_entity(far).is_ok());
    }

    #[test]
    fn touching_a_moth_drains_light_without_scoring() {
        let mut app = world_app();
        spawn_at(&mut app, 0.0, 0.0, Player);
        let moth = spawn_at(&mut app, 3.0, 0.0, Moth);
        app.update();
        let run = app.world().resource::<CurrentRun>();
        assert_eq!(run.score, 0);
        assert!(run.light < LIGHT_START);
        assert!(app.world().get_entity(moth).is_err());
    }

    #[test]
    fn strays_are_despawned() {
        let mut app = world_app();
        let stray = spawn_at(&mut app, 5000.0, 0.0, Spark { vel: V2::ZERO });
        app.update();
        assert!(app.world().get_entity(stray).is_err());
    }
}
