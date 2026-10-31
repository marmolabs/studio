//! Conversions between Bevy math types and the engine-free types in `rules`.

use crate::rules::V2;
use bevy::prelude::{Vec2, Vec3};

impl From<Vec2> for V2 {
    fn from(v: Vec2) -> Self {
        V2::new(v.x, v.y)
    }
}

impl From<V2> for Vec2 {
    fn from(v: V2) -> Self {
        Vec2::new(v.x, v.y)
    }
}

/// Read a transform translation as a rules vector.
pub fn xy(v: Vec3) -> V2 {
    V2::new(v.x, v.y)
}
