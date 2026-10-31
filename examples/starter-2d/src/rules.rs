//! Pure game rules for Lantern. No Bevy types live here: everything is plain
//! math on plain data, driven by an injected seeded RNG, so it is trivially
//! unit-testable and deterministic.

use std::ops::{Add, Mul, Sub};

// --- Tuning constants -------------------------------------------------------

/// Half the arena width, in world units.
pub const ARENA_HALF_W: f32 = 400.0;
/// Half the arena height, in world units.
pub const ARENA_HALF_H: f32 = 300.0;
/// How far outside the arena things spawn / are despawned.
pub const SPAWN_MARGIN: f32 = 40.0;

pub const PLAYER_SPEED: f32 = 260.0;
pub const PLAYER_RADIUS: f32 = 10.0;
pub const SPARK_RADIUS: f32 = 7.0;
pub const MOTH_RADIUS: f32 = 9.0;

pub const LIGHT_MAX: f32 = 100.0;
pub const LIGHT_START: f32 = 60.0;
/// Light lost per second at the start of a run.
pub const DECAY_BASE: f32 = 4.0;
/// Extra decay per second for every second survived.
pub const DECAY_RAMP: f32 = 0.04;
pub const SPARK_GAIN: f32 = 14.0;
pub const MOTH_DRAIN: f32 = 18.0;
pub const RADIUS_MIN: f32 = 40.0;
pub const RADIUS_MAX: f32 = 220.0;

pub const SPARK_INTERVAL: f32 = 1.2;
pub const MOTH_INTERVAL_START: f32 = 6.0;
pub const MOTH_INTERVAL_MIN: f32 = 2.0;
pub const MOTH_SPEED_START: f32 = 70.0;
pub const MOTH_SPEED_MAX: f32 = 140.0;

// --- Math -------------------------------------------------------------------

/// A minimal 2D vector.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct V2 {
    pub x: f32,
    pub y: f32,
}

impl V2 {
    pub const ZERO: V2 = V2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }

    pub fn distance(self, other: V2) -> f32 {
        (other - self).length()
    }

    /// Unit vector in the same direction, or zero for a zero-length vector.
    pub fn normalized_or_zero(self) -> V2 {
        let len = self.length();
        if len > f32::EPSILON {
            self * (1.0 / len)
        } else {
            V2::ZERO
        }
    }
}

impl Add for V2 {
    type Output = V2;
    fn add(self, o: V2) -> V2 {
        V2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for V2 {
    type Output = V2;
    fn sub(self, o: V2) -> V2 {
        V2::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f32> for V2 {
    type Output = V2;
    fn mul(self, k: f32) -> V2 {
        V2::new(self.x * k, self.y * k)
    }
}

/// True when two circles overlap (touching counts).
pub fn circles_overlap(a: V2, ar: f32, b: V2, br: f32) -> bool {
    a.distance(b) <= ar + br
}

/// Move the player by `dir` (any length; normalized so diagonals are not
/// faster) and keep them inside the arena.
pub fn step_player(pos: V2, dir: V2, dt: f32) -> V2 {
    let p = pos + dir.normalized_or_zero() * (PLAYER_SPEED * dt);
    V2::new(
        p.x.clamp(-ARENA_HALF_W + PLAYER_RADIUS, ARENA_HALF_W - PLAYER_RADIUS),
        p.y.clamp(-ARENA_HALF_H + PLAYER_RADIUS, ARENA_HALF_H - PLAYER_RADIUS),
    )
}

/// Move a moth toward the lantern, never overshooting it.
pub fn step_moth(pos: V2, target: V2, speed: f32, dt: f32) -> V2 {
    let to = target - pos;
    let step = speed * dt;
    if to.length() <= step {
        target
    } else {
        pos + to.normalized_or_zero() * step
    }
}

/// Moths get faster the longer the run lasts.
pub fn moth_speed(elapsed: f32) -> f32 {
    (MOTH_SPEED_START + elapsed * 0.8).min(MOTH_SPEED_MAX)
}

/// True once a point is far enough outside the arena to be discarded.
pub fn is_far_outside(pos: V2) -> bool {
    pos.x.abs() > ARENA_HALF_W + SPAWN_MARGIN * 2.0
        || pos.y.abs() > ARENA_HALF_H + SPAWN_MARGIN * 2.0
}

// --- RNG --------------------------------------------------------------------

/// Tiny deterministic xorshift64* generator.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        // xorshift must never hold an all-zero state.
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
}

// --- Run state --------------------------------------------------------------

/// The state of one run: light, score and time survived.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub light: f32,
    pub score: u32,
    pub elapsed: f32,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            light: LIGHT_START,
            score: 0,
            elapsed: 0.0,
        }
    }
}

impl Run {
    /// Advance time: the light decays, faster the longer you survive.
    pub fn tick(&mut self, dt: f32) {
        self.light = (self.light - (DECAY_BASE + DECAY_RAMP * self.elapsed) * dt).max(0.0);
        self.elapsed += dt;
    }

    pub fn collect_spark(&mut self) {
        self.score += 1;
        self.light = (self.light + SPARK_GAIN).min(LIGHT_MAX);
    }

    pub fn hit_by_moth(&mut self) {
        self.light = (self.light - MOTH_DRAIN).max(0.0);
    }

    pub fn is_over(&self) -> bool {
        self.light <= 0.0
    }

    /// Light level as a fraction in `[0, 1]`.
    pub fn light_fraction(&self) -> f32 {
        (self.light / LIGHT_MAX).clamp(0.0, 1.0)
    }

    /// Radius of the lit area around the lantern.
    pub fn light_radius(&self) -> f32 {
        RADIUS_MIN + (RADIUS_MAX - RADIUS_MIN) * self.light_fraction()
    }
}

/// Best score of the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Best(pub u32);

impl Best {
    /// Record a finished run; returns true if it set a new best.
    pub fn record(&mut self, score: u32) -> bool {
        if score > self.0 {
            self.0 = score;
            true
        } else {
            false
        }
    }
}

// --- Phases -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Title,
    Playing,
    GameOver,
}

/// The one place that decides how the game moves between phases.
pub fn next_phase(phase: Phase, confirm: bool, light_out: bool) -> Phase {
    match phase {
        Phase::Title | Phase::GameOver if confirm => Phase::Playing,
        Phase::Playing if light_out => Phase::GameOver,
        other => other,
    }
}

// --- Spawning ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnKind {
    Spark,
    Moth,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spawn {
    pub kind: SpawnKind,
    pub pos: V2,
    /// Drift velocity (zero for moths, which steer toward the lantern).
    pub vel: V2,
}

/// Seconds between moths at a given point in the run.
pub fn moth_interval(elapsed: f32) -> f32 {
    (MOTH_INTERVAL_START - elapsed * 0.05).max(MOTH_INTERVAL_MIN)
}

/// Countdown timers that decide when sparks and moths appear.
#[derive(Debug, Clone, PartialEq)]
pub struct Spawner {
    spark_in: f32,
    moth_in: f32,
}

impl Default for Spawner {
    fn default() -> Self {
        Self {
            spark_in: 0.5,
            moth_in: MOTH_INTERVAL_START,
        }
    }
}

impl Spawner {
    /// Advance the timers by `dt` and return everything that should spawn.
    pub fn tick(&mut self, dt: f32, elapsed: f32, rng: &mut Rng) -> Vec<Spawn> {
        let mut out = Vec::new();
        self.spark_in -= dt;
        while self.spark_in <= 0.0 {
            let pos = edge_position(rng);
            let target = V2::new(
                rng.range(-ARENA_HALF_W * 0.7, ARENA_HALF_W * 0.7),
                rng.range(-ARENA_HALF_H * 0.7, ARENA_HALF_H * 0.7),
            );
            let vel = (target - pos).normalized_or_zero() * rng.range(40.0, 80.0);
            out.push(Spawn {
                kind: SpawnKind::Spark,
                pos,
                vel,
            });
            self.spark_in += SPARK_INTERVAL * rng.range(0.6, 1.4);
        }
        self.moth_in -= dt;
        while self.moth_in <= 0.0 {
            out.push(Spawn {
                kind: SpawnKind::Moth,
                pos: edge_position(rng),
                vel: V2::ZERO,
            });
            self.moth_in += moth_interval(elapsed);
        }
        out
    }
}

/// A random point just outside one of the four arena edges.
pub fn edge_position(rng: &mut Rng) -> V2 {
    let w = ARENA_HALF_W + SPAWN_MARGIN;
    let h = ARENA_HALF_H + SPAWN_MARGIN;
    let along_x = rng.range(-w, w);
    let along_y = rng.range(-h, h);
    match (rng.next_f32() * 4.0) as u32 {
        0 => V2::new(along_x, h),
        1 => V2::new(along_x, -h),
        2 => V2::new(w, along_y),
        _ => V2::new(-w, along_y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..100 {
            let x = a.next_f32();
            assert_eq!(x, b.next_f32());
            assert!((0.0..1.0).contains(&x));
        }
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
    }

    #[test]
    fn rng_zero_seed_still_works() {
        let mut r = Rng::new(0);
        assert_ne!(r.next_u64(), 0);
    }

    #[test]
    fn rng_range_respects_bounds() {
        let mut r = Rng::new(3);
        for _ in 0..200 {
            let v = r.range(5.0, 9.0);
            assert!((5.0..9.0).contains(&v));
        }
    }

    #[test]
    fn vector_math() {
        let v = V2::new(3.0, 4.0);
        assert!((v.length() - 5.0).abs() < EPS);
        assert!((v.normalized_or_zero().length() - 1.0).abs() < EPS);
        assert_eq!(V2::ZERO.normalized_or_zero(), V2::ZERO);
        assert!((V2::ZERO.distance(v) - 5.0).abs() < EPS);
    }

    #[test]
    fn overlap_includes_touching_and_excludes_apart() {
        assert!(circles_overlap(V2::ZERO, 5.0, V2::new(10.0, 0.0), 5.0));
        assert!(circles_overlap(V2::ZERO, 5.0, V2::new(3.0, 0.0), 1.0));
        assert!(!circles_overlap(V2::ZERO, 5.0, V2::new(10.1, 0.0), 5.0));
    }

    #[test]
    fn player_moves_and_diagonals_are_not_faster() {
        let straight = step_player(V2::ZERO, V2::new(1.0, 0.0), 0.1);
        let diagonal = step_player(V2::ZERO, V2::new(1.0, 1.0), 0.1);
        assert!((straight.length() - PLAYER_SPEED * 0.1).abs() < EPS);
        assert!((diagonal.length() - PLAYER_SPEED * 0.1).abs() < EPS);
        assert_eq!(step_player(V2::ZERO, V2::ZERO, 0.1), V2::ZERO);
    }

    #[test]
    fn player_is_clamped_to_arena() {
        let p = step_player(V2::new(ARENA_HALF_W, ARENA_HALF_H), V2::new(1.0, 1.0), 1.0);
        assert!((p.x - (ARENA_HALF_W - PLAYER_RADIUS)).abs() < EPS);
        assert!((p.y - (ARENA_HALF_H - PLAYER_RADIUS)).abs() < EPS);
        let q = step_player(
            V2::new(-ARENA_HALF_W, -ARENA_HALF_H),
            V2::new(-1.0, -1.0),
            1.0,
        );
        assert!((q.x + (ARENA_HALF_W - PLAYER_RADIUS)).abs() < EPS);
        assert!((q.y + (ARENA_HALF_H - PLAYER_RADIUS)).abs() < EPS);
    }

    #[test]
    fn moth_approaches_and_never_overshoots() {
        let target = V2::new(100.0, 0.0);
        let p = step_moth(V2::ZERO, target, 50.0, 1.0);
        assert!((p.x - 50.0).abs() < EPS);
        assert_eq!(step_moth(V2::new(99.0, 0.0), target, 50.0, 1.0), target);
    }

    #[test]
    fn moth_speed_ramps_and_caps() {
        assert_eq!(moth_speed(0.0), MOTH_SPEED_START);
        assert!(moth_speed(10.0) > moth_speed(0.0));
        assert_eq!(moth_speed(10_000.0), MOTH_SPEED_MAX);
    }

    #[test]
    fn far_outside_detection() {
        assert!(!is_far_outside(V2::new(ARENA_HALF_W + SPAWN_MARGIN, 0.0)));
        assert!(is_far_outside(V2::new(
            ARENA_HALF_W + SPAWN_MARGIN * 3.0,
            0.0
        )));
        assert!(is_far_outside(V2::new(
            0.0,
            -ARENA_HALF_H - SPAWN_MARGIN * 3.0
        )));
    }

    #[test]
    fn light_decays_faster_over_time() {
        let mut early = Run::default();
        early.tick(1.0);
        assert!((early.light - (LIGHT_START - DECAY_BASE)).abs() < EPS);

        let mut late = Run {
            elapsed: 100.0,
            ..Run::default()
        };
        late.tick(1.0);
        assert!(LIGHT_START - late.light > DECAY_BASE);
    }

    #[test]
    fn light_never_goes_negative_and_ends_run() {
        let mut run = Run::default();
        assert!(!run.is_over());
        run.tick(1000.0);
        assert_eq!(run.light, 0.0);
        assert!(run.is_over());
    }

    #[test]
    fn spark_scores_and_is_capped() {
        let mut run = Run::default();
        run.collect_spark();
        assert_eq!(run.score, 1);
        assert!((run.light - (LIGHT_START + SPARK_GAIN)).abs() < EPS);
        for _ in 0..20 {
            run.collect_spark();
        }
        assert_eq!(run.light, LIGHT_MAX);
        assert_eq!(run.score, 21);
    }

    #[test]
    fn moth_drains_without_going_negative() {
        let mut run = Run::default();
        run.hit_by_moth();
        assert!((run.light - (LIGHT_START - MOTH_DRAIN)).abs() < EPS);
        for _ in 0..10 {
            run.hit_by_moth();
        }
        assert_eq!(run.light, 0.0);
        assert!(run.is_over());
        assert_eq!(run.score, 0);
    }

    #[test]
    fn light_radius_tracks_light() {
        let mut run = Run {
            light: 0.0,
            ..Run::default()
        };
        assert_eq!(run.light_radius(), RADIUS_MIN);
        run.light = LIGHT_MAX;
        assert_eq!(run.light_radius(), RADIUS_MAX);
        run.light = LIGHT_MAX / 2.0;
        assert!((run.light_radius() - (RADIUS_MIN + RADIUS_MAX) / 2.0).abs() < EPS);
    }

    #[test]
    fn best_only_updates_on_improvement() {
        let mut best = Best::default();
        assert!(best.record(3));
        assert!(!best.record(3));
        assert!(!best.record(1));
        assert_eq!(best.0, 3);
        assert!(best.record(4));
        assert_eq!(best, Best(4));
    }

    #[test]
    fn phase_transitions() {
        use Phase::*;
        assert_eq!(next_phase(Title, false, false), Title);
        assert_eq!(next_phase(Title, true, false), Playing);
        assert_eq!(next_phase(Playing, true, false), Playing);
        assert_eq!(next_phase(Playing, false, true), GameOver);
        assert_eq!(next_phase(GameOver, false, true), GameOver);
        assert_eq!(next_phase(GameOver, true, true), Playing);
        // A held confirm key must not skip the title when the light is out.
        assert_eq!(next_phase(Title, true, true), Playing);
    }

    #[test]
    fn moth_interval_tightens_to_a_floor() {
        assert_eq!(moth_interval(0.0), MOTH_INTERVAL_START);
        assert!(moth_interval(30.0) < moth_interval(0.0));
        assert_eq!(moth_interval(10_000.0), MOTH_INTERVAL_MIN);
    }

    #[test]
    fn spawner_waits_then_spawns_sparks() {
        let mut rng = Rng::new(1);
        let mut sp = Spawner::default();
        assert!(sp.tick(0.1, 0.0, &mut rng).is_empty());
        let spawned = sp.tick(0.5, 0.0, &mut rng);
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].kind, SpawnKind::Spark);
    }

    #[test]
    fn spawner_emits_moths_on_schedule() {
        let mut rng = Rng::new(1);
        let mut sp = Spawner::default();
        let spawned = sp.tick(MOTH_INTERVAL_START + 0.01, 0.0, &mut rng);
        assert_eq!(
            spawned.iter().filter(|s| s.kind == SpawnKind::Moth).count(),
            1
        );
    }

    #[test]
    fn spawner_is_deterministic_for_a_seed() {
        let run = |seed| {
            let mut rng = Rng::new(seed);
            let mut sp = Spawner::default();
            (0..300)
                .flat_map(|i| sp.tick(0.1, i as f32 * 0.1, &mut rng))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(42), run(42));
        assert_ne!(run(42), run(43));
    }

    #[test]
    fn spawns_start_outside_and_sparks_drift_inward() {
        let mut rng = Rng::new(5);
        let mut sp = Spawner::default();
        let all: Vec<_> = (0..600)
            .flat_map(|_| sp.tick(0.1, 30.0, &mut rng))
            .collect();
        assert!(!all.is_empty());
        for s in &all {
            let outside = s.pos.x.abs() >= ARENA_HALF_W || s.pos.y.abs() >= ARENA_HALF_H;
            assert!(outside, "{s:?} should start outside the arena");
            assert!(!is_far_outside(s.pos));
            if s.kind == SpawnKind::Spark {
                // Moving toward the centre: velocity opposes position.
                assert!(s.vel.x * s.pos.x + s.vel.y * s.pos.y < 0.0);
            }
        }
        assert!(all.iter().any(|s| s.kind == SpawnKind::Moth));
    }
}
