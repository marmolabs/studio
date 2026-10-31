//! Where things stand in the office, kept in `<data_dir>/office.json`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const WIDTH: u32 = 20;
pub const HEIGHT: u32 = 12;
/// Rows taken by the back wall; people never stand there.
pub const WALL_ROWS: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Desk,
    Plant,
    Couch,
    Coffee,
    Bookshelf,
    Rug,
    Whiteboard,
    Jar,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::Desk,
        Kind::Plant,
        Kind::Couch,
        Kind::Coffee,
        Kind::Bookshelf,
        Kind::Rug,
        Kind::Whiteboard,
        Kind::Jar,
    ];

    /// Width and height in tiles. A desk includes the chair tile below it.
    pub fn size(self) -> (u32, u32) {
        match self {
            Kind::Desk | Kind::Bookshelf => (2, 2),
            Kind::Plant | Kind::Coffee | Kind::Jar => (1, 1),
            Kind::Couch => (3, 1),
            Kind::Rug => (4, 3),
            Kind::Whiteboard => (4, 2),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Desk => "desk",
            Kind::Plant => "plant",
            Kind::Couch => "couch",
            Kind::Coffee => "coffee",
            Kind::Bookshelf => "bookshelf",
            Kind::Rug => "rug",
            Kind::Whiteboard => "whiteboard",
            Kind::Jar => "jar",
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub kind: Kind,
    pub x: u32,
    pub y: u32,
}

impl Item {
    pub fn covers(&self, x: u32, y: u32) -> bool {
        let (w, h) = self.kind.size();
        x >= self.x && x < self.x + w && y >= self.y && y < self.y + h
    }

    fn overlaps(&self, other: &Item) -> bool {
        let (aw, ah) = self.kind.size();
        let (bw, bh) = other.kind.size();
        self.x < other.x + bw
            && other.x < self.x + aw
            && self.y < other.y + bh
            && other.y < self.y + ah
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub items: Vec<Item>,
}

impl Default for Layout {
    fn default() -> Self {
        presets::open_plan()
    }
}

impl Layout {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("office.json")
    }

    /// The saved layout, or the default if the file is missing or broken.
    pub async fn load(data_dir: &Path) -> Layout {
        match tokio::fs::read_to_string(Layout::path(data_dir)).await {
            Ok(text) => Layout::parse(&text),
            Err(_) => Layout::default(),
        }
    }

    /// Reads a layout, dropping items that don't fit. Anything unreadable or
    /// with an odd grid size gives the default.
    pub fn parse(text: &str) -> Layout {
        let Ok(mut layout) = serde_json::from_str::<Layout>(text) else {
            return Layout::default();
        };
        if !(8..=32).contains(&layout.width) || !(6..=20).contains(&layout.height) {
            return Layout::default();
        }
        let (w, h) = (layout.width, layout.height);
        layout.items.retain(|i| fits(i.kind, i.x, i.y, w, h));
        layout
    }

    /// Writes the file whole or not at all: temp file, then rename.
    pub async fn save(&self, data_dir: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        tokio::fs::create_dir_all(data_dir).await?;
        let path = Layout::path(data_dir);
        let temp = path.with_extension("json.tmp");
        tokio::fs::write(&temp, text).await?;
        tokio::fs::rename(&temp, &path).await
    }

    /// Puts `what` (an item name, or `erase`) on the tile. A new item
    /// replaces whatever of its own layer it overlaps; the rug is the floor
    /// layer, everything else stands on it.
    pub fn place(&mut self, x: i64, y: i64, what: &str) -> Result<(), String> {
        let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y)) else {
            return Err("that tile is outside the office".into());
        };
        if x >= self.width || y >= self.height {
            return Err("that tile is outside the office".into());
        }
        if what == "erase" {
            let top = self
                .items
                .iter()
                .rposition(|i| i.kind != Kind::Rug && i.covers(x, y))
                .or_else(|| self.items.iter().rposition(|i| i.covers(x, y)));
            if let Some(index) = top {
                self.items.remove(index);
            }
            return Ok(());
        }
        let kind = Kind::parse(what).ok_or_else(|| format!("unknown item: {what}"))?;
        if !fits(kind, x, y, self.width, self.height) {
            return Err(format!("a {} doesn't fit there", kind.name()));
        }
        let item = Item { kind, x, y };
        self.items
            .retain(|i| (i.kind == Kind::Rug) != (kind == Kind::Rug) || !i.overlaps(&item));
        self.items.push(item);
        Ok(())
    }

    /// Desks in seating order: top to bottom, then left to right.
    pub fn desks(&self) -> Vec<Item> {
        let mut desks: Vec<Item> = self
            .items
            .iter()
            .filter(|i| i.kind == Kind::Desk)
            .copied()
            .collect();
        desks.sort_by_key(|d| (d.y, d.x));
        desks
    }
}

fn fits(kind: Kind, x: u32, y: u32, width: u32, height: u32) -> bool {
    let (w, h) = kind.size();
    x + w <= width && y + h <= height
}

/// Ready-made arrangements, picked by key.
pub mod presets {
    use super::{HEIGHT, Item, Kind, Layout, WIDTH};

    pub const ALL: [(&str, &str); 4] = [
        ("open", "Open plan"),
        ("rows", "Rows"),
        ("pods", "Pods"),
        ("loft", "Loft"),
    ];

    pub fn by_key(key: &str) -> Option<Layout> {
        match key {
            "open" => Some(open_plan()),
            "rows" => Some(rows()),
            "pods" => Some(pods()),
            "loft" => Some(loft()),
            _ => None,
        }
    }

    fn build(items: &[(Kind, u32, u32)]) -> Layout {
        Layout {
            width: WIDTH,
            height: HEIGHT,
            items: items
                .iter()
                .map(|&(kind, x, y)| Item { kind, x, y })
                .collect(),
        }
    }

    pub fn open_plan() -> Layout {
        use Kind::{Bookshelf, Coffee, Couch, Desk, Jar, Plant, Rug, Whiteboard};
        build(&[
            (Rug, 14, 9),
            (Bookshelf, 0, 0),
            (Bookshelf, 18, 0),
            (Whiteboard, 8, 0),
            (Desk, 2, 3),
            (Desk, 5, 3),
            (Desk, 8, 3),
            (Desk, 11, 3),
            (Desk, 14, 3),
            (Desk, 2, 6),
            (Desk, 5, 6),
            (Desk, 8, 6),
            (Desk, 11, 6),
            (Coffee, 18, 3),
            (Jar, 19, 6),
            (Plant, 0, 3),
            (Plant, 0, 10),
            (Plant, 19, 11),
            (Couch, 15, 9),
        ])
    }

    pub fn rows() -> Layout {
        use Kind::{Bookshelf, Coffee, Couch, Desk, Jar, Plant, Rug, Whiteboard};
        build(&[
            (Rug, 12, 7),
            (Whiteboard, 8, 0),
            (Bookshelf, 0, 0),
            (Desk, 1, 3),
            (Desk, 4, 3),
            (Desk, 7, 3),
            (Desk, 1, 7),
            (Desk, 4, 7),
            (Desk, 7, 7),
            (Couch, 13, 7),
            (Coffee, 16, 3),
            (Jar, 17, 3),
            (Plant, 11, 3),
            (Plant, 11, 10),
            (Plant, 19, 10),
            (Bookshelf, 18, 0),
        ])
    }

    pub fn pods() -> Layout {
        use Kind::{Bookshelf, Coffee, Couch, Desk, Jar, Plant, Rug, Whiteboard};
        build(&[
            (Rug, 1, 3),
            (Rug, 8, 3),
            (Rug, 1, 7),
            (Whiteboard, 8, 0),
            (Desk, 1, 3),
            (Desk, 3, 3),
            (Desk, 8, 3),
            (Desk, 10, 3),
            (Desk, 1, 7),
            (Desk, 3, 7),
            (Couch, 14, 9),
            (Rug, 14, 9),
            (Coffee, 18, 3),
            (Jar, 17, 3),
            (Plant, 6, 5),
            (Plant, 6, 9),
            (Plant, 13, 5),
            (Bookshelf, 15, 0),
        ])
    }

    pub fn loft() -> Layout {
        use Kind::{Bookshelf, Coffee, Couch, Desk, Jar, Plant, Rug, Whiteboard};
        build(&[
            (Rug, 3, 6),
            (Rug, 12, 7),
            (Whiteboard, 8, 0),
            (Bookshelf, 0, 0),
            (Bookshelf, 2, 0),
            (Bookshelf, 18, 0),
            (Desk, 13, 2),
            (Desk, 16, 2),
            (Desk, 13, 4),
            (Couch, 4, 6),
            (Couch, 3, 9),
            (Couch, 7, 7),
            (Coffee, 11, 2),
            (Jar, 11, 3),
            (Plant, 0, 3),
            (Plant, 0, 8),
            (Plant, 10, 10),
            (Plant, 19, 10),
            (Plant, 12, 7),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_layout_loads_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut layout = presets::rows();
        layout.place(10, 10, "plant").unwrap();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            layout.save(dir.path()).await.unwrap();
            assert_eq!(Layout::load(dir.path()).await, layout);
        });
        assert!(!dir.path().join("office.json.tmp").exists());
    }

    #[test]
    fn missing_or_corrupt_files_give_the_default() {
        assert_eq!(Layout::parse("{ nope"), Layout::default());
        assert_eq!(
            Layout::parse("{\"width\":2,\"height\":2,\"items\":[]}"),
            Layout::default()
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(Layout::path(dir.path()), "garbage").unwrap();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            assert_eq!(Layout::load(dir.path()).await, Layout::default());
            assert_eq!(
                Layout::load(&dir.path().join("none")).await,
                Layout::default()
            );
        });
    }

    #[test]
    fn items_that_do_not_fit_are_dropped_on_load() {
        let text = r#"{"width":20,"height":12,"items":[
            {"kind":"desk","x":19,"y":0},{"kind":"plant","x":3,"y":3},{"kind":"sofa","x":1,"y":1}]}"#;
        // An unknown kind makes the whole file unreadable, so it falls back.
        assert_eq!(Layout::parse(text), Layout::default());
        let text = r#"{"width":20,"height":12,"items":[
            {"kind":"desk","x":19,"y":0},{"kind":"plant","x":3,"y":3}]}"#;
        assert_eq!(
            Layout::parse(text).items,
            vec![Item {
                kind: Kind::Plant,
                x: 3,
                y: 3
            }]
        );
    }

    #[test]
    fn placing_validates_and_replaces() {
        let mut layout = Layout {
            width: WIDTH,
            height: HEIGHT,
            items: vec![],
        };
        assert!(layout.place(-1, 3, "plant").is_err());
        assert!(layout.place(3, 12, "plant").is_err());
        assert!(layout.place(3, 3, "sofa").is_err());
        assert!(layout.place(19, 11, "desk").is_err());

        layout.place(3, 3, "desk").unwrap();
        layout.place(4, 4, "plant").unwrap();
        // A plant over a desk's chair tile replaces that desk.
        assert_eq!(
            layout.items,
            vec![Item {
                kind: Kind::Plant,
                x: 4,
                y: 4
            }]
        );

        // A rug sits under things instead of replacing them.
        layout.place(2, 3, "rug").unwrap();
        assert_eq!(layout.items.len(), 2);
        // Erase takes the thing on top first, then the rug.
        layout.place(4, 4, "erase").unwrap();
        assert_eq!(layout.items.len(), 1);
        layout.place(4, 4, "erase").unwrap();
        assert!(layout.items.is_empty());
    }

    #[test]
    fn every_preset_fits_the_grid() {
        for (key, _) in presets::ALL {
            let layout = presets::by_key(key).unwrap();
            for item in &layout.items {
                assert!(
                    fits(item.kind, item.x, item.y, layout.width, layout.height),
                    "{key}: {item:?}"
                );
            }
            assert!(layout.desks().len() >= 3, "{key}");
        }
        assert!(presets::by_key("nope").is_none());
    }
}
