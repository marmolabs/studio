//! Pixel maps. Each row is a string; each character is a palette key and
//! `.` is empty. `draw` turns a map into SVG `<rect>`s, one per run of
//! equal pixels.

use std::fmt::Write;

/// Fixed colours. Person colours (`h` hair, `s` skin, `t`/`d` shirt) come
/// from `Look` instead.
pub fn color(key: char) -> Option<&'static str> {
    Some(match key {
        'k' => "#2b2118",
        'w' => "#ffffff",
        'g' => "#aab2b7",
        'G' => "#646c72",
        'b' => "#6b4a2b",
        'B' => "#9a6b3c",
        'l' => "#c18f56",
        'n' => "#3f8f45",
        'N' => "#2b6a33",
        'L' => "#7fc46c",
        'r' => "#d65a4a",
        'R' => "#93392f",
        'y' => "#f2c14e",
        'Y' => "#c98f25",
        'u' => "#2f5f8f",
        'U' => "#9fd8f5",
        'o' => "#ee9b3a",
        'O' => "#bd6a22",
        'c' => "#efe3ca",
        'p' => "#e58fa8",
        'T' => "#3f8f8b",
        'X' => "#2a625f",
        'P' => "#3d4a6b",
        'S' => "#25252e",
        'K' => "#111111",
        _ => return None,
    })
}

/// Draws `rows` with its top-left corner at `(ox, oy)`. `paint` may swap a
/// key's colour; it falls back to the fixed table.
pub fn draw(rows: &[&str], ox: i32, oy: i32, paint: &dyn Fn(char) -> Option<String>) -> String {
    let mut out = String::new();
    for (y, row) in rows.iter().enumerate() {
        let cells: Vec<char> = row.chars().collect();
        let mut x = 0;
        while x < cells.len() {
            let key = cells[x];
            let mut end = x + 1;
            while end < cells.len() && cells[end] == key {
                end += 1;
            }
            let fill = paint(key).or_else(|| color(key).map(str::to_owned));
            if let Some(fill) = fill {
                let _ = write!(
                    out,
                    r#"<rect x="{}" y="{}" width="{}" height="1" fill="{fill}"/>"#,
                    ox + x as i32,
                    oy + y as i32,
                    end - x
                );
            }
            x = end;
        }
    }
    out
}

/// `draw` with the fixed colours only.
pub fn plain(rows: &[&str], ox: i32, oy: i32) -> String {
    draw(rows, ox, oy, &|_| None)
}

/// A solid rectangle.
pub fn block(x: i32, y: i32, w: i32, h: i32, fill: &str) -> String {
    format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="{fill}"/>"#)
}

/// 32x16. The monitor screen is `m`; its colour says whether it is on.
pub const DESK: &[&str] = &[
    "................................",
    "...........kkkkkkkkkk...........",
    "...........kmmmmmmmmk...........",
    "...........kmmmmmmmmk...........",
    "...........kmmmmmmmmk...........",
    "...........kmmmmmmmmk...........",
    "...........kkkkkkkkkk...........",
    "..............kkkk..............",
    "llllllllllllllllllllllllllllllll",
    "lBBBBBBBBBBBBBBBBBBBBBBBBrrBBBBl",
    "lBBBBBBBBBBkgwgwgwgwkBBBBRRBBBBl",
    "bBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBb",
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    ".bb..........................bb.",
    ".bb..........................bb.",
    ".bb..........................bb.",
];

/// 12x7, seen from behind.
pub const CHAIR: &[&str] = &[
    "..XXXXXXXX..",
    ".XTTTTTTTTX.",
    ".XTTTTTTTTX.",
    ".XTTTTTTTTX.",
    ".XXXXXXXXXX.",
    "....kkkk....",
    "..kk.kk.kk..",
];

/// 16x16.
pub const PLANT: &[&str] = &[
    "................",
    "......nn........",
    "....nnLLn.nn....",
    "...nLLnnLnLLn...",
    "..nLLnNnLLnNn...",
    "...nNnLnnLnn....",
    "..nLnnNnnNLLn...",
    "...nNLnnLnnN....",
    ".....nnNnn......",
    "....kkkkkkkk....",
    "....koooooook...",
    "....koooooook...",
    ".....kOOOOk.....",
    ".....kOOOOk.....",
    "......kkkk......",
    "................",
];

/// 16x16.
pub const COFFEE: &[&str] = &[
    "................",
    "...kkkkkkkkk....",
    "...kGGGGGGGk....",
    "...kGrGGGggk....",
    "...kGGGGGGGk....",
    "...kkkkkkkkk....",
    "...kGGkkkGGk....",
    "...kG..B..Gk....",
    "...kG.kkk.Gk....",
    "...kG.kBk.Gk....",
    "...kG.kkk.Gk....",
    "...kGGGGGGGk....",
    "...kkkkkkkkk....",
    "................",
    "................",
    "................",
];

/// 10x8, facing right.
pub const CAT: &[&str] = &[
    "......o.o.",
    ".....ooooo",
    ".....oKoKo",
    "o...ooooop",
    "oooooooooo",
    ".oooooooo.",
    ".oO....oO.",
    ".oo....oo.",
];

/// 10x14, seen from behind, sitting at a desk (only the top 12 rows show).
pub const BACK: &[&str] = &[
    "...hhhh...",
    "..hhhhhh..",
    ".hhhhhhhh.",
    ".hhhhhhhh.",
    ".hhhhhhhh.",
    "..hhhhhh..",
    "...ssss...",
    ".tttttttt.",
    "tttttttttt",
    "tttttttttt",
    "dttttttttd",
    "dttttttttd",
];

/// 10x14, facing us.
pub const FRONT: &[&str] = &[
    "...hhhh...",
    "..hhhhhh..",
    ".hhhhhhhh.",
    ".hssssssh.",
    ".hsossosh.",
    "..ssssss..",
    ".tttttttt.",
    "tttttttttt",
    "tttttttttt",
    "stttttttts",
    "..PPPPPP..",
    "..PPPPPP..",
    "..PP..PP..",
    "..SS..SS..",
];

/// 7x7 magnifier for the reading bubble.
pub const LENS: &[&str] = &[
    ".kkk...", "kUUUk..", "kUwUk..", "kUUUk..", ".kkkk..", "....kk.", ".....kk",
];

const SHIRTS: [(&str, &str); 8] = [
    ("#d65a4a", "#a8402f"),
    ("#4a8fd6", "#2f6aa8"),
    ("#58b368", "#3a8749"),
    ("#e0b13c", "#b88a1f"),
    ("#9b6bd6", "#744aa8"),
    ("#e07aa5", "#b85580"),
    ("#4cc0b8", "#2f948d"),
    ("#e8844a", "#b9622b"),
];
const HAIRS: [&str; 6] = [
    "#2b2118", "#5a3a22", "#8a5a2b", "#d9b36a", "#b0452f", "#9aa0a6",
];
const SKINS: [&str; 4] = ["#f1c9a0", "#e0a97a", "#b98056", "#8a5a3c"];

/// What one hire looks like, from a hash of their name.
pub struct Look {
    shirt: (&'static str, &'static str),
    hair: &'static str,
    skin: &'static str,
}

impl Look {
    pub fn of(name: &str) -> Look {
        // FNV-1a: stable, so a hire keeps their look between renders.
        let mut hash: u32 = 0x811c_9dc5;
        for byte in name.bytes() {
            hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
        }
        Look {
            shirt: SHIRTS[(hash % 8) as usize],
            hair: HAIRS[((hash >> 8) % 6) as usize],
            skin: SKINS[((hash >> 16) % 4) as usize],
        }
    }

    /// Draws a person map, keeping only its first `rows` rows.
    pub fn draw(&self, map: &[&str], rows: usize, ox: i32, oy: i32) -> String {
        let paint = |key: char| {
            Some(
                match key {
                    't' => self.shirt.0,
                    'd' => self.shirt.1,
                    'h' => self.hair,
                    's' => self.skin,
                    _ => return None,
                }
                .to_owned(),
            )
        };
        draw(&map[..rows.min(map.len())], ox, oy, &paint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_map_is_a_clean_rectangle() {
        let maps: [(&str, &[&str], usize); 9] = [
            ("desk", DESK, 32),
            ("chair", CHAIR, 12),
            ("plant", PLANT, 16),
            ("coffee", COFFEE, 16),
            ("cat", CAT, 10),
            ("back", BACK, 10),
            ("front", FRONT, 10),
            ("lens", LENS, 7),
            ("lens2", LENS, 7),
        ];
        for (name, rows, width) in maps {
            for (i, row) in rows.iter().enumerate() {
                assert_eq!(row.chars().count(), width, "{name} row {i}: {row}");
            }
        }
    }

    #[test]
    fn looks_are_stable_and_vary() {
        let a = Look::of("engineer");
        assert_eq!(a.shirt, Look::of("engineer").shirt);
        let distinct: std::collections::HashSet<_> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|n| Look::of(n).shirt.0)
            .collect();
        assert!(distinct.len() > 2);
    }

    #[test]
    fn runs_become_one_rect() {
        assert_eq!(plain(&["kk.k"], 0, 0).matches("<rect").count(), 2);
    }
}
