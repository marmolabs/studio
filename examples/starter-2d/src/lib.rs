//! Lantern: a tiny 2D Bevy game. Game rules live in [`rules`] (no Bevy types);
//! each other module is a Bevy plugin that wires those rules into the engine.

pub mod capture;
pub mod convert;
pub mod hud;
pub mod input;
pub mod player;
pub mod rules;
pub mod spawning;
pub mod states;

use bevy::prelude::*;
use bevy::window::WindowResolution;

/// Colors shared by gameplay and UI.
pub mod palette {
    use bevy::prelude::Color;

    pub const BACKGROUND: Color = Color::srgb(0.03, 0.04, 0.09);
    pub const HALO: Color = Color::srgba(1.0, 0.85, 0.45, 0.05);
    pub const PLAYER: Color = Color::srgb(1.0, 0.92, 0.6);
    pub const SPARK: Color = Color::srgb(0.4, 0.95, 1.0);
    pub const MOTH: Color = Color::srgb(0.75, 0.45, 0.95);
    pub const TEXT: Color = Color::srgb(0.92, 0.92, 0.85);
    pub const METER_BACK: Color = Color::srgb(0.15, 0.16, 0.22);
    pub const METER_FILL: Color = Color::srgb(1.0, 0.8, 0.3);
}

/// Window width in pixels (matches the arena).
pub const WINDOW_W: u32 = (rules::ARENA_HALF_W * 2.0) as u32;
/// Window height in pixels (matches the arena).
pub const WINDOW_H: u32 = (rules::ARENA_HALF_H * 2.0) as u32;

/// Build the full game app. `seed` drives all randomness.
pub fn build_app(seed: u64) -> App {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Lantern".into(),
            resolution: WindowResolution::new(WINDOW_W, WINDOW_H),
            resizable: false,
            ..default()
        }),
        ..default()
    }))
    .insert_resource(ClearColor(palette::BACKGROUND))
    .add_systems(Startup, |mut commands: Commands| {
        commands.spawn(Camera2d);
    })
    .add_plugins((
        input::InputPlugin,
        states::GameStatePlugin,
        player::PlayerPlugin,
        spawning::SpawningPlugin { seed },
        hud::HudPlugin,
    ));
    app
}
