//! `GameState` and the run lifecycle: title -> playing -> game over.

use crate::input::Intent;
use crate::rules::{self, Best, Phase, Run};
use bevy::prelude::*;

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GameState {
    #[default]
    Title,
    Playing,
    GameOver,
}

impl From<GameState> for Phase {
    fn from(s: GameState) -> Self {
        match s {
            GameState::Title => Phase::Title,
            GameState::Playing => Phase::Playing,
            GameState::GameOver => Phase::GameOver,
        }
    }
}

impl From<Phase> for GameState {
    fn from(p: Phase) -> Self {
        match p {
            Phase::Title => GameState::Title,
            Phase::Playing => GameState::Playing,
            Phase::GameOver => GameState::GameOver,
        }
    }
}

/// The current run (light, score, time) as a resource.
#[derive(Resource, Default, Deref, DerefMut)]
pub struct CurrentRun(pub Run);

/// Best score this session.
#[derive(Resource, Default, Deref, DerefMut)]
pub struct BestScore(pub Best);

pub struct GameStatePlugin;

impl Plugin for GameStatePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<GameState>()
            .init_resource::<CurrentRun>()
            .init_resource::<BestScore>()
            .add_systems(OnEnter(GameState::Playing), reset_run)
            .add_systems(
                Update,
                (tick_run.run_if(in_state(GameState::Playing)), advance_state).chain(),
            );
    }
}

fn reset_run(mut run: ResMut<CurrentRun>) {
    run.0 = Run::default();
}

fn tick_run(time: Res<Time>, mut run: ResMut<CurrentRun>) {
    run.tick(time.delta_secs());
}

fn advance_state(
    state: Res<State<GameState>>,
    intent: Res<Intent>,
    mut run: ResMut<CurrentRun>,
    mut best: ResMut<BestScore>,
    mut next: ResMut<NextState<GameState>>,
) {
    let current = *state.get();
    let target = rules::next_phase(current.into(), intent.confirm, run.is_over());
    let target = GameState::from(target);
    if target == current {
        return;
    }
    if current == GameState::Playing && target == GameState::GameOver {
        let score = run.score;
        best.record(score);
    }
    if target == GameState::Playing {
        // Clear the old run right away so a stale "light out" cannot end the
        // new run before `reset_run` has executed.
        run.0 = Run::default();
    }
    next.set(target);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::StatesPlugin;

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin))
            .init_resource::<Intent>()
            .add_plugins(GameStatePlugin);
        app.update();
        app
    }

    fn state(app: &App) -> GameState {
        *app.world().resource::<State<GameState>>().get()
    }

    fn press_confirm(app: &mut App) {
        app.world_mut().resource_mut::<Intent>().confirm = true;
        app.update();
        app.world_mut().resource_mut::<Intent>().confirm = false;
        app.update();
    }

    #[test]
    fn starts_on_title_and_space_starts_play() {
        let mut app = test_app();
        assert_eq!(state(&app), GameState::Title);
        app.update();
        assert_eq!(state(&app), GameState::Title);
        press_confirm(&mut app);
        assert_eq!(state(&app), GameState::Playing);
    }

    #[test]
    fn light_out_ends_run_and_records_best() {
        let mut app = test_app();
        press_confirm(&mut app);
        {
            let mut run = app.world_mut().resource_mut::<CurrentRun>();
            run.score = 5;
            run.light = 0.0;
        }
        app.update();
        app.update();
        assert_eq!(state(&app), GameState::GameOver);
        assert_eq!(app.world().resource::<BestScore>().0, Best(5));
    }

    #[test]
    fn restart_resets_run_but_keeps_best() {
        let mut app = test_app();
        press_confirm(&mut app);
        {
            let mut run = app.world_mut().resource_mut::<CurrentRun>();
            run.score = 2;
            run.light = 0.0;
        }
        app.update();
        app.update();
        assert_eq!(state(&app), GameState::GameOver);
        press_confirm(&mut app);
        assert_eq!(state(&app), GameState::Playing);
        let run = app.world().resource::<CurrentRun>();
        assert_eq!(run.score, 0);
        assert!(run.light > 0.0);
        assert_eq!(app.world().resource::<BestScore>().0, Best(2));
    }

    #[test]
    fn state_conversion_round_trips() {
        for s in [GameState::Title, GameState::Playing, GameState::GameOver] {
            assert_eq!(GameState::from(Phase::from(s)), s);
        }
    }
}
