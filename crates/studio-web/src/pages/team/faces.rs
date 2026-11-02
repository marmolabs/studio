//! A person's portrait, round: a head and shoulders as an inline drawing, in
//! the colours the floor's figures have. The head fills about half the circle
//! and the shoulders the lower third, so a face still reads at 24 pixels. The
//! skin and the hair tone come from `iso::looks` with the hire's id as the
//! seed, the shirt from the role (a lead's is its project's colour), and the
//! hair style (short, long or in a bun) from the same seed. The owner wears a
//! dark jacket and a white collar.

use topcoat::view::{View, component, view};

use crate::pages::office::iso::looks;
use crate::pages::office::model::Role;

/// How a person wears their hair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hair {
    Short,
    Long,
    Bun,
}

impl Hair {
    /// The style as a class name.
    fn class(self) -> &'static str {
        match self {
            Hair::Short => "hs-short",
            Hair::Long => "hs-long",
            Hair::Bun => "hs-bun",
        }
    }
}

/// How a hire looks: the skin and hair tones the floor gives its figure, and
/// the hair style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Look {
    skin: char,
    tone: char,
    style: Hair,
}

/// The look the seed gives: the same seed always gives the same one.
fn look(seed: &str) -> Look {
    let (skin, tone) = looks(seed);
    // Mixed well, so short names that look alike still get different styles.
    let h = seed.bytes().fold(2_166_136_261u32, |a, b| {
        (a ^ u32::from(b)).wrapping_mul(16_777_619)
    });
    let style = match ((h ^ h >> 16).wrapping_mul(0x045d_9f3b) >> 11) % 3 {
        0 => Hair::Short,
        1 => Hair::Long,
        _ => Hair::Bun,
    };
    Look { skin, tone, style }
}

// The drawing is 40 by 40 units. The head is centred at (20, 15) with a radius
// of 10; the shoulders start at 27 and the circle cuts their corners.

/// Hair that falls behind the head and over the shoulders.
const LONG_HAIR: &str = "M9 15a11 11 0 0 1 22 0v17.2c0 1.1-.8 1.8-1.9 1.8h-2.4c-1 0-1.7-.7-1.7-1.7V20h-9.2v12.3c0 1-.7 1.7-1.7 1.7h-2.4c-1.1 0-1.9-.7-1.9-1.8z";
/// The head's cap of hair, on every style.
const CAP: &str = "M10 15.4A10 10 0 0 1 30 15.4C28.4 12 24.8 10.1 20 10.1S11.6 12 10 15.4z";
/// A light streak on the hair, so dark hair reads as hair.
const SHEEN: &str = "M13.6 9.6c2-1.2 4-1.6 6-1.4-2.3.5-4.3 1.5-5.8 3.1z";
const SHOULDERS: &str = "M0 41c0-9.6 7-14 20-14s20 4.4 20 14z";
const COLLAR: &str = "M15 27.6 20 34.4l5-6.8z";

/// The face of the hire `seed`, `size` pixels across (24, 30, 40 or 48). `role`
/// gives the shirt its colour, except a lead's, which is the colour of its
/// project, `tint`.
#[component]
pub async fn face(
    #[into] seed: String,
    role: Role,
    tint: usize,
    size: u32,
) -> topcoat::Result<impl View> {
    let Look { skin, tone, style } = look(&seed);
    let class = format!(
        "fc {} s-{skin} h-{tone} {} pj-{tint}",
        role.class(),
        style.class()
    );
    let owner = role == Role::Owner;
    let long = style == Hair::Long;
    let bun = style == Hair::Bun;
    Ok(view! {
        <svg class=(class) viewBox="0 0 40 40" width=(size) height=(size) data-face=(seed) data-role=(role.class()) aria-hidden="true">
            if bun { <circle class="hr" cx="20" cy="4.9" r="4.1"></circle> }
            <path class="sh" d=(SHOULDERS)></path>
            if long { <path class="hr" d=(LONG_HAIR)></path> }
            if owner { <path class="cl" d=(COLLAR)></path> }
            <rect class="nk" x="16.2" y="20" width="7.6" height="10" rx="3.4"></rect>
            <circle class="sk" cx="10.1" cy="16.7" r="2"></circle>
            <circle class="sk" cx="29.9" cy="16.7" r="2"></circle>
            <circle class="sk" cx="20" cy="15" r="10"></circle>
            <path class="hr" d=(CAP)></path>
            <path class="hl" d=(SHEEN)></path>
            <circle class="ey" cx="15.6" cy="16.2" r="1.45"></circle>
            <circle class="ey" cx="24.4" cy="16.2" r="1.45"></circle>
            <path class="mo" d="M16.8 20.3q3.2 2.5 6.4 0"></path>
        </svg>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const SEEDS: [&str; 12] = [
        "nora", "sam", "ines", "theo", "dev", "writer", "mira", "kit", "leo", "ada", "bo", "zed",
    ];

    #[test]
    fn the_same_seed_always_gives_the_same_face() {
        for seed in SEEDS {
            assert_eq!(look(seed), look(seed), "{seed}");
        }
    }

    #[test]
    fn a_face_wears_the_skin_and_hair_tone_its_figure_on_the_floor_has() {
        for seed in SEEDS {
            let Look { skin, tone, .. } = look(seed);
            assert_eq!((skin, tone), looks(seed), "{seed}");
        }
    }

    #[test]
    fn different_seeds_give_visibly_different_faces() {
        let faces: Vec<Look> = SEEDS.iter().map(|seed| look(seed)).collect();
        // All three hair styles and most hair tones turn up.
        for style in [Hair::Short, Hair::Long, Hair::Bun] {
            assert!(faces.iter().any(|f| f.style == style), "{style:?}");
        }
        let tones: BTreeSet<char> = faces.iter().map(|f| f.tone).collect();
        assert!(tones.len() >= 3, "{tones:?}");
        // Few people share a style and a hair tone.
        let mut seen = BTreeSet::new();
        let alike = faces
            .iter()
            .filter(|f| !seen.insert((f.style.class(), f.tone)))
            .count();
        assert!(
            alike <= 4,
            "{alike} of {} share style and hair",
            faces.len()
        );
    }
}
