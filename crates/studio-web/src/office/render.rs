//! Draws the office as one SVG. Everything is built as a string of
//! `<rect>`s on a tile grid; motion comes from the CSS in `CSS`.

use std::collections::HashSet;
use std::fmt::Write;

use super::layout::{Item, Kind, Layout, WALL_ROWS};
use super::scene::{Mood, Placement, Pose};
use super::sprites::{self, Look, block, plain};

/// Pixels per tile.
const T: i32 = 16;

/// Motion and hover styles, included once in the page.
pub const CSS: &str = "\
.o-type{animation:o-bob .35s steps(2) infinite}\
.o-slow{animation:o-bob 1.4s steps(2) infinite}\
@keyframes o-bob{50%{transform:translateY(-1px)}}\
.o-hop{animation:o-hop .7s steps(4) infinite}\
@keyframes o-hop{0%,100%{transform:translateY(0)}50%{transform:translateY(-3px)}}\
.o-flick{animation:o-flick 1.1s steps(1) infinite}\
@keyframes o-flick{0%{opacity:0}20%{opacity:.35}40%{opacity:.1}60%{opacity:.5}80%{opacity:.2}}\
.o-steam{animation:o-rise 1.8s linear infinite}\
.o-z{animation:o-rise 2.6s linear infinite}\
@keyframes o-rise{0%{transform:translateY(2px);opacity:0}30%{opacity:.9}100%{transform:translateY(-5px);opacity:0}}\
.o-spark{animation:o-spark 1.4s ease-out infinite}\
@keyframes o-spark{0%{transform:translate(0,4px);opacity:0}30%{opacity:1}100%{transform:translate(var(--dx),-14px);opacity:0}}\
.o-who{cursor:pointer}\
.o-who:hover .o-person{filter:brightness(1.12)}\
.o-cell{fill:transparent;stroke:rgba(255,255,255,.4);stroke-width:.3;cursor:pointer}\
.o-cell:hover{fill:rgba(255,255,255,.4)}\
@media (prefers-reduced-motion:reduce){#office *{animation:none!important}}";

/// What the live props need to know.
#[derive(Default)]
pub struct Props {
    /// Queued, working, needs you, ready.
    pub counts: [u32; 4],
    pub spent: f64,
    pub budget: f64,
    /// Local hour, 0 to 23.
    pub hour: u32,
    /// Hires whose ticket landed a moment ago.
    pub landed: HashSet<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sky {
    Day,
    Sunset,
    Night,
}

fn sky(hour: u32) -> Sky {
    match hour {
        6..17 => Sky::Day,
        17..20 => Sky::Sunset,
        _ => Sky::Night,
    }
}

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

fn px(tile: u32) -> i32 {
    tile as i32 * T
}

/// The whole office, wrapped in the element Datastar patches.
pub fn office(layout: &Layout, placed: &[Placement], props: &Props) -> String {
    let (w, h) = (px(layout.width), px(layout.height));
    let mut out = format!(
        r#"<div id="office"><svg viewBox="0 0 {w} {h}" width="100%" shape-rendering="crispEdges" role="img" aria-label="The studio office" class="block h-auto w-full">"#
    );
    out.push_str(&room(layout, props));

    let occupied: HashSet<(u32, u32)> = placed
        .iter()
        .filter(|p| p.pose == Pose::Seated)
        .map(|p| p.tile)
        .collect();
    let mut items: Vec<&Item> = layout.items.iter().collect();
    items.sort_by_key(|i| (i.kind != Kind::Rug, i.y + i.kind.size().1));
    for item in items {
        out.push_str(&draw_item(
            item,
            props,
            occupied.contains(&(item.x, item.y)),
        ));
    }
    if sky(props.hour) == Sky::Night {
        out.push_str(&format!(
            r##"<rect width="{w}" height="{h}" fill="#1b1f4b" opacity=".2"/>"##
        ));
    }

    let mut people: Vec<&Placement> = placed.iter().collect();
    people.sort_by_key(|p| p.tile.1);
    for person in &people {
        out.push_str(&character(
            person,
            layout,
            props.landed.contains(&person.hire),
        ));
    }
    out.push_str(&cat(layout));
    out.push_str("</svg></div>");
    out
}

/// Floor, wall and windows.
fn room(layout: &Layout, props: &Props) -> String {
    let (w, h) = (px(layout.width), px(layout.height));
    let wall = px(WALL_ROWS);
    let mut out = String::from(
        r##"<defs><pattern id="o-wood" width="32" height="16" patternUnits="userSpaceOnUse"><rect width="16" height="16" fill="#cf9d63"/><rect x="16" width="16" height="16" fill="#c8965c"/><rect y="7" width="32" height="1" fill="#b4814b"/><rect y="15" width="32" height="1" fill="#b4814b"/><rect x="11" width="1" height="7" fill="#b4814b"/><rect x="27" y="8" width="1" height="7" fill="#b4814b"/><rect x="3" y="2" width="2" height="1" fill="#dcae75"/><rect x="19" y="10" width="2" height="1" fill="#dcae75"/></pattern></defs>"##,
    );
    let _ = write!(
        out,
        r#"<rect y="{wall}" width="{w}" height="{}" fill="url(#o-wood)"/>"#,
        h - wall
    );
    out.push_str(&block(0, 0, w, wall, "#eadcc3"));
    out.push_str(&block(0, 0, w, 2, "#d6c4a2"));
    for x in (T - 1..w).step_by(T as usize) {
        out.push_str(&block(x, 2, 1, wall - 6, "#dccdb0"));
    }
    out.push_str(&block(0, wall - 4, w, 4, "#8d6238"));
    out.push_str(&block(0, wall - 4, w, 1, "#a8794a"));
    let n = layout.width;
    for col in [n * 3 / 20, n * 6 / 20, n * 12 / 20, n * 15 / 20] {
        out.push_str(&window(px(col), sky(props.hour)));
    }
    out
}

fn window(x: i32, sky: Sky) -> String {
    let mut out = block(x + 1, 4, 30, 21, "#6b4a2b");
    let (top, bottom) = match sky {
        Sky::Day => ("#8fd0f0", "#b9e4f7"),
        Sky::Sunset => ("#f3a35c", "#e4766a"),
        Sky::Night => ("#1f2a55", "#2e3a73"),
    };
    out.push_str(&block(x + 3, 6, 26, 8, top));
    out.push_str(&block(x + 3, 14, 26, 9, bottom));
    match sky {
        Sky::Day => {
            out.push_str(&block(x + 20, 8, 5, 5, "#ffe066"));
            out.push_str(&block(x + 6, 10, 8, 2, "#ffffff"));
            out.push_str(&block(x + 8, 8, 4, 2, "#ffffff"));
        }
        Sky::Sunset => {
            out.push_str(&block(x + 11, 16, 8, 6, "#ffd36b"));
            out.push_str(&block(x + 3, 20, 26, 3, "#b2506a"));
        }
        Sky::Night => {
            out.push_str(&block(x + 21, 8, 4, 4, "#f4f0c8"));
            out.push_str(&block(x + 23, 8, 2, 3, "#1f2a55"));
            for (sx, sy) in [(6, 8), (12, 12), (16, 8), (9, 17), (25, 18)] {
                out.push_str(&block(x + sx, sy, 1, 1, "#ffffff"));
            }
        }
    }
    out.push_str(&block(x + 15, 6, 2, 17, "#6b4a2b"));
    out.push_str(&block(x + 3, 14, 26, 2, "#6b4a2b"));
    out.push_str(&block(x, 24, 32, 2, "#c9b48c"));
    out
}

fn draw_item(item: &Item, props: &Props, occupied: bool) -> String {
    let (ox, oy) = (px(item.x), px(item.y));
    let mut out = format!(r#"<g data-kind="{}">"#, item.kind.name());
    match item.kind {
        Kind::Rug => out.push_str(&rug(ox, oy, item.x + item.y)),
        Kind::Desk => out.push_str(&desk(ox, oy, occupied)),
        Kind::Plant => out.push_str(&plain(sprites::PLANT, ox, oy)),
        Kind::Coffee => {
            out.push_str(&plain(sprites::COFFEE, ox, oy));
            for (dx, dy, delay) in [(6, -1, "0s"), (9, -3, "-0.9s")] {
                let _ = write!(
                    out,
                    r##"<rect class="o-steam" x="{}" y="{}" width="1" height="2" fill="#ffffff" style="animation-delay:{delay}"/>"##,
                    ox + dx,
                    oy + dy
                );
            }
        }
        Kind::Couch => out.push_str(&couch(ox, oy)),
        Kind::Bookshelf => out.push_str(&bookshelf(ox, oy)),
        Kind::Whiteboard => out.push_str(&whiteboard(ox, oy, props.counts)),
        Kind::Jar => out.push_str(&jar(ox, oy, props.spent, props.budget)),
    }
    out.push_str("</g>");
    out
}

fn rug(x: i32, y: i32, seed: u32) -> String {
    let (edge, mid, inner) = if seed.is_multiple_of(2) {
        ("#93392f", "#c85a4a", "#e8b46a")
    } else {
        ("#2a625f", "#3f8f8b", "#e8d6a0")
    };
    let (w, h) = (64, 48);
    let mut out = block(x, y, w, h, edge);
    out.push_str(&block(x + 2, y + 2, w - 4, h - 4, mid));
    out.push_str(&block(x + 5, y + 5, w - 10, h - 10, inner));
    out.push_str(&block(x + 7, y + 7, w - 14, h - 14, mid));
    for i in 0..8 {
        out.push_str(&block(x + 4 + i * 8, y - 1, 1, 1, inner));
        out.push_str(&block(x + 4 + i * 8, y + h, 1, 1, inner));
    }
    out
}

fn desk(x: i32, y: i32, occupied: bool) -> String {
    let screen = if occupied { "#7ec8f0" } else { "#3a4450" };
    let paint = |key: char| (key == 'm').then(|| screen.to_owned());
    let mut out = sprites::draw(sprites::DESK, x, y, &paint);
    out.push_str(&plain(sprites::CHAIR, x + 10, y + 17));
    if occupied {
        let _ = write!(
            out,
            r##"<rect class="o-flick" x="{}" y="{}" width="8" height="4" fill="#e8f8ff" opacity="0"/>"##,
            x + 12,
            y + 2
        );
    }
    out
}

fn couch(x: i32, y: i32) -> String {
    let mut out = block(x, y, 48, 16, "#2f5470");
    out.push_str(&block(x + 3, y, 42, 8, "#4d7a9c"));
    out.push_str(&block(x + 3, y, 42, 1, "#6a95b5"));
    out.push_str(&block(x + 5, y + 8, 38, 7, "#6a95b5"));
    out.push_str(&block(x + 17, y + 8, 1, 7, "#3f6683"));
    out.push_str(&block(x + 30, y + 8, 1, 7, "#3f6683"));
    out.push_str(&block(x, y + 15, 48, 1, "#1d3446"));
    out
}

fn bookshelf(x: i32, y: i32) -> String {
    let mut out = block(x, y, 32, 32, "#6b4a2b");
    out.push_str(&block(x + 2, y + 2, 28, 28, "#3a2a1c"));
    let covers = [
        "#d65a4a", "#4a8fd6", "#e0b13c", "#58b368", "#9b6bd6", "#e8844a",
    ];
    for shelf in 0..3 {
        let top = y + 2 + shelf * 10;
        let mut cursor = x + 3;
        let mut n = shelf as usize * 5;
        while cursor < x + 28 {
            let width = 2 + (n % 3) as i32;
            let height = 5 + ((n * 7) % 3) as i32;
            if cursor + width > x + 29 {
                break;
            }
            out.push_str(&block(
                cursor,
                top + 8 - height,
                width,
                height,
                covers[n % covers.len()],
            ));
            cursor += width + 1 - (n % 2) as i32;
            n += 1;
        }
        out.push_str(&block(x + 2, top + 8, 28, 2, "#9a6b3c"));
    }
    out
}

fn whiteboard(x: i32, y: i32, counts: [u32; 4]) -> String {
    let mut out = block(x, y, 64, 32, "#7d868c");
    out.push_str(&block(x + 2, y + 2, 60, 26, "#f7f7f1"));
    out.push_str(&block(x + 6, y + 29, 52, 2, "#a8b0b5"));
    out.push_str(&block(x + 22, y + 28, 3, 2, "#d65a4a"));
    let rows = [
        ("queued", "#8a929a"),
        ("working", "#2f6aa8"),
        ("needs you", "#d19a2b"),
        ("ready", "#3f8f45"),
    ];
    for (i, ((label, color), count)) in rows.iter().zip(counts).enumerate() {
        let ty = y + 8 + i as i32 * 6;
        out.push_str(&block(x + 5, ty - 4, 3, 3, color));
        let _ = write!(
            out,
            r##"<text x="{}" y="{ty}" font-size="5" font-family="ui-monospace,Menlo,monospace" fill="#333" text-rendering="geometricPrecision">{label} {count}</text>"##,
            x + 10
        );
    }
    out
}

fn jar(x: i32, y: i32, spent: f64, budget: f64) -> String {
    let fraction = if budget > 0.0 {
        (spent / budget).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let coins = (fraction * 9.0).round() as i32;
    let mut out = format!(
        "<g><title>{}</title>",
        esc(&format!("${spent:.2} spent of ${budget:.2} today"))
    );
    out.push_str(&block(x + 3, y + 2, 10, 2, "#c98f25"));
    out.push_str(&block(x + 4, y + 4, 8, 11, "#aab2b7"));
    out.push_str(&block(x + 5, y + 4, 6, 10, "#e3f1f4"));
    if coins > 0 {
        out.push_str(&block(x + 5, y + 14 - coins, 6, coins, "#f2c14e"));
        out.push_str(&block(x + 6, y + 14 - coins, 2, 1, "#fff0a8"));
        if coins > 3 {
            out.push_str(&block(x + 8, y + 12, 2, 1, "#c98f25"));
        }
    }
    out.push_str(&block(x + 4, y + 15, 8, 1, "#646c72"));
    out.push_str("</g>");
    out
}

/// A hire with their tooltip, bubble and name.
fn character(p: &Placement, layout: &Layout, landed: bool) -> String {
    let look = Look::of(&p.hire);
    let (tx, ty) = (px(p.tile.0), px(p.tile.1));
    // Where the body goes, where the bubble hangs from, and the name line.
    let (body, anchor, name_at) = match p.pose {
        Pose::Seated => (
            look.draw(sprites::BACK, 12, tx + 11, ty + 11),
            (tx + 26, ty - 3),
            (tx + 16, ty + 31),
        ),
        Pose::Standing => (
            look.draw(sprites::FRONT, 14, tx + 3, ty + 1),
            (tx + 8, ty - 9),
            (tx + 8, ty + 21),
        ),
        Pose::Couch => (
            look.draw(sprites::FRONT, 10, tx + 3, ty - 4),
            (tx + 8, ty - 13),
            (tx + 8, ty + 21),
        ),
    };
    let motion = match p.mood {
        Mood::Typing => "o-person o-type",
        Mood::Reading | Mood::Waiting => "o-person o-slow",
        _ => "o-person",
    };
    let tip = match &p.focus {
        Some(f) => format!("{} · {} · {}: {}", p.hire, f.state, f.id, f.title),
        None => format!("{} · idle", p.hire),
    };
    let mut inner = format!(
        r#"<title>{}</title><g class="{motion}">{body}</g>"#,
        esc(&tip)
    );
    // Someone with a coffee in hand doesn't need a sleepy "z".
    let at_coffee = p.mood == Mood::Idle
        && p.pose == Pose::Standing
        && layout
            .items
            .iter()
            .any(|i| i.kind == Kind::Coffee && (i.x, i.y + 1) == p.tile);
    if !at_coffee {
        inner.push_str(&bubble(p, anchor, px(layout.width)));
    }
    let name: String = p.hire.chars().take(10).collect();
    let _ = write!(
        inner,
        r##"<text x="{}" y="{}" font-size="4.5" text-anchor="middle" font-family="ui-monospace,Menlo,monospace" fill="#fff" stroke="#2b2118" stroke-width="1.2" paint-order="stroke" text-rendering="geometricPrecision">{}</text>"##,
        name_at.0,
        name_at.1,
        esc(&name)
    );
    let mut out = match (&p.focus, p.mood) {
        (_, Mood::Asking) => format!(r#"<a href="/asks" class="o-who">{inner}</a>"#),
        (Some(f), _) => format!(
            r#"<a href="/tickets/{}" class="o-who">{inner}</a>"#,
            esc(&f.id)
        ),
        (None, _) => format!("<g>{inner}</g>"),
    };
    if landed {
        out.push_str(&sparkles(anchor.0, anchor.1 + 10));
    }
    out
}

/// A speech bubble for what the hire is up to: the ticket, a magnifier, a
/// wait, an urgent "!" or a sleepy "z".
fn bubble(p: &Placement, (cx, top): (i32, i32), max_x: i32) -> String {
    let id = p.focus.as_ref().map_or("", |f| f.id.as_str());
    let (w, class, content): (i32, &str, String) = match p.mood {
        Mood::Typing => {
            let w = (id.chars().count() as i32 * 3 + 4).max(11);
            (w, "", text_in(id, w, "#333"))
        }
        Mood::Reading => (11, "", plain(sprites::LENS, 2, 1)),
        Mood::Waiting => (
            11,
            "",
            [2, 5, 8].map(|x| block(x, 6, 1, 1, "#646c72")).concat(),
        ),
        Mood::Asking => (9, "o-hop", text_in("!", 9, "#9a5b00")),
        Mood::Idle => (9, "o-z", text_in("z", 9, "#6a7fa8")),
    };
    // Keep the bubble inside the picture; the tail keeps pointing at the head.
    let left = (cx - w / 2).clamp(1, (max_x - w - 1).max(1));
    let top = top.max(1);
    let fill = if p.mood == Mood::Asking {
        "#ffd96b"
    } else {
        "#ffffff"
    };
    let tail = (cx - 1).clamp(left + 1, left + w - 3);
    format!(
        r##"<g class="{class}"><g transform="translate({left},{top})"><rect x="-1" y="-1" width="{}" height="11" fill="#2b2118"/><rect width="{w}" height="9" fill="{fill}"/><rect x="{}" y="9" width="3" height="2" fill="#2b2118"/><rect x="{}" y="9" width="1" height="1" fill="{fill}"/>{content}</g></g>"##,
        w + 2,
        tail - left - 1,
        tail - left
    )
}

fn text_in(text: &str, width: i32, color: &str) -> String {
    format!(
        r#"<text x="{}" y="6.6" font-size="5.5" text-anchor="middle" font-weight="bold" font-family="ui-monospace,Menlo,monospace" fill="{color}" text-rendering="geometricPrecision">{}</text>"#,
        width / 2,
        esc(text)
    )
}

fn sparkles(x: i32, y: i32) -> String {
    let mut out = String::from("<g>");
    let bits = [
        ("#ffd23f", -12, "0s"),
        ("#ff6f91", -5, "-0.3s"),
        ("#4cc9f0", 4, "-0.6s"),
        ("#9ef01a", 11, "-0.9s"),
        ("#ffffff", 0, "-1.2s"),
    ];
    for (color, dx, delay) in bits {
        let _ = write!(
            out,
            r#"<rect class="o-spark" x="{}" y="{y}" width="2" height="2" fill="{color}" style="--dx:{}px;animation-delay:{delay}"/>"#,
            x + dx,
            dx / 2
        );
    }
    out.push_str("</g>");
    out
}

/// The cat's tour: along the bottom, up to the first desk for a sit, and
/// back. Position is a CSS animation, so it costs nothing to keep running.
fn cat(layout: &Layout) -> String {
    let (w, h) = (px(layout.width), px(layout.height));
    let floor = (h - 11) as f32;
    let mut stops: Vec<(f32, f32, f32)> = vec![(6.0, floor, 2.0), ((w - 24) as f32, floor, 3.0)];
    if let Some(desk) = layout.desks().first() {
        // Up onto the left end of the desk, then a long sit.
        stops.push((px(desk.x) as f32 + 1.0, px(desk.y) as f32 + 1.0, 14.0));
    }
    const SPEED: f32 = 9.0;
    let mut times = Vec::new();
    let mut clock = 0.0;
    for (i, &(x, y, hold)) in stops.iter().enumerate() {
        times.push((clock, clock + hold));
        let (nx, ny, _) = stops[(i + 1) % stops.len()];
        clock += hold + ((nx - x).powi(2) + (ny - y).powi(2)).sqrt() / SPEED;
    }
    let total = clock.max(1.0);
    let flip = |x: f32, nx: f32| nx < x;
    let place = |x: f32, y: f32, left: bool| {
        if left {
            format!("translate({}px,{y}px) scaleX(-1)", x + 10.0)
        } else {
            format!("translate({x}px,{y}px)")
        }
    };
    let mut frames = String::new();
    for (i, &(x, y, _)) in stops.iter().enumerate() {
        let (px_, _, _) = stops[(i + stops.len() - 1) % stops.len()];
        let (nx, _, _) = stops[(i + 1) % stops.len()];
        let (arrive, leave) = times[i];
        let _ = write!(
            frames,
            "{:.3}%{{transform:{}}}{:.3}%{{transform:{}}}",
            arrive / total * 100.0,
            place(x, y, flip(px_, x)),
            leave / total * 100.0,
            place(x, y, flip(x, nx)),
        );
    }
    format!(
        r#"<style>@keyframes o-cat{{{frames}100%{{transform:{}}}}}</style><g style="animation:o-cat {total:.1}s linear infinite" transform="translate(6,{floor})"><g class="o-slow"><title>The office cat</title>{}</g></g>"#,
        place(
            stops[0].0,
            stops[0].1,
            flip(stops[stops.len() - 1].0, stops[0].0)
        ),
        plain(sprites::CAT, 0, 0),
    )
}

/// Invisible clickable tiles over the office, for edit mode. Each click
/// sets the `x` and `y` signals and posts them with the chosen item.
pub fn grid_overlay(layout: &Layout) -> String {
    let mut out = format!(
        r#"<svg viewBox="0 0 {} {}" class="absolute inset-0 h-full w-full" style="display:none" data-show="$edit">"#,
        px(layout.width),
        px(layout.height)
    );
    for y in 0..layout.height {
        for x in 0..layout.width {
            let _ = write!(
                out,
                r#"<rect class="o-cell" x="{}" y="{}" width="16" height="16" data-on:click="$x={x};$y={y};@post('/office/place')"/>"#,
                px(x),
                px(y)
            );
        }
    }
    out.push_str("</svg>");
    out
}

/// A small picture of an item, for the edit palette.
pub fn thumb(kind: Kind) -> String {
    let (w, h) = kind.size();
    let item = Item { kind, x: 0, y: 0 };
    let props = Props {
        counts: [3, 2, 1, 1],
        spent: 5.0,
        budget: 10.0,
        ..Props::default()
    };
    format!(
        r#"<svg viewBox="0 0 {} {}" shape-rendering="crispEdges" class="h-7 w-auto max-w-14">{}</svg>"#,
        px(w),
        px(h),
        draw_item(&item, &props, true)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::layout::presets;
    use crate::office::scene::Focus;

    fn working(hire: &str, tile: (u32, u32), mood: Mood) -> Placement {
        Placement {
            hire: hire.into(),
            tile,
            pose: Pose::Seated,
            mood,
            focus: Some(Focus {
                id: "G-1".into(),
                title: "Fix <b>it</b>".into(),
                state: "working",
            }),
        }
    }

    #[test]
    fn people_get_tooltips_links_and_escaping() {
        let layout = presets::open_plan();
        let html = office(
            &layout,
            &[
                working("dev", (2, 3), Mood::Typing),
                working("qa", (5, 3), Mood::Asking),
            ],
            &Props::default(),
        );
        assert!(html.contains(r#"href="/tickets/G-1""#));
        assert!(html.contains(r#"href="/asks""#));
        assert!(html.contains("dev · working · G-1: Fix &lt;b&gt;it&lt;/b&gt;"));
        assert!(html.contains("o-type") && html.contains("o-hop"));
        assert!(html.starts_with(r#"<div id="office">"#));
    }

    #[test]
    fn idle_hires_have_no_link_and_landings_sparkle() {
        let layout = presets::open_plan();
        let idle = Placement {
            hire: "zed".into(),
            tile: (10, 10),
            pose: Pose::Standing,
            mood: Mood::Idle,
            focus: None,
        };
        let props = Props {
            landed: ["zed".to_owned()].into(),
            ..Props::default()
        };
        let html = office(&layout, &[idle], &props);
        assert!(html.contains("zed · idle"));
        assert!(html.contains("o-z"));
        assert!(html.contains("o-spark"));
        assert!(!html.contains(r#"class="o-who""#));
    }

    #[test]
    fn the_sky_follows_the_hour() {
        assert!(sky(12) == Sky::Day && sky(18) == Sky::Sunset && sky(23) == Sky::Night);
        assert!(sky(3) == Sky::Night && sky(6) == Sky::Day);
    }
}
