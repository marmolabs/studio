//! Turns raw keyboard state into a small [`Intent`] resource. Gameplay only
//! reads `Intent`, which lets `--capture` script input without a keyboard.

use crate::rules::V2;
use bevy::prelude::*;

/// What the player wants this frame.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub struct Intent {
    /// Desired movement direction (not necessarily normalized).
    pub dir: V2,
    /// True on the frame the confirm key (Space) was pressed.
    pub confirm: bool,
}

/// System set (in `PreUpdate`) in which `Intent` is written; anything that
/// overrides input, like the capture script, orders itself after this set.
/// Read `Intent` in `Update` or later.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct IntentSet;

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Intent>()
            .add_systems(PreUpdate, read_keyboard.in_set(IntentSet));
    }
}

/// Direction from held keys: WASD or arrows.
pub fn direction_from_keys(up: bool, down: bool, left: bool, right: bool) -> V2 {
    V2::new(
        f32::from(u8::from(right)) - f32::from(u8::from(left)),
        f32::from(u8::from(up)) - f32::from(u8::from(down)),
    )
}

fn read_keyboard(keys: Res<ButtonInput<KeyCode>>, mut intent: ResMut<Intent>) {
    intent.dir = direction_from_keys(
        keys.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp]),
        keys.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown]),
        keys.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft]),
        keys.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]),
    );
    intent.confirm = keys.just_pressed(KeyCode::Space);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_directions() {
        assert_eq!(
            direction_from_keys(true, false, false, false),
            V2::new(0.0, 1.0)
        );
        assert_eq!(
            direction_from_keys(false, true, true, false),
            V2::new(-1.0, -1.0)
        );
        assert_eq!(direction_from_keys(true, true, true, true), V2::ZERO);
        assert_eq!(direction_from_keys(false, false, false, false), V2::ZERO);
    }

    #[test]
    fn keyboard_system_fills_intent() {
        let mut app = App::new();
        app.add_plugins(InputPlugin)
            .init_resource::<ButtonInput<KeyCode>>();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyD);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Space);
        app.update();
        let intent = *app.world().resource::<Intent>();
        assert_eq!(intent.dir, V2::new(1.0, 0.0));
        assert!(intent.confirm);
    }
}
