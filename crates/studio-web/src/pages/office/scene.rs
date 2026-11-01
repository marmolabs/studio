//! The isometric office floor: a `Floor` drawn as SVG, with the labels
//! to pin over it.
//!
//! The floor is a grid in world coordinates: x runs along the long side, y
//! along the short side, z is up. A `View` turns the room a quarter turn at
//! a time and maps the grid onto the screen: u and v are the grid as seen
//! from the chosen corner, u running right and down the screen and v left
//! and down. Everything is drawn back to front: ground, walls, then objects
//! by depth, then overlays. Colours come only from CSS classes (and the
//! gradients in `<defs>`), so both themes work.

use std::fmt::Write;

use super::model::{Crate, Doing, Element, Floor, Note, Pin, PinWhat, Role, Scene, Turn, Worker};
use super::{key, safe};

/// Tile width on screen.
const TW: f64 = 56.0;
const HX: f64 = TW / 2.0;
const HY: f64 = TW / 4.0;
/// One unit of height.
const ZH: f64 = 30.0;
/// Floor size in tiles.
const W: f64 = 22.0;
const D: f64 = 14.0;
const PAD: f64 = 28.0;
const OY: f64 = 2.9 * ZH + PAD + 6.0;
const VW: f64 = (W + D) * HX + 2.0 * PAD;
const VH: f64 = (W + D) * HY + OY + 0.8 * ZH + PAD + 6.0;

/// The most desks drawn in the workshop.
const MAX_DESKS: usize = 9;
const DESK_X: [f64; 3] = [8.0, 10.6, 13.2];
const DESK_Y: [f64; 3] = [1.2, 4.4, 6.6];
/// The most racks in the checks lab, besides the free slot after them.
const MAX_RACKS: usize = 5;
/// The most house-rule posters, hire cards and trophy cups drawn.
const MAX_POSTERS: usize = 4;
const MAX_CARDS: usize = 8;
const MAX_CUPS: usize = 5;
/// How far into the room a board stands when its wall is at the front.
const STAND: f64 = 0.5;
/// What tells two depths apart.
const EPS: f64 = 1e-6;
/// The step that puts one thing just before or after another in depth.
const NUDGE: f64 = 1e-3;

type Pt = (f64, f64);

// ---------- the view ----------

/// The room seen from one of its four corners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    t: u8,
}

impl View {
    fn new(turn: Turn) -> View {
        View { t: turn.index() }
    }

    /// A world point as view coordinates.
    fn uv(self, x: f64, y: f64) -> Pt {
        match self.t {
            1 => (D - y, x),
            2 => (W - x, D - y),
            3 => (y, W - x),
            _ => (x, y),
        }
    }

    /// View coordinates as a world point.
    fn xy(self, u: f64, v: f64) -> Pt {
        match self.t {
            1 => (v, D - u),
            2 => (W - u, D - v),
            3 => (W - v, u),
            _ => (u, v),
        }
    }

    /// How far the room reaches along u and along v.
    fn size(self) -> Pt {
        if self.t.is_multiple_of(2) {
            (W, D)
        } else {
            (D, W)
        }
    }

    fn ox(self) -> f64 {
        self.size().1 * HX + PAD
    }

    /// A view point on the screen.
    fn s(self, u: f64, v: f64, z: f64) -> Pt {
        (self.ox() + (u - v) * HX, OY + (u + v) * HY - z * ZH)
    }

    /// A grid point on the screen.
    fn p(self, x: f64, y: f64, z: f64) -> Pt {
        let (u, v) = self.uv(x, y);
        self.s(u, v, z)
    }

    /// A screen point as a percentage of the view box.
    fn pct_of((sx, sy): Pt) -> Pt {
        (
            (sx / VW * 10000.0).round() / 100.0,
            (sy / VH * 10000.0).round() / 100.0,
        )
    }

    /// A grid point as a percentage of the view box.
    fn pct(self, x: f64, y: f64, z: f64) -> Pt {
        View::pct_of(self.p(x, y, z))
    }

    /// Like `pct`, moved by (du, dv) in view space, so a label sits the
    /// same way from its subject in every view.
    fn pct_off(self, at: (f64, f64, f64), off: Pt) -> Pt {
        let (u, v) = self.uv(at.0, at.1);
        View::pct_of(self.s(u + off.0, v + off.1, at.2))
    }

    /// A world direction as a view direction.
    fn dir(self, dx: f64, dy: f64) -> Pt {
        let (a, b) = self.uv(dx, dy);
        let (o, q) = self.uv(0.0, 0.0);
        (a - o, b - q)
    }

    /// Whether a world direction points toward the camera.
    fn toward(self, dx: f64, dy: f64) -> bool {
        let (du, dv) = self.dir(dx, dy);
        du + dv > 0.0
    }

    /// The two walls at the back: the one on the plane u = 0 and the one on
    /// the plane v = 0.
    fn walls(self) -> (Wall, Wall) {
        match self.t {
            1 => (Wall::South, Wall::West),
            2 => (Wall::East, Wall::South),
            3 => (Wall::North, Wall::East),
            _ => (Wall::West, Wall::North),
        }
    }

    fn at_back(self, wall: Wall) -> bool {
        let (a, b) = self.walls();
        wall == a || wall == b
    }

    /// The pose of a figure facing the world direction `face`: from behind
    /// when it faces away from the camera, from the front when it faces it.
    /// A figure in profile is seen from the side when it faces away along u.
    fn pose(self, sit: bool, face: Pt, profile: bool) -> &'static str {
        let (du, dv) = self.dir(face.0, face.1);
        let kind = if dv < -0.5 || (du < -0.5 && !profile) {
            0
        } else if du < -0.5 {
            1
        } else {
            2
        };
        match (sit, kind) {
            (true, 0) => "sit-back",
            (true, 1) => "sit-side",
            (true, _) => "sit-front",
            (false, 0) => "stand-back",
            (false, 1) => "stand-side",
            (false, _) => "stand-front",
        }
    }

    /// How far from the back corner a footprint's centre is: bigger is
    /// nearer the camera.
    fn depth(self, f: &Fp) -> f64 {
        let (u, v) = self.uv((f.x0 + f.x1) / 2.0, (f.y0 + f.y1) / 2.0);
        u + v
    }

    /// Whether `a` is drawn before `b`: when they stand apart along an
    /// axis, the one farther from the camera; when they overlap, the lower.
    fn before(self, a: &Fp, b: &Fp) -> bool {
        let axis = |a0: f64, a1: f64, b0: f64, b1: f64, plus_near: bool| {
            if a1 <= b0 + EPS {
                Some(plus_near)
            } else if b1 <= a0 + EPS {
                Some(!plus_near)
            } else {
                None
            }
        };
        let ox = axis(a.x0, a.x1, b.x0, b.x1, self.toward(1.0, 0.0));
        let oy = axis(a.y0, a.y1, b.y0, b.y1, self.toward(0.0, 1.0));
        match (ox, oy) {
            (Some(p), Some(q)) if p == q => p,
            (Some(_), Some(_)) => self.depth(a) < self.depth(b),
            (Some(p), None) | (None, Some(p)) => p,
            (None, None) => a.z0 < b.z0 - EPS,
        }
    }

    /// The depth of something that stands with `host`: just before it when
    /// it is behind, otherwise at least just after it.
    fn depth_on(self, f: &Fp, host: &Fp) -> f64 {
        let own = self.depth(f);
        let at = self.depth(host);
        if self.before(f, host) {
            own.min(at - NUDGE)
        } else {
            own.max(at + NUDGE)
        }
    }

    /// The box around a solid on the screen: left, top, right, bottom.
    fn bbox(self, f: &Fp) -> [f64; 4] {
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for x in [f.x0, f.x1] {
            for y in [f.y0, f.y1] {
                for z in [f.z0, f.z1] {
                    let (sx, sy) = self.p(x, y, z);
                    b = [b[0].min(sx), b[1].min(sy), b[2].max(sx), b[3].max(sy)];
                }
            }
        }
        b
    }
}

/// What a solid takes up on the floor and in height.
#[derive(Clone, Copy, Debug)]
struct Fp {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    z0: f64,
    z1: f64,
}

/// The footprint of a box, with the arguments `bx` takes.
fn fp(x: f64, y: f64, z: f64, w: f64, d: f64, h: f64) -> Fp {
    Fp {
        x0: x,
        y0: y,
        x1: x + w,
        y1: y + d,
        z0: z,
        z1: z + h,
    }
}

/// The footprint of a figure standing or sitting at a point.
fn spot(x: f64, y: f64, z: f64) -> Fp {
    fp(x - 0.2, y - 0.2, z, 0.4, 0.4, 1.2)
}

/// One of the four walls round the room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wall {
    /// The plane y = 0.
    North,
    /// The plane x = 0.
    West,
    /// The plane y = D.
    South,
    /// The plane x = W.
    East,
}

impl Wall {
    /// A point `a` along the wall, `off` into the room and `z` up.
    fn at(self, a: f64, off: f64, z: f64) -> (f64, f64, f64) {
        match self {
            Wall::North => (a, off, z),
            Wall::West => (off, a, z),
            Wall::South => (a, D - off, z),
            Wall::East => (W - off, a, z),
        }
    }

    fn along_x(self) -> bool {
        matches!(self, Wall::North | Wall::South)
    }

    /// The footprint of a thing `thick` deep, centred `off` into the room,
    /// from `a0` to `a1` along the wall.
    fn fp(self, off: f64, (a0, a1): Pt, thick: f64, z: f64, h: f64) -> Fp {
        let c = self.at(a0, off, z);
        if self.along_x() {
            fp(c.0, c.1 - thick / 2.0, z, a1 - a0, thick, h)
        } else {
            fp(c.0 - thick / 2.0, c.1, z, thick, a1 - a0, h)
        }
    }
}

fn pts(ps: &[Pt]) -> String {
    ps.iter()
        .map(|(a, b)| format!("{a:.1},{b:.1}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` safe to put in XML text or an attribute.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// The attributes that make a drawing a control.
fn control(sel: &str, label: &str) -> String {
    format!(
        r#"data-sel="{}" tabindex="0" role="button" aria-label="{}""#,
        esc(sel),
        esc(label)
    )
}

fn line(a: Pt, b: Pt) -> String {
    format!(
        r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}"/>"#,
        a.0, a.1, b.0, b.1
    )
}

fn poly(class: &str, ps: &[Pt]) -> String {
    format!(r#"<polygon class="{class}" points="{}"/>"#, pts(ps))
}

/// The left, right and top faces of a box. The footprint is turned as a
/// rectangle: the right face is the one on the far-u side and the left face
/// the one on the far-v side, in the view.
fn faces(v: View, foot: Foot, z: f64, h: f64) -> [[Pt; 4]; 3] {
    let (x, y, w, d) = foot;
    let (ua, va) = v.uv(x, y);
    let (ub, vb) = v.uv(x + w, y + d);
    let (u0, u1) = (ua.min(ub), ua.max(ub));
    let (v0, v1) = (va.min(vb), va.max(vb));
    let top = z + h;
    [
        [
            v.s(u0, v1, z),
            v.s(u1, v1, z),
            v.s(u1, v1, top),
            v.s(u0, v1, top),
        ],
        [
            v.s(u1, v0, z),
            v.s(u1, v1, z),
            v.s(u1, v1, top),
            v.s(u1, v0, top),
        ],
        [
            v.s(u0, v0, top),
            v.s(u1, v0, top),
            v.s(u1, v1, top),
            v.s(u0, v1, top),
        ],
    ]
}

/// A box with its left, right and top faces, in a group of class `m mat`.
#[allow(clippy::too_many_arguments)]
fn bx(v: View, x: f64, y: f64, z: f64, w: f64, d: f64, h: f64, mat: &str, extra: &str) -> String {
    let [l, r, t] = faces(v, (x, y, w, d), z, h);
    format!(
        r#"<g class="m {mat}"{extra}>{}{}{}</g>"#,
        poly("l", &l),
        poly("r", &r),
        poly("t", &t)
    )
}

/// The dashed outline of a box that is not there yet.
fn ghost_box(v: View, foot: Foot, z: f64, h: f64) -> String {
    faces(v, foot, z, h)
        .iter()
        .map(|f| poly("ghost", f))
        .collect()
}

/// A box given in view coordinates.
fn bx_view(v: View, (u0, u1): Pt, (v0, v1): Pt, z: f64, h: f64, mat: &str) -> String {
    let (xa, ya) = v.xy(u0, v0);
    let (xb, yb) = v.xy(u1, v1);
    bx(
        v,
        xa.min(xb),
        ya.min(yb),
        z,
        (xa - xb).abs(),
        (ya - yb).abs(),
        h,
        mat,
        "",
    )
}

/// The footprint of a box as `bx` takes it: x, y, width, depth.
type Foot = (f64, f64, f64, f64);

/// Where a box's face on the far-v side has `a` along it from its near-u
/// corner, at height `z`: for marks drawn on the face that shows.
fn vface_pt(v: View, foot: Foot, a: f64, z: f64) -> Pt {
    let (x, y, w, d) = foot;
    let (ua, va) = v.uv(x, y);
    let (ub, vb) = v.uv(x + w, y + d);
    v.s(ua.min(ub) + a, va.max(vb), z)
}

/// A rectangle on the face of a box that is on the far-v side.
fn vface(v: View, foot: Foot, (a0, a1): Pt, (z0, z1): Pt, class: &str) -> String {
    poly(
        class,
        &[
            vface_pt(v, foot, a0, z0),
            vface_pt(v, foot, a1, z0),
            vface_pt(v, foot, a1, z1),
            vface_pt(v, foot, a0, z1),
        ],
    )
}

/// A flat patch of floor.
fn flat(v: View, x0: f64, y0: f64, x1: f64, y1: f64, class: &str) -> String {
    poly(
        class,
        &[
            v.p(x0, y0, 0.0),
            v.p(x1, y0, 0.0),
            v.p(x1, y1, 0.0),
            v.p(x0, y1, 0.0),
        ],
    )
}

fn shadow(v: View, x: f64, y: f64, rx: f64, ry: f64) -> String {
    let (sx, sy) = v.p(x, y, 0.0);
    format!(
        r#"<ellipse class="shade" cx="{sx:.1}" cy="{sy:.1}" rx="{:.1}" ry="{:.1}"/>"#,
        rx * HX,
        ry * HX
    )
}

/// A rectangle on a wall, `off` into the room (0 for on the wall itself),
/// from `a0` to `a1` along it.
fn wall_rect(v: View, wall: Wall, off: f64, (a0, a1): Pt, (z0, z1): Pt, class: &str) -> String {
    let at = |a: f64, z: f64| {
        let (x, y, z) = wall.at(a, off, z);
        v.p(x, y, z)
    };
    poly(class, &[at(a0, z0), at(a1, z0), at(a1, z1), at(a0, z1)])
}

/// Text laid on the floor, reading along the view's u axis.
fn floor_text(v: View, x: f64, y: f64, text: &str) -> String {
    let (sx, sy) = v.p(x, y, 0.0);
    format!(
        r#"<text class="paint" transform="matrix(1 0.5 -1 0.5 {sx:.1} {sy:.1})" text-anchor="middle">{text}</text>"#
    )
}

/// A "+" lying at a point.
fn plus(v: View, x: f64, y: f64, z: f64) -> String {
    let (cx, cy) = v.p(x, y, z);
    format!(
        r#"<g class="plus" transform="translate({cx:.1} {cy:.1})"><rect x="-1.5" y="-8" width="3" height="16" rx="1.5"/><rect x="-8" y="-1.5" width="16" height="3" rx="1.5"/></g>"#
    )
}

// ---------- drawing order ----------

/// The parts of one thing, drawn back to front for the view they are seen
/// in. Its shadows go first.
struct Group {
    v: View,
    shade: String,
    parts: Vec<(Fp, String)>,
}

impl Group {
    fn new(v: View) -> Group {
        Group {
            v,
            shade: String::new(),
            parts: Vec::new(),
        }
    }

    fn shade(&mut self, svg: &str) {
        self.shade.push_str(svg);
    }

    fn put(&mut self, f: Fp, svg: String) {
        self.parts.push((f, svg));
    }

    /// The parts in an order that draws what is behind first. Parts that
    /// don't overlap on the screen stay in the order they were put.
    fn finish(self) -> String {
        let n = self.parts.len();
        let boxes: Vec<[f64; 4]> = self.parts.iter().map(|(f, _)| self.v.bbox(f)).collect();
        let mut waits = vec![0usize; n];
        let mut then: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, (a, _)) in self.parts.iter().enumerate() {
            for (j, (b, _)) in self.parts.iter().enumerate() {
                let (p, q) = (boxes[i], boxes[j]);
                let apart = p[0] >= q[2] || q[0] >= p[2] || p[1] >= q[3] || q[1] >= p[3];
                if i != j && !apart && self.v.before(a, b) && !self.v.before(b, a) {
                    then[i].push(j);
                    waits[j] += 1;
                }
            }
        }
        let mut done = vec![false; n];
        let mut out = self.shade;
        for _ in 0..n {
            let next = (0..n)
                .find(|i| !done[*i] && waits[*i] == 0)
                .or_else(|| (0..n).find(|i| !done[*i]));
            let Some(i) = next else { break };
            done[i] = true;
            for j in &then[i] {
                waits[*j] = waits[*j].saturating_sub(1);
            }
            out.push_str(&self.parts[i].1);
        }
        out
    }
}

// ---------- people ----------

/// How a figure is placed on the floor.
#[derive(Clone, Copy, PartialEq)]
enum Place {
    /// With the transform attribute.
    Attr,
    /// With a style, so the page can animate it walking.
    Style,
    /// Not at all: something else moves it.
    None,
}

struct Fig {
    x: f64,
    y: f64,
    z: f64,
    role: Role,
    /// What the figure's looks are picked from.
    seed: String,
    pose: &'static str,
    state: &'static str,
    place: Place,
    id: Option<String>,
    /// The selection key and the label that make it a control.
    sel: Option<(String, String)>,
    long_hair: bool,
}

impl Fig {
    fn new(x: f64, y: f64, z: f64, role: Role, seed: &str, pose: &'static str) -> Fig {
        Fig {
            x,
            y,
            z,
            role,
            seed: seed.to_string(),
            pose,
            state: "",
            place: Place::Attr,
            id: None,
            sel: None,
            long_hair: role == Role::Designer,
        }
    }

    fn state(mut self, state: &'static str) -> Fig {
        self.state = state;
        self
    }

    fn place(mut self, place: Place) -> Fig {
        self.place = place;
        self
    }

    fn control(mut self, sel: String, label: String) -> Fig {
        self.sel = Some((sel, label));
        self
    }

    fn id(mut self, id: String) -> Fig {
        self.id = Some(id);
        self
    }
}

/// A skin and a hair tone, from a name.
fn looks(seed: &str) -> (char, char) {
    let h = seed
        .bytes()
        .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    let pick = |n: u32| "abcd".chars().nth((n % 4) as usize).unwrap_or('a');
    (pick(h), pick(h / 4))
}

/// A small figure. Poses: sit-back (seen from behind at a desk), sit-front,
/// sit-side, stand-back, stand-front and stand-side.
fn person(v: View, f: &Fig) -> String {
    let (sx, sy) = v.p(f.x, f.y, f.z);
    let sit = f.pose.starts_with("sit");
    let back = f.pose.ends_with("back");
    let side = f.pose.ends_with("side");
    let (skin, hair) = looks(&f.seed);
    let cls = format!(
        "agent {} s-{skin} h-{hair} {} {}",
        f.role.class(),
        f.pose,
        f.state
    );
    let mut attrs = String::new();
    if let Some(id) = &f.id {
        attrs.push_str(&format!(r#" id="{}""#, esc(id)));
    }
    match f.place {
        Place::Attr => attrs.push_str(&format!(r#" transform="translate({sx:.1} {sy:.1})""#)),
        Place::Style => attrs.push_str(&format!(
            r#" style="transform: translate({sx:.1}px, {sy:.1}px)""#
        )),
        Place::None => {}
    }
    if let Some((sel, label)) = &f.sel {
        attrs.push(' ');
        attrs.push_str(&control(sel, label));
    }
    let mut g = format!(r#"<g class="{}"{attrs}>"#, cls.trim());
    g.push_str(r#"<ellipse class="shade" cx="0" cy="2" rx="13" ry="5.5"/>"#);
    let body_y: f64 = if sit {
        -21.0
    } else {
        g.push_str(
            r#"<rect class="leg" x="-6" y="-14" width="5" height="15" rx="2"/><rect class="leg" x="1" y="-14" width="5" height="15" rx="2"/>"#,
        );
        -33.0
    };
    g.push_str(&format!(
        r#"<rect class="body" x="-9" y="{body_y}" width="18" height="21" rx="7"/>"#
    ));
    if back {
        g.push_str(&format!(
            r#"<rect class="arm a1" x="5" y="{}" width="13" height="5" rx="2.5"/>"#,
            body_y + 6.0
        ));
        g.push_str(&format!(
            r#"<rect class="arm a2" x="-4" y="{}" width="13" height="5" rx="2.5"/>"#,
            body_y + 9.0
        ));
    } else if side {
        g.push_str(&format!(
            r#"<rect class="arm a1" x="-15" y="{}" width="12" height="5" rx="2.5"/>"#,
            body_y + 4.0
        ));
    } else {
        g.push_str(&format!(
            r#"<rect class="arm a1" x="-13" y="{0}" width="6" height="12" rx="3"/><rect class="arm a2" x="7" y="{0}" width="6" height="12" rx="3"/>"#,
            body_y + 8.0
        ));
    }
    let hy = body_y - 8.0;
    g.push_str(&format!(
        r#"<circle class="head" cx="0" cy="{hy}" r="8.5"/>"#
    ));
    if back {
        g.push_str(&format!(
            r#"<path class="hair" d="M-8.6 {} a8.6 8.6 0 1 1 17.2 0 q-1 5 -3 6.5 h-11.2 q-2 -1.5 -3 -6.5z"/>"#,
            hy + 1.0
        ));
    } else if side {
        g.push_str(&format!(
            r#"<path class="hair" d="M-8.6 {hy} a8.6 8.6 0 0 1 17.2 0 q-3 -2 -6 -1.5 q-3 3 -11.2 1.5z"/>"#
        ));
        g.push_str(&format!(
            r#"<circle class="eye" cx="-5" cy="{}" r="1.1"/>"#,
            hy + 1.0
        ));
    } else {
        if f.long_hair {
            g.push_str(&format!(
                r#"<path class="hair" d="M-9.2 {} a9.2 9.2 0 1 1 18.4 0 v-3 q-4 -5 -9.2 -4.5 q-5 0 -9.2 4.5z"/>"#,
                hy + 6.0
            ));
        } else {
            g.push_str(&format!(
                r#"<path class="hair" d="M-8.8 {hy} a8.8 8.8 0 0 1 17.6 0 q-4 -3.2 -8.8 -3 q-5 .2 -8.8 3z"/>"#
            ));
        }
        let ey = hy + 1.0;
        g.push_str(&format!(
            r#"<circle class="eye" cx="-3" cy="{ey}" r="1.1"/><circle class="eye" cx="3" cy="{ey}" r="1.1"/>"#
        ));
    }
    g.push_str("</g>");
    g
}

fn doing_state(d: &Doing) -> &'static str {
    match d {
        Doing::Working => "working",
        Doing::Asking(_) => "asking",
        Doing::Held(_) => "held",
    }
}

fn doing_screen(d: &Doing) -> &'static str {
    match d {
        Doing::Working => "on",
        Doing::Asking(_) => "ask",
        Doing::Held(_) => "off",
    }
}

fn doing_words(d: &Doing) -> String {
    match d {
        Doing::Working => "working".to_string(),
        Doing::Asking(what) if what.trim().is_empty() => "waiting for you".to_string(),
        Doing::Asking(what) => format!("waiting for you: {}", what.trim()),
        Doing::Held(why) if why.trim().is_empty() => "on hold".to_string(),
        Doing::Held(why) => format!("on hold: {}", why.trim()),
    }
}

/// A ticket's figure: a control that the page can walk to another room.
fn ticket_fig(w: &Worker, x: f64, y: f64, z: f64, pose: &'static str, state: &'static str) -> Fig {
    let label = format!("{}, {}, {}", w.ticket, w.title, doing_words(&w.doing));
    Fig::new(x, y, z, w.role, &w.hire, pose)
        .state(state)
        .place(Place::Style)
        .id(format!("w-{}", safe(&w.ticket)))
        .control(key("tickets", &w.ticket), label)
}

fn minutes(n: u64) -> String {
    if n == 1 {
        "1 minute".to_string()
    } else {
        format!("{n} minutes")
    }
}

// ---------- furniture ----------

/// Facing the back wall at t = 0: a desk whose monitor faces the worker,
/// who sits on the +y side.
fn desk(g: &mut Group, x: f64, y: f64, screen: &str) {
    let v = g.v;
    g.shade(&shadow(v, x + 0.8, y + 0.5, 1.05, 0.4));
    let mut part = |f: Fp, mat: &str| {
        let s = bx(
            v,
            f.x0,
            f.y0,
            f.z0,
            f.x1 - f.x0,
            f.y1 - f.y0,
            f.z1 - f.z0,
            mat,
            "",
        );
        g.put(f, s);
    };
    part(fp(x, y, 0.0, 1.6, 0.8, 0.07), "m-deskleg");
    part(fp(x + 0.05, y + 0.05, 0.0, 0.08, 0.7, 0.72), "m-deskleg");
    part(fp(x + 1.47, y + 0.05, 0.0, 0.08, 0.7, 0.72), "m-deskleg");
    part(fp(x, y, 0.72, 1.6, 0.8, 0.07), "m-desk");
    part(fp(x + 0.75, y + 0.15, 0.79, 0.12, 0.08, 0.16), "m-metal");
    let monitor = fp(x + 0.35, y + 0.12, 0.95, 0.95, 0.07, 0.62);
    part(monitor, "m-monitor");
    // The screen faces the worker: it shows only from that side.
    if v.toward(0.0, 1.0) {
        let screen_fp = Fp {
            z0: 0.96,
            ..monitor
        };
        g.put(
            screen_fp,
            poly(
                &format!("screen {screen}"),
                &[
                    v.p(x + 0.42, y + 0.19, 1.0),
                    v.p(x + 1.23, y + 0.19, 1.0),
                    v.p(x + 1.23, y + 0.19, 1.52),
                    v.p(x + 0.42, y + 0.19, 1.52),
                ],
            ),
        );
        if screen == "on" || screen == "ask" {
            let mut lines = String::new();
            for (i, frac) in [0.7, 0.45, 0.85, 0.3, 0.6, 0.75, 0.4].iter().enumerate() {
                let z = 1.44 - i as f64 * 0.075;
                lines.push_str(&line(
                    v.p(x + 0.48, y + 0.19, z),
                    v.p(x + 0.48 + 0.7 * frac, y + 0.19, z),
                ));
            }
            let code_fp = Fp {
                z0: 0.961,
                ..monitor
            };
            g.put(code_fp, format!(r#"<g class="code">{lines}</g>"#));
        }
    }
    g.put(
        fp(x + 0.15, y + 0.35, 0.79, 0.32, 0.16, 0.02),
        bx(v, x + 0.15, y + 0.35, 0.79, 0.32, 0.16, 0.02, "m-keys", ""),
    );
}

fn chair(g: &mut Group, x: f64, y: f64) {
    let v = g.v;
    g.shade(&shadow(v, x, y, 0.42, 0.22));
    g.put(
        fp(x - 0.28, y - 0.28, 0.0, 0.56, 0.56, 0.4),
        bx(v, x - 0.28, y - 0.28, 0.0, 0.56, 0.56, 0.4, "m-chair", ""),
    );
}

fn plant(v: View, x: f64, y: f64, size: f64) -> String {
    let (sx, sy) = v.p(x, y, 0.42 * size);
    let r = 15.0 * size;
    let mut s = shadow(v, x, y, 0.45 * size, 0.22 * size);
    s.push_str(&bx(
        v,
        x - 0.22 * size,
        y - 0.22 * size,
        0.0,
        0.44 * size,
        0.44 * size,
        0.42 * size,
        "m-pot",
        "",
    ));
    s.push_str(&format!(
        r#"<g class="leaves"><circle cx="{:.1}" cy="{:.1}" r="{:.1}"/><circle cx="{:.1}" cy="{:.1}" r="{:.1}"/><circle cx="{sx:.1}" cy="{:.1}" r="{:.1}"/></g>"#,
        sx - 6.0 * size,
        sy - 10.0 * size,
        r * 0.62,
        sx + 6.0 * size,
        sy - 12.0 * size,
        r * 0.7,
        sy - 22.0 * size,
        r * 0.66,
    ));
    s
}

/// What a plant takes up.
fn plant_fp(x: f64, y: f64, size: f64) -> Fp {
    fp(
        x - 0.22 * size,
        y - 0.22 * size,
        0.0,
        0.44 * size,
        0.44 * size,
        1.2 * size,
    )
}

/// A crate for a ticket: a control, taped in its project's colour. The
/// tape is on the face that shows.
fn crate_box(
    v: View,
    floor: &Floor,
    c: &Crate,
    (x, y, z): (f64, f64, f64),
    mat: &str,
    what: &str,
) -> String {
    let pj = floor.tint(&c.project);
    let extra = format!(
        " {}",
        control(&key("tickets", &c.ticket), &format!("{}, {what}", c.ticket))
    );
    let tape = vface(
        v,
        (x, y, 0.62, 0.62),
        (0.12, 0.5),
        (z + 0.18, z + 0.32),
        &format!("tape pj-{pj}"),
    );
    format!(
        r#"<g data-pj="{pj}">{}{tape}</g>"#,
        bx(v, x, y, z, 0.62, 0.62, 0.5, mat, &extra)
    )
}

// ---------- the drawing ----------

/// Which pass a drawn piece belongs to: the ground, the walls and what is
/// mounted on them, the objects in the room, then what lies over all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Layer {
    Ground,
    Wall,
    Object,
    Overlay,
}

/// Where a piece goes in the order: its layer and its depth.
type Slot = (Layer, f64);

struct Drawing<'a> {
    floor: &'a Floor,
    v: View,
    /// Everything drawn, in the order it was put.
    items: Vec<(Slot, String)>,
    pins: Vec<Pin>,
    hints: Vec<Pin>,
}

/// Something mounted on a wall: on it when the wall is at the back, on two
/// legs a little into the room when it is at the front.
struct Mount {
    wall: Wall,
    /// How far into the room: 0 on the wall.
    off: f64,
    slot: Slot,
    standing: bool,
}

impl Mount {
    /// A rectangle on the board.
    fn rect(&self, v: View, a: Pt, z: Pt, class: &str) -> String {
        wall_rect(v, self.wall, self.off, a, z, class)
    }

    /// The legs of a board from `a0` to `a1`, `shift` further into the room,
    /// up to height `top`: none while it is on the wall.
    fn legs(&self, v: View, (a0, a1): Pt, shift: f64, top: f64) -> String {
        if !self.standing {
            return String::new();
        }
        let mut s = String::new();
        for a in [a0 + 0.06, a1 - 0.14] {
            let f = self
                .wall
                .fp(self.off + shift, (a, a + 0.08), 0.08, 0.0, top);
            s.push_str(&bx(
                v,
                f.x0,
                f.y0,
                0.0,
                f.x1 - f.x0,
                f.y1 - f.y0,
                top,
                "m-metal",
                "",
            ));
        }
        s
    }

    /// A point `a` along the board and `z` up, for a hint.
    fn at(&self, a: f64, z: f64) -> (f64, f64, f64) {
        self.wall.at(a, self.off, z)
    }
}

impl Drawing<'_> {
    fn put(&mut self, slot: Slot, svg: String) {
        self.items.push((slot, svg));
    }

    /// The slot of an object in the room.
    fn obj(&self, f: &Fp) -> Slot {
        (Layer::Object, self.v.depth(f))
    }

    /// The slot of something that stands with `host` and is drawn after it
    /// unless it is behind it.
    fn with(&self, f: &Fp, host: &Fp) -> Slot {
        (Layer::Object, self.v.depth_on(f, host))
    }

    /// The slot of something that is always drawn after `host`.
    fn after(&self, f: &Fp, host: &Fp) -> Slot {
        let at = self.v.depth(host) + NUDGE;
        (Layer::Object, self.v.depth(f).max(at))
    }

    /// A board on `wall` from `a0` to `a1`.
    fn mount(&self, wall: Wall, span: Pt) -> Mount {
        if self.v.at_back(wall) {
            return Mount {
                wall,
                off: 0.0,
                slot: (Layer::Wall, 0.0),
                standing: false,
            };
        }
        let f = wall.fp(STAND, span, 0.1, 0.0, 2.5);
        Mount {
            wall,
            off: STAND,
            slot: self.obj(&f),
            standing: true,
        }
    }

    fn pin(&mut self, x: f64, y: f64, z: f64, what: PinWhat) {
        let (left, top) = self.v.pct(x, y, z);
        self.pins.push(Pin { left, top, what });
    }

    /// A pin moved by (du, dv) in view space from the point it is for.
    fn pin_off(&mut self, at: (f64, f64, f64), off: Pt, what: PinWhat) {
        let (left, top) = self.v.pct_off(at, off);
        self.pins.push(Pin { left, top, what });
    }

    /// Draws a game element as a control, with its hint pinned at (x, y, z).
    fn el(&mut self, e: Element, slot: Slot, inner: &str, label: &str, at: (f64, f64, f64)) {
        let k = e.key();
        let sel = format!("elements/{k}");
        self.put(
            slot,
            format!(
                r#"<g class="el" data-el="{k}" {}>{inner}</g>"#,
                control(&sel, label)
            ),
        );
        let (left, top) = self.v.pct(at.0, at.1, at.2);
        self.hints.push(Pin {
            left,
            top,
            what: PinWhat::Hint(e),
        });
    }

    fn tint(&self, project: &str) -> usize {
        self.floor.tint(project)
    }

    /// Wraps `inner` in a group for `project`.
    fn pj_group(&self, project: &str, inner: &str) -> String {
        format!(r#"<g data-pj="{}">{inner}</g>"#, self.tint(project))
    }

    /// The drawing, ground first, then walls, objects by depth and
    /// overlays; pieces at the same depth stay as they were put.
    fn svg(mut self) -> (String, Vec<Pin>) {
        self.items
            .sort_by(|a, b| a.0.0.cmp(&b.0.0).then(a.0.1.total_cmp(&b.0.1)));
        let mut s = String::new();
        for (_, svg) in &self.items {
            s.push_str(svg);
        }
        let mut pins = self.pins;
        pins.append(&mut self.hints);
        (s, pins)
    }
}

/// Draws `floor`, seen from the corner `turn` sets.
pub fn render(floor: &Floor, turn: Turn) -> Scene {
    let v = View::new(turn);
    let mut d = Drawing {
        floor,
        v,
        items: Vec::new(),
        pins: Vec::new(),
        hints: Vec::new(),
    };
    shell(&mut d);
    planning(&mut d);
    checks_lab(&mut d);
    workshop(&mut d);
    lounge(&mut d);
    review(&mut d);
    front_desk(&mut d);
    shipping(&mut d);
    if floor.queued > 0 {
        d.put((Layer::Overlay, 0.0), walker(v));
    }
    let (body, pins) = d.svg();
    let svg = format!(
        r#"<svg class="iso" viewBox="0 0 {VW:.0} {VH:.0}" role="img" aria-label="The studio floor: planning, workshop, checks lab, review, shipping, lounge and front desk">{DEFS}{body}</svg>"#
    );
    Scene {
        width: VW.round(),
        height: VH.round(),
        svg,
        pins,
    }
}

const DEFS: &str = concat!(
    "<defs>",
    r##"<radialGradient id="halo" cx="50%" cy="50%" r="50%"><stop offset="0" stop-color="#7aa7ff" stop-opacity=".16"/><stop offset="1" stop-color="#7aa7ff" stop-opacity="0"/></radialGradient>"##,
    r##"<radialGradient id="pool" cx="50%" cy="50%" r="50%"><stop offset="0" stop-color="var(--pool)" stop-opacity="1"/><stop offset="1" stop-color="var(--pool)" stop-opacity="0"/></radialGradient>"##,
    r##"<pattern id="hz" width="10" height="10" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><rect width="5" height="10" fill="#f5b400"/><rect x="5" width="5" height="10" fill="#22252b"/></pattern>"##,
    "</defs>"
);

const GROUND: Slot = (Layer::Ground, 0.0);
const WALL: Slot = (Layer::Wall, 0.0);

/// The slab, the floors, the walls and the windows.
fn shell(d: &mut Drawing) {
    let v = d.v;
    d.put(
        GROUND,
        poly(
            "glow",
            &[
                v.p(-3.0, -3.0, 0.0),
                v.p(W + 3.0, -3.0, 0.0),
                v.p(W + 3.0, D + 3.0, 0.0),
                v.p(-3.0, D + 3.0, 0.0),
            ],
        ),
    );
    d.put(GROUND, bx(v, 0.0, 0.0, -0.75, W, D, 0.75, "m-slab", ""));
    for (x0, y0, x1, y1, cls) in [
        (0.0, 0.0, 7.0, 5.0, "fl-plan"),
        (7.0, 0.0, 16.0, 8.0, "fl-work"),
        (16.0, 0.0, W, 6.0, "fl-lab"),
        (0.0, 5.0, 7.0, D, "fl-lounge"),
        (7.0, 8.0, 12.0, D, "fl-front"),
        (12.0, 8.0, 16.0, D, "fl-dock"),
        (16.0, 6.0, W, D, "fl-review"),
    ] {
        d.put(GROUND, flat(v, x0, y0, x1, y1, &format!("fl {cls}")));
    }
    // Planks in the workshop and the lounge, tiles in the lab and the front.
    let mut planks = String::new();
    for k in 1..16 {
        let yy = f64::from(k) * 0.5;
        if yy < 8.0 {
            planks.push_str(&line(v.p(7.0, yy, 0.0), v.p(16.0, yy, 0.0)));
        }
    }
    for k in 11..28 {
        let yy = f64::from(k) * 0.5;
        if yy > 5.0 && yy < D {
            planks.push_str(&line(v.p(0.0, yy, 0.0), v.p(7.0, yy, 0.0)));
        }
    }
    d.put(GROUND, format!(r#"<g class="planks">{planks}</g>"#));
    let mut tiles = String::new();
    for k in 17..22 {
        let k = f64::from(k);
        tiles.push_str(&line(v.p(k, 0.0, 0.0), v.p(k, 6.0, 0.0)));
    }
    for k in 1..6 {
        let k = f64::from(k);
        tiles.push_str(&line(v.p(16.0, k, 0.0), v.p(W, k, 0.0)));
    }
    for k in 8..12 {
        let k = f64::from(k);
        tiles.push_str(&line(v.p(k, 8.0, 0.0), v.p(k, D, 0.0)));
    }
    for k in 9..14 {
        let k = f64::from(k);
        tiles.push_str(&line(v.p(7.0, k, 0.0), v.p(12.0, k, 0.0)));
    }
    d.put(GROUND, format!(r#"<g class="tiles">{tiles}</g>"#));
    // Rugs and floor marks.
    d.put(GROUND, flat(v, 1.4, 7.0, 5.6, 12.4, "rug"));
    d.put(GROUND, flat(v, 17.6, 8.4, 20.6, 11.8, "rug rug-2"));
    d.put(GROUND, flat(v, 12.2, 13.2, 15.8, 13.85, "hazard"));
    d.put(GROUND, flat(v, 12.2, 8.15, 15.8, 8.8, "lane"));
    for (x, y, text) in [
        (3.5, 4.55, "PLANNING"),
        (11.5, 7.55, "WORKSHOP"),
        (19.0, 5.55, "CHECKS LAB"),
        (3.5, 13.55, "LOUNGE"),
        (9.5, 13.55, "FRONT DESK"),
        (14.0, 12.9, "SHIPPING"),
        (19.0, 13.55, "REVIEW"),
    ] {
        d.put(GROUND, floor_text(v, x, y, text));
    }
    // Light pools, seen at night.
    for (x, y, r) in [
        (9.2, 2.6, 2.4),
        (12.8, 2.6, 2.4),
        (9.2, 5.6, 2.4),
        (12.8, 5.6, 2.4),
        (3.4, 2.4, 2.6),
        (19.0, 3.0, 2.4),
        (19.0, 10.0, 2.6),
        (3.4, 9.6, 3.0),
        (9.6, 11.0, 2.2),
        (14.0, 11.0, 2.2),
    ] {
        let (sx, sy) = v.p(x, y, 0.0);
        d.put(
            GROUND,
            format!(
                r#"<ellipse class="pool" cx="{sx:.1}" cy="{sy:.1}" rx="{:.1}" ry="{:.1}"/>"#,
                r * HX * 1.4,
                r * HX * 0.7
            ),
        );
    }
    // The two walls at the back, with windows.
    let (uw, vw) = v.walls();
    let (us, vs) = v.size();
    d.put(
        WALL,
        bx_view(v, (-0.3, 0.0), (-0.3, vs), -0.75, 3.65, "m-wall"),
    );
    d.put(
        WALL,
        bx_view(v, (0.0, us), (-0.3, 0.0), -0.75, 3.65, "m-wall"),
    );
    for wall in [vw, uw] {
        d.put(WALL, windows(v, wall));
    }
    // City lights in the windows at night, clouds by day.
    let mut city = String::new();
    for wall in [vw, uw] {
        city.push_str(&towers(v, wall));
    }
    d.put(WALL, format!(r#"<g class="city">{city}</g>"#));
}

/// The windows of a wall, in the same places on every wall of its kind.
fn windows(v: View, wall: Wall) -> String {
    let mut s = String::new();
    let (starts, width, mullion): (&[f64], f64, f64) = if wall.along_x() {
        (&[7.6, 10.6, 13.6], 2.2, 1.08)
    } else {
        (&[6.2, 9.6], 2.4, 1.17)
    };
    for a0 in starts {
        s.push_str(&wall_rect(
            v,
            wall,
            0.0,
            (*a0, a0 + width),
            (1.15, 2.55),
            "window",
        ));
        s.push_str(&wall_rect(
            v,
            wall,
            0.0,
            (a0 + mullion, a0 + mullion + 0.06),
            (1.15, 2.55),
            "mullion",
        ));
    }
    s
}

/// The skyline seen through a wall's windows.
fn towers(v: View, wall: Wall) -> String {
    let mut s = String::new();
    if wall.along_x() {
        for a0 in [7.6, 10.6, 13.6] {
            for (da, z1) in [
                (0.2, 1.6),
                (0.5, 1.9),
                (0.85, 1.45),
                (1.3, 2.05),
                (1.7, 1.7),
            ] {
                s.push_str(&wall_rect(
                    v,
                    wall,
                    0.0,
                    (a0 + da, a0 + da + 0.22),
                    (1.15, z1),
                    "tower",
                ));
            }
        }
    } else {
        for a0 in [6.2, 9.6] {
            for (da, z1) in [(0.25, 1.75), (0.7, 1.5), (1.4, 2.0), (1.9, 1.6)] {
                s.push_str(&wall_rect(
                    v,
                    wall,
                    0.0,
                    (a0 + da, a0 + da + 0.24),
                    (1.15, z1),
                    "tower",
                ));
            }
        }
    }
    s
}

/// Where hire cards go on the team board: along, and the top of the card.
const CARDS: [(f64, f64); MAX_CARDS] = [
    (4.95, 1.95),
    (5.4, 1.85),
    (5.0, 1.45),
    (5.45, 1.4),
    (4.85, 2.15),
    (5.55, 2.15),
    (5.2, 1.72),
    (5.6, 1.72),
];
/// Where posters hang: the last ones are used first.
const POSTERS: [f64; MAX_POSTERS] = [3.25, 4.2, 5.15, 6.1];

/// The whiteboard with the open goals, the team board with a card for each
/// hire, the posters for the house rules and the people planning.
fn planning(d: &mut Drawing) {
    let v = d.v;
    let wb = d.mount(Wall::West, (0.5, 4.6));
    let inner = wb.legs(v, (0.5, 4.6), 0.0, 0.95) + &wb.rect(v, (0.5, 4.6), (0.95, 2.45), "board");
    d.el(
        Element::Board,
        wb.slot,
        &inner,
        "Whiteboard: goals",
        wb.at(2.6, 2.75),
    );

    let cb = d.mount(Wall::West, (4.8, 5.95));
    let mut cork =
        cb.legs(v, (4.8, 5.95), 0.0, 1.05) + &cb.rect(v, (4.8, 5.95), (1.05, 2.2), "cork");
    for (i, (a, top)) in CARDS.iter().take(d.floor.hires.min(MAX_CARDS)).enumerate() {
        cork.push_str(&cb.rect(
            v,
            (*a, a + 0.35),
            (top - 0.3, *top),
            &format!("card c{}", i % 3 + 1),
        ));
    }
    d.el(
        Element::Corkboard,
        cb.slot,
        &cork,
        "Team board: the team and hiring",
        cb.at(5.4, 2.45),
    );

    // One poster per rule file; with none, a blank frame.
    let rules = d.floor.rules.min(MAX_POSTERS);
    let first = POSTERS
        .get(MAX_POSTERS - rules.max(1))
        .copied()
        .unwrap_or(6.1);
    let span = (first, 6.85);
    let pb = d.mount(Wall::North, span);
    let mut posters = pb.legs(v, span, 0.0, 1.05);
    if pb.standing {
        posters.push_str(&pb.rect(v, (span.0 - 0.1, span.1 + 0.1), (1.05, 2.35), "board"));
    }
    if rules == 0 {
        posters.push_str(&pb.rect(v, (6.1, 6.85), (1.15, 2.25), "poster blank"));
    }
    for (i, x0) in POSTERS.iter().skip(MAX_POSTERS - rules).enumerate() {
        posters.push_str(&pb.rect(
            v,
            (*x0, x0 + 0.75),
            (1.15, 2.25),
            &format!("poster p{}", i % 3 + 1),
        ));
        posters.push_str(&pb.rect(v, (x0 + 0.08, x0 + 0.67), (1.95, 2.15), "poster-band"));
        for zz in [1.75, 1.6, 1.45, 1.32] {
            posters.push_str(&pb.rect(v, (x0 + 0.1, x0 + 0.6), (zz - 0.035, zz), "poster-line"));
        }
    }
    d.el(
        Element::Posters,
        pb.slot,
        &posters,
        "House rules",
        pb.at((span.0 + span.1) / 2.0, 2.5),
    );

    for (i, n) in d.floor.goals.iter().take(6).enumerate() {
        let note = goal_note(d, n, i, &wb);
        d.put(wb.slot, note);
    }
    let table = fp(2.8, 1.6, 0.0, 1.4, 1.0, 0.9);
    let slot = d.obj(&table);
    d.put(slot, bx(v, 2.8, 1.6, 0.0, 1.4, 1.0, 0.9, "m-desk", "")); // a standing table
    for (i, who) in d.floor.planners.iter().take(3).enumerate() {
        let y = [2.7, 3.7, 1.7].get(i).copied().unwrap_or(2.7);
        let fig = Fig::new(
            1.35,
            y,
            0.0,
            who.role,
            &who.hire,
            v.pose(false, (-1.0, 0.0), true),
        )
        .state("writing")
        .control(key("hires", &who.hire), format!("{}, planning", who.hire));
        let slot = d.obj(&spot(1.35, y, 0.0));
        d.put(slot, person(v, &fig));
    }
    if !d.floor.planners.is_empty() {
        let unplanned = d.floor.goals.iter().filter(|g| g.total == 0).count();
        let n = if unplanned > 0 {
            unplanned
        } else {
            d.floor.planners.len()
        };
        d.pin(1.6, 2.6, 2.75, PinWhat::Planning(n));
    }
}

/// One goal's note on the whiteboard, with its bar of landed over total.
fn goal_note(d: &Drawing, n: &Note, i: usize, board: &Mount) -> String {
    let v = d.v;
    let y0 = [0.75, 1.75, 2.75].get(i % 3).copied().unwrap_or(0.75);
    let z1 = if i < 3 { 2.05 } else { 1.42 };
    let pj = d.tint(&n.project);
    let done = if n.done { " done" } else { "" };
    let label = if n.total == 0 {
        format!("{}, no tickets yet", n.title)
    } else {
        format!("{}, {} of {} landed", n.title, n.landed, n.total)
    };
    let mut inner = board.rect(
        v,
        (y0, y0 + 0.82),
        (z1 - 0.5, z1),
        &format!("note pj-{pj}{done}"),
    );
    if n.total > 0 && n.landed > 0 {
        let frac = (n.landed as f64 / n.total as f64).clamp(0.0, 1.0);
        inner.push_str(&board.rect(
            v,
            (y0 + 0.08, y0 + 0.08 + 0.66 * frac),
            (z1 - 0.42, z1 - 0.37),
            "note-bar",
        ));
    }
    format!(
        r#"<g class="note-hit" data-pj="{pj}" {}>{inner}</g>"#,
        control(&n.key, &label)
    )
}

/// Where the racks of a lab with this many slots stand: the first one's
/// place along x, the step between them and a rack's width.
fn rack_layout(slots: usize) -> (f64, f64, f64) {
    if slots <= 4 {
        (16.5, 1.2, 1.0)
    } else {
        (16.2, 0.95, 0.8)
    }
}

/// A rack for each project, a free slot after them to add one, and the
/// console where checks run.
fn checks_lab(d: &mut Drawing) {
    let v = d.v;
    let projects: Vec<String> = d.floor.projects.iter().take(MAX_RACKS).cloned().collect();
    let n = projects.len();
    let (start, pitch, w) = rack_layout(n + 1);
    if n > 0 {
        let mut g = Group::new(v);
        for (i, name) in projects.iter().enumerate() {
            let x0 = start + pitch * i as f64;
            g.put(fp(x0, 0.15, 0.0, w, 0.9, 2.3), rack(d, name, x0, w, i));
        }
        let span = pitch * (n - 1) as f64 + w;
        let racks_fp = fp(start, 0.15, 0.0, span, 0.9, 2.3);
        let slot = d.obj(&racks_fp);
        d.el(
            Element::Racks,
            slot,
            &g.finish(),
            "Checks lab: every project's checks",
            (start + span / 2.0, 0.6, 2.75),
        );
    } else {
        // No project yet: a bench stands where the racks will.
        let bench = bx(v, 20.4, 0.2, 0.0, 1.3, 0.7, 0.8, "m-desk", "")
            + &bx(v, 20.75, 0.3, 0.8, 0.6, 0.35, 0.1, "m-metal", "");
        let slot = d.obj(&fp(20.4, 0.2, 0.0, 1.3, 0.7, 0.9));
        d.el(
            Element::Racks,
            slot,
            &bench,
            "Checks lab: every project's checks",
            (21.05, 0.55, 1.3),
        );
    }
    // The free slot after the last rack: an outline of the rack to come.
    let x0 = start + pitch * n as f64;
    let ghost = format!(
        "{}{}",
        ghost_box(v, (x0, 0.15, w, 0.9), 0.0, 2.3),
        plus(v, x0 + w / 2.0, 0.6, 1.15)
    );
    let slot = d.obj(&fp(x0, 0.15, 0.0, w, 0.9, 2.3));
    d.el(
        Element::AddProject,
        slot,
        &ghost,
        "Add a project",
        (x0 + w / 2.0, 0.6, 2.4),
    );

    let first = d.floor.checks.first();
    let screen = first.map_or("off", |w| doing_screen(&w.doing));
    let mut g = Group::new(v);
    desk(&mut g, 17.6, 2.8, screen);
    let spots = [(18.5, 4.35), (20.9, 3.7), (19.9, 5.3)];
    for (i, (w, (x, y))) in d.floor.checks.iter().zip(spots).enumerate() {
        let fig = ticket_fig(
            w,
            x,
            y,
            0.0,
            v.pose(false, (0.0, -1.0), false),
            doing_state(&w.doing),
        );
        let fig = person(v, &fig);
        let fig = if i == 0 {
            fig
        } else {
            d.pj_group(&w.project, &fig)
        };
        g.put(spot(x, y, 0.0), fig);
        let lift = if i == 2 { 0.8 } else { 0.0 };
        d.pin_off(
            (x, y, 2.5 + lift),
            (-0.1, -1.15),
            PinWhat::Ticket(w.clone()),
        );
    }
    let station = g.finish();
    let station = match first {
        Some(w) => d.pj_group(&w.project, &station),
        None => station,
    };
    let slot = d.obj(&fp(17.6, 2.8, 0.0, 3.5, 2.7, 1.6));
    d.put(slot, station);
    let slot = d.obj(&plant_fp(21.4, 5.3, 0.9));
    d.put(slot, plant(v, 21.4, 5.3, 0.9));
}

/// One project's rack: the box, its lights where it faces the room, and a
/// stripe in the project's colour.
fn rack(d: &Drawing, project: &str, x0: f64, w: f64, i: usize) -> String {
    let v = d.v;
    let pj = d.tint(project);
    let mut s = bx(v, x0, 0.15, 0.0, w, 0.9, 2.3, "m-rack", "");
    // The lights and the band on the face that looks into the room.
    if v.toward(0.0, 1.0) {
        let mut leds = String::new();
        for row in 0..6 {
            for col in 0..3 {
                let a = v.p(
                    x0 + 0.2 * w + f64::from(col) * 0.18 * w,
                    1.05,
                    2.0 - f64::from(row) * 0.3,
                );
                leds.push_str(&format!(
                    r#"<circle class="led l{}" cx="{:.1}" cy="{:.1}" r="1.8"/>"#,
                    (row + col + i as i32) % 3,
                    a.0,
                    a.1
                ));
            }
        }
        s.push_str(&format!(r#"<g class="leds">{leds}</g>"#));
        s.push_str(&poly(
            &format!("stripe pj-{pj}"),
            &[
                v.p(x0 + 0.1 * w, 1.05, 2.1),
                v.p(x0 + 0.9 * w, 1.05, 2.1),
                v.p(x0 + 0.9 * w, 1.05, 2.22),
                v.p(x0 + 0.1 * w, 1.05, 2.22),
            ],
        ));
    }
    // And along the top, which shows from every side.
    s.push_str(&poly(
        &format!("stripe pj-{pj}"),
        &[
            v.p(x0 + 0.1 * w, 0.72, 2.3),
            v.p(x0 + 0.9 * w, 0.72, 2.3),
            v.p(x0 + 0.9 * w, 0.92, 2.3),
            v.p(x0 + 0.1 * w, 0.92, 2.3),
        ],
    ));
    format!(r#"<g data-pj="{pj}">{s}</g>"#)
}

/// Rows of desks, a worker at each, and a spot for one more.
fn workshop(d: &mut Drawing) {
    let v = d.v;
    let desks = d.floor.desks.len().min(MAX_DESKS);
    for (i, desk_data) in d.floor.desks.iter().take(MAX_DESKS).enumerate() {
        let x = DESK_X.get(i % 3).copied().unwrap_or(8.0);
        let y = DESK_Y.get(i / 3).copied().unwrap_or(6.6);
        let screen = desk_data
            .worker
            .as_ref()
            .map_or("off", |w| doing_screen(&w.doing));
        let mut g = Group::new(v);
        desk(&mut g, x, y, screen);
        chair(&mut g, x + 0.8, y + 1.25);
        let mut attrs = format!(r#" data-desk="{i}""#);
        if let Some(w) = &desk_data.worker {
            // The worker sits facing the desk's screen.
            let fig = ticket_fig(
                w,
                x + 0.8,
                y + 1.25,
                0.42,
                v.pose(true, (0.0, -1.0), false),
                doing_state(&w.doing),
            );
            g.put(spot(x + 0.8, y + 1.25, 0.42), person(v, &fig));
            attrs.push_str(&format!(r#" data-pj="{}""#, d.tint(&w.project)));
            d.pin(x + 0.8, y + 0.4, 2.35, PinWhat::Ticket(w.clone()));
        } else {
            // A free desk leads to the desks, as the "+" spot does.
            attrs.push(' ');
            attrs.push_str(&control(
                &format!("elements/{}", Element::AddDesk.key()),
                "A free desk: add or remove desks",
            ));
        }
        let slot = d.obj(&fp(x, y, 0.0, 1.6, 1.53, 1.6));
        d.put(slot, format!("<g{attrs}>{}</g>", g.finish()));
    }
    if d.floor.room_for_desk && desks < MAX_DESKS {
        let x = DESK_X.get(desks % 3).copied().unwrap_or(8.0);
        let y = DESK_Y.get(desks / 3).copied().unwrap_or(6.6);
        let ghost = format!(
            "{}{}",
            poly(
                "ghost",
                &[
                    v.p(x, y - 0.05, 0.0),
                    v.p(x + 1.6, y - 0.05, 0.0),
                    v.p(x + 1.6, y + 0.85, 0.0),
                    v.p(x, y + 0.85, 0.0),
                ]
            ),
            plus(v, x + 0.8, y + 0.4, 0.0)
        );
        d.el(
            Element::AddDesk,
            GROUND,
            &ghost,
            "Add a desk",
            (x + 0.8, y + 0.4, 0.55),
        );
    }
    let slot = d.obj(&plant_fp(15.4, 0.7, 1.0));
    d.put(slot, plant(v, 15.4, 0.7, 1.0));
    // A printer.
    let mut g = Group::new(v);
    g.put(
        fp(15.0, 6.2, 0.0, 0.8, 0.6, 0.85),
        bx(v, 15.0, 6.2, 0.0, 0.8, 0.6, 0.85, "m-metal", ""),
    );
    g.put(
        fp(15.08, 6.26, 0.85, 0.64, 0.48, 0.12),
        bx(v, 15.08, 6.26, 0.85, 0.64, 0.48, 0.12, "m-paper", ""),
    );
    let slot = d.obj(&fp(15.0, 6.2, 0.0, 0.8, 0.6, 0.97));
    d.put(slot, g.finish());
}

/// Where the cups on the trophy shelf stand along it.
fn cup_spots(n: usize) -> Vec<f64> {
    let step = if n > 3 { 0.29 } else { 0.45 };
    (0..n)
        .map(|i| 13.0 + (i as f64 - (n as f64 - 1.0) / 2.0) * step)
        .collect()
}

/// The sofa with a free seat to hire into, the coffee table, the coffee
/// machine and the trophy shelf.
fn lounge(d: &mut Drawing) {
    let v = d.v;
    let sofa = fp(0.15, 6.1, 0.0, 0.9, 5.5, 0.92);
    let slot = d.obj(&sofa);
    d.el(
        Element::Lounge,
        slot,
        &(bx(v, 0.15, 6.1, 0.0, 0.9, 5.5, 0.42, "m-sofa", "")
            + &bx(v, 0.15, 6.1, 0.42, 0.28, 5.5, 0.5, "m-sofa2", "")),
        "Lounge: who's free",
        (0.6, 8.85, 1.75),
    );
    // Whoever rests sits facing the room.
    let mut latest = slot.1;
    for (i, who) in d.floor.lounge.iter().take(4).enumerate() {
        let y = 7.75 + 1.1 * i as f64;
        let label = match who.idle_minutes {
            Some(m) => format!("{}, resting {}", who.hire, minutes(m)),
            None => format!("{}, resting", who.hire),
        };
        let fig = Fig::new(
            0.75,
            y,
            0.42,
            who.role,
            &who.hire,
            v.pose(true, (1.0, 0.0), false),
        )
        .state("idle")
        .control(key("hires", &who.hire), label);
        let at = d.with(&spot(0.75, y, 0.42), &sofa);
        latest = latest.max(at.1);
        d.put(at, person(v, &fig));
    }
    // From the west the sofa's back is in front of them.
    if v.toward(-1.0, 0.0) && !d.floor.lounge.is_empty() {
        let back = bx(v, 0.15, 6.1, 0.42, 0.28, 5.5, 0.5, "m-sofa2", "");
        d.put(
            (Layer::Object, latest + NUDGE),
            format!(r#"<g pointer-events="none">{back}</g>"#),
        );
    }
    // The free seat at the end of the sofa.
    let seat = format!(
        "{}{}",
        ghost_box(v, (0.45, 6.2, 0.55, 0.9), 0.42, 0.14),
        plus(v, 0.72, 6.65, 0.62)
    );
    d.el(
        Element::Hire,
        (Layer::Object, latest + 2.0 * NUDGE),
        &seat,
        "Hire someone",
        (0.72, 6.65, 1.3),
    );

    let table = fp(2.3, 8.6, 0.0, 1.4, 1.6, 0.36);
    let mut g = Group::new(v);
    g.put(table, bx(v, 2.3, 8.6, 0.0, 1.4, 1.6, 0.36, "m-desk", ""));
    for (x, y) in [(2.7, 9.0), (3.2, 9.6)] {
        let (sx, sy) = v.p(x, y, 0.36);
        g.put(
            spot(x, y, 0.36),
            format!(
                r#"<g class="mug"><rect x="{:.1}" y="{:.1}" width="6" height="7" rx="1.5"/><path class="steam" d="M{:.1} {:.1} q2 -4 0 -8 q-2 -4 0 -8"/></g>"#,
                sx - 3.0,
                sy - 7.0,
                sx - 1.0,
                sy - 9.0
            ),
        );
    }
    let slot = d.obj(&table);
    d.put(slot, g.finish());

    let foot = (0.2, 12.6, 0.9, 1.0);
    let mut coffee = bx(v, 0.2, 12.6, 0.0, 0.9, 1.0, 1.25, "m-coffee", "");
    coffee.push_str(&vface(v, foot, (0.15, 0.55), (0.95, 1.1), "coffee-light"));
    let slot = d.obj(&fp(0.2, 12.6, 0.0, 0.9, 1.0, 1.25));
    d.el(
        Element::Coffee,
        slot,
        &coffee,
        "Coffee machine: take a break and close the office",
        (0.6, 13.1, 1.6),
    );

    // The shelf, with a cup for each trophy earned.
    let m = d.mount(Wall::West, (12.3, 13.9));
    let mut shelf = m.legs(v, (12.3, 13.9), 0.16, 1.72);
    shelf.push_str(&bx(v, m.off, 12.3, 1.72, 0.32, 1.6, 0.06, "m-desk", ""));
    for yy in cup_spots(d.floor.trophies.min(MAX_CUPS)) {
        let (tx, ty) = v.p(m.off + 0.16, yy, 1.78);
        shelf.push_str(&format!(
            r#"<g class="trophy"><rect x="{:.1}" y="{:.1}" width="6" height="3" rx="1"/><rect x="{:.1}" y="{:.1}" width="2" height="5"/><path d="M{:.1} {:.1} h12 v3 a6 6 0 0 1 -12 0z"/></g>"#,
            tx - 3.0,
            ty - 3.0,
            tx - 1.0,
            ty - 8.0,
            tx - 6.0,
            ty - 16.0
        ));
    }
    d.el(
        Element::Trophies,
        m.slot,
        &shelf,
        "Trophy shelf: achievements",
        (m.off + 0.2, 13.05, 2.45),
    );
    let slot = d.obj(&plant_fp(5.9, 5.6, 1.1));
    d.put(slot, plant(v, 5.9, 5.6, 1.1));
    let slot = d.obj(&plant_fp(6.2, 13.2, 1.0));
    d.put(slot, plant(v, 6.2, 13.2, 1.0));
}

/// The glass room with the round table.
fn review(d: &mut Drawing) {
    let v = d.v;
    for (x, y, w, dd) in [(15.9, 6.0, 0.12, 8.0), (16.0, 5.9, 6.0, 0.12)] {
        let slot = d.obj(&fp(x, y, 0.0, w, dd, 2.4));
        d.put(slot, bx(v, x, y, 0.0, w, dd, 2.4, "m-glass", ""));
    }
    let workers: Vec<Worker> = d.floor.review.iter().take(3).cloned().collect();
    // Seats: the front, the back and the right of the table, each facing it.
    let seats: [(f64, f64, (f64, f64), bool); 3] = [
        (19.1, 11.55, (0.0, -1.0), false),
        (19.1, 8.7, (0.0, 1.0), false),
        (20.5, 10.1, (-1.0, 0.0), true),
    ];
    for (i, (x, y, face, profile)) in seats.iter().enumerate() {
        let mut g = Group::new(v);
        chair(&mut g, *x, *y);
        if let Some(w) = workers.get(i) {
            let state = match &w.doing {
                Doing::Working => "reading",
                other => doing_state(other),
            };
            let fig = ticket_fig(w, *x, *y, 0.42, v.pose(true, *face, *profile), state);
            let s = d.pj_group(&w.project, &person(v, &fig));
            g.put(spot(*x, *y, 0.42), s);
            d.pin_off((*x, *y, 2.4), (0.0, -1.55), PinWhat::Ticket(w.clone()));
        }
        let slot = d.obj(&fp(x - 0.28, y - 0.28, 0.0, 0.56, 0.56, 1.6));
        d.put(slot, g.finish());
    }
    let (tx, ty) = v.p(19.1, 10.1, 0.75);
    let table_fp = fp(17.65, 8.65, 0.0, 2.9, 2.9, 0.8);
    let table = shadow(v, 19.1, 10.1, 1.5, 0.75)
        + &bx(v, 18.95, 9.95, 0.0, 0.3, 0.3, 0.75, "m-deskleg", "")
        + &format!(
            r#"<ellipse class="tabletop" cx="{tx:.1}" cy="{ty:.1}" rx="{:.1}" ry="{:.1}"/>"#,
            1.45 * HX * 1.41,
            1.45 * HX * 0.71
        );
    let slot = d.obj(&table_fp);
    d.el(
        Element::Review,
        slot,
        &table,
        "Review room: reviewers and critics",
        (19.1, 10.1, 1.25),
    );
    // The chair on the table's west side is drawn over its rim.
    let mut g = Group::new(v);
    chair(&mut g, 17.7, 10.1);
    let chair_fp = fp(17.42, 9.82, 0.0, 0.56, 0.56, 0.4);
    let at = d.after(&chair_fp, &table_fp);
    d.put(at, g.finish());
    let sheet = poly(
        "",
        &[
            v.p(18.6, 10.3, 0.78),
            v.p(19.2, 10.3, 0.78),
            v.p(19.2, 10.75, 0.78),
            v.p(18.6, 10.75, 0.78),
        ],
    )
    .replacen(r#" class="""#, "", 1);
    let second = poly(
        "p2",
        &[
            v.p(19.3, 9.6, 0.78),
            v.p(19.8, 9.6, 0.78),
            v.p(19.8, 10.0, 0.78),
            v.p(19.3, 10.0, 0.78),
        ],
    );
    let at = d.after(&fp(18.6, 9.6, 0.78, 1.2, 1.15, 0.02), &table_fp);
    d.put(at, format!(r#"<g class="papers">{sheet}{second}</g>"#));
    let slot = d.obj(&plant_fp(21.5, 13.4, 1.0));
    d.put(slot, plant(v, 21.5, 13.4, 1.0));
}

/// The counter, the people behind it, the tray of what's next and the safe.
fn front_desk(d: &mut Drawing) {
    let v = d.v;
    let counter = fp(7.6, 10.0, 0.0, 4.0, 0.7, 1.06);
    for (i, who) in d.floor.front.iter().take(4).enumerate() {
        let x = [8.6, 10.4, 9.5, 11.3].get(i).copied().unwrap_or(8.6);
        let fig = Fig::new(
            x,
            9.3,
            0.0,
            who.role,
            &who.hire,
            v.pose(false, (0.0, 1.0), false),
        )
        .control(key("hires", &who.hire), who.hire.clone());
        let at = spot(x, 9.3, 0.0);
        let slot = (Layer::Object, v.depth_on(&at, &counter));
        d.put(slot, person(v, &fig));
    }
    let slot = d.obj(&counter);
    d.el(
        Element::Counter,
        slot,
        &(bx(v, 7.6, 10.0, 0.0, 4.0, 0.7, 1.0, "m-counter", "")
            + &bx(v, 7.6, 10.0, 1.0, 4.0, 0.7, 0.06, "m-countertop", "")),
        "Front desk: approver and router",
        (10.6, 10.3, 1.4),
    );
    let sheets = match d.floor.queued {
        0 | 1 => 1,
        2 | 3 => 2,
        _ => 3,
    };
    let tray: String = [8.0, 8.35, 8.7]
        .iter()
        .take(sheets)
        .enumerate()
        .map(|(i, x0)| {
            bx(
                v,
                *x0,
                10.15,
                1.06,
                0.28,
                0.36,
                0.02 + i as f64 * 0.01,
                "m-paper",
                "",
            )
        })
        .collect();
    let tray_fp = fp(8.0, 10.15, 1.06, 1.0, 0.36, 0.05);
    let slot = d.after(&tray_fp, &counter);
    d.el(
        Element::Queue,
        slot,
        &tray,
        "Up next: the queue",
        (8.5, 10.7, 1.2),
    );
    if d.floor.queued > 0 {
        d.pin(9.4, 10.3, 1.55, PinWhat::Queue(d.floor.queued));
    }
    let foot = (7.3, 12.2, 0.85, 0.85);
    let (dx, dy) = vface_pt(v, foot, 0.42, 0.55);
    let safe = bx(v, 7.3, 12.2, 0.0, 0.85, 0.85, 1.0, "m-safe", "")
        + &format!(
            r#"<circle class="dial" cx="{dx:.1}" cy="{dy:.1}" r="5.5"/><circle class="dial-c" cx="{dx:.1}" cy="{dy:.1}" r="1.6"/>"#
        );
    let slot = d.obj(&fp(7.3, 12.2, 0.0, 0.85, 0.85, 1.0));
    d.el(
        Element::Safe,
        slot,
        &safe,
        "Safe: the budget",
        (7.7, 12.6, 1.5),
    );
    let slot = d.obj(&plant_fp(11.5, 13.3, 0.9));
    d.put(slot, plant(v, 11.5, 13.3, 0.9));
}

/// What shipped today, the returns cart and the pallet of what's ready.
fn shipping(d: &mut Drawing) {
    let v = d.v;
    let floor = d.floor;
    // Shipped: two columns, three rows, two layers, on a low pallet.
    let mut g = Group::new(v);
    g.put(
        fp(12.4, 10.65, 0.0, 1.52, 2.22, 0.1),
        bx(v, 12.4, 10.65, 0.0, 1.52, 2.22, 0.1, "m-pallet", ""),
    );
    for (i, c) in floor.shipped.iter().take(12).enumerate() {
        let layer = i / 6;
        let row = (i % 6) / 2;
        let col = i % 2;
        let at = (
            12.5 + 0.7 * col as f64,
            10.75 + 0.7 * row as f64,
            0.1 + 0.5 * layer as f64,
        );
        g.put(
            fp(at.0, at.1, at.2, 0.62, 0.62, 0.5),
            crate_box(v, floor, c, at, "m-crate", "shipped"),
        );
    }
    let slot = d.obj(&fp(12.4, 10.65, 0.0, 1.52, 2.22, 1.1));
    d.el(
        Element::Crates,
        slot,
        &g.finish(),
        "Shipped today",
        (13.2, 11.8, 1.45),
    );

    // Returns ride the cart: up to four, two in a stack.
    let n = floor.returns.len().min(4);
    let depth = if n > 2 { 1.5 } else { 0.95 };
    let cart_fp = fp(14.55, 11.25, 0.0, 1.2, depth + 0.1, 0.67);
    let cart = bx(v, 14.6, 11.3, 0.0, 1.15, depth, 0.12, "m-cart", "")
        + &bx(v, 14.55, 11.25, 0.12, 0.06, depth + 0.1, 0.55, "m-cart", "");
    let slot = d.obj(&cart_fp);
    d.el(
        Element::Returns,
        slot,
        &cart,
        "Returns: failed work",
        (15.2, 11.8, 1.45),
    );
    let mut rides = Group::new(v);
    for (i, c) in floor.returns.iter().take(4).enumerate() {
        let (row, layer) = match i {
            0 => (0, 0),
            1 => (0, 1),
            2 => (1, 0),
            _ => (1, 1),
        };
        let at = (14.78, 11.38 + 0.62 * row as f64, 0.12 + 0.5 * layer as f64);
        rides.put(
            fp(at.0, at.1, at.2, 0.62, 0.62, 0.5),
            crate_box(v, floor, c, at, "m-crate fail", "failed"),
        );
    }
    if n > 0 {
        let slot = d.after(&fp(14.78, 11.38, 0.12, 0.62, 0.62 + 0.62, 1.0), &cart_fp);
        d.put(slot, rides.finish());
    }

    // Ready to land: up to three on the pallet.
    let ready: Vec<&Crate> = floor.ready.iter().take(3).collect();
    let w = 1.0 + 0.72 * ready.len().saturating_sub(1) as f64;
    let mut g = Group::new(v);
    g.put(
        fp(12.9, 9.0, 0.0, w, 1.0, 0.1),
        bx(v, 12.9, 9.0, 0.0, w, 1.0, 0.1, "m-pallet", ""),
    );
    for (i, c) in ready.iter().enumerate() {
        let x = 13.07 + 0.72 * i as f64;
        g.put(
            fp(x, 9.17, 0.1, 0.62, 0.62, 0.5),
            crate_box(
                v,
                floor,
                c,
                (x, 9.17, 0.1),
                "m-crate ready",
                "ready to land",
            ),
        );
        if i == 0 {
            d.pin(x + 0.31, 9.48, 1.3, PinWhat::Ready((*c).clone()));
        }
    }
    let slot = d.obj(&fp(12.9, 9.0, 0.0, w, 1.0, 0.7));
    d.put(slot, g.finish());
}

/// The router, walking a new ticket to a free desk.
fn walker(v: View) -> String {
    let path = [
        v.p(9.6, 8.6, 0.0),
        v.p(9.6, 7.0, 0.0),
        v.p(12.6, 7.0, 0.0),
        v.p(14.0, 6.3, 0.0),
    ];
    let mut d = String::new();
    for (i, (a, b)) in path.iter().enumerate() {
        let _ = write!(d, "{}{a:.1} {b:.1}", if i == 0 { "M" } else { " L" });
    }
    let fig = Fig::new(0.0, 0.0, 0.0, Role::Router, "walker", "stand-front")
        .state("walking")
        .place(Place::None);
    format!(
        r#"<g class="walker">{}<animateMotion dur="12s" repeatCount="indefinite" path="{d}" keyPoints="0;1;1;0;0" keyTimes="0;0.38;0.5;0.88;1" calcMode="linear"/></g>"#,
        person(v, &fig)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::office::model::{Desk, Person};

    fn worker(id: &str, project: &str, doing: Doing) -> Worker {
        Worker {
            ticket: id.to_string(),
            project: project.to_string(),
            title: format!("Title of {id}"),
            hire: format!("hire-{id}"),
            role: Role::Engineer,
            doing,
            progress: 0.5,
        }
    }

    fn crate_of(id: &str) -> Crate {
        Crate {
            ticket: id.to_string(),
            project: "shop".to_string(),
            title: id.to_string(),
        }
    }

    fn floor_with(n: usize, room: bool) -> Floor {
        Floor {
            desks: (0..n)
                .map(|i| Desk {
                    worker: Some(worker(
                        &format!("SHOP-{i}"),
                        "shop",
                        match i % 3 {
                            0 => Doing::Working,
                            1 => Doing::Asking("Run a command".to_string()),
                            _ => Doing::Held("budget".to_string()),
                        },
                    )),
                })
                .collect(),
            room_for_desk: room,
            projects: vec!["shop".to_string(), "api".to_string()],
            ..Floor::default()
        }
    }

    fn count(s: &str, what: &str) -> usize {
        s.matches(what).count()
    }

    const TURNS: [i64; 4] = [0, 1, 2, 3];

    fn at(t: i64) -> View {
        View::new(Turn::new(t))
    }

    #[test]
    fn projection_maps_known_points() {
        let v = at(0);
        assert_eq!(v.p(0.0, 0.0, 0.0), (420.0, 121.0));
        assert_eq!(v.p(1.0, 0.0, 0.0), (448.0, 135.0));
        assert_eq!(v.p(0.0, 1.0, 0.0), (392.0, 135.0));
        assert_eq!(v.p(0.0, 0.0, 1.0), (420.0, 91.0));
        assert_eq!(v.p(22.0, 14.0, 0.0), (644.0, 625.0));
        assert_eq!((VW, VH), (1064.0, 683.0));
        let (l, t) = v.pct(0.0, 0.0, 0.0);
        assert!((l - 420.0 / 1064.0 * 100.0).abs() < 0.01);
        assert!((t - 121.0 / 683.0 * 100.0).abs() < 0.01);
    }

    #[test]
    fn every_turn_maps_the_room_inside_the_same_box() {
        for t in TURNS {
            let v = at(t);
            for (x, y) in [(0.0, 0.0), (W, 0.0), (0.0, D), (W, D), (11.0, 7.0)] {
                let (u, w) = v.uv(x, y);
                let (bx_, by_) = v.xy(u, w);
                assert!((bx_ - x).abs() < 1e-9 && (by_ - y).abs() < 1e-9, "turn {t}");
                let (sx, sy) = v.p(x, y, 0.0);
                assert!(
                    (PAD - 1e-9..=VW - PAD + 1e-9).contains(&sx),
                    "turn {t}: {sx}"
                );
                assert!(
                    (OY - 1e-9..=OY + (W + D) * HY + 1e-9).contains(&sy),
                    "turn {t}"
                );
            }
            let scene = render(&floor_with(2, true), Turn::new(t));
            assert_eq!((scene.width, scene.height), (1064.0, 683.0));
        }
    }

    #[test]
    fn the_back_walls_are_the_ones_the_spec_names() {
        let walls: Vec<(Wall, Wall)> = TURNS.iter().map(|t| at(*t).walls()).collect();
        assert_eq!(
            walls,
            vec![
                (Wall::West, Wall::North),
                (Wall::South, Wall::West),
                (Wall::East, Wall::South),
                (Wall::North, Wall::East),
            ]
        );
        // They are the planes u = 0 and v = 0.
        for t in TURNS {
            let v = at(t);
            let (a, b) = v.walls();
            let (x, y, _) = a.at(3.0, 0.0, 0.0);
            assert!(v.uv(x, y).0.abs() < 1e-9, "turn {t}");
            let (x, y, _) = b.at(3.0, 0.0, 0.0);
            assert!(v.uv(x, y).1.abs() < 1e-9, "turn {t}");
        }
    }

    #[test]
    fn desks_follow_the_data() {
        for (n, drawn) in [(0, 0), (3, 3), (6, 6), (9, 9), (12, 9)] {
            for room in [false, true] {
                for t in TURNS {
                    let svg = render(&floor_with(n, room), Turn::new(t)).svg;
                    assert_eq!(count(&svg, "data-desk="), drawn, "{n} desks");
                    let plus = count(&svg, r#"data-el="add-desk""#);
                    assert_eq!(
                        plus,
                        usize::from(room && drawn < 9),
                        "{n} desks, room {room}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_desk_screen_shows_what_its_worker_is_doing() {
        let svg = render(&floor_with(3, false), Turn::default()).svg;
        assert_eq!(count(&svg, r#"class="screen on""#), 1);
        assert_eq!(count(&svg, r#"class="screen ask""#), 1);
        // Held, plus the console in the lab with nobody at it.
        assert_eq!(count(&svg, r#"class="screen off""#), 2);
        assert!(svg.contains(r#"id="w-SHOP-0""#));
        assert!(svg.contains(r#"data-sel="tickets/SHOP-0""#));
        assert!(svg.contains("style=\"transform: translate("));
    }

    #[test]
    fn titles_are_escaped() {
        let mut floor = floor_with(1, false);
        if let Some(w) = floor.desks.first_mut().and_then(|d| d.worker.as_mut()) {
            w.title = "<script>alert(1)</script> & \"more\"".to_string();
        }
        let svg = render(&floor, Turn::default()).svg;
        assert!(!svg.contains("<script"));
        assert!(svg.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;more&quot;"));
    }

    fn busy() -> Floor {
        let mut floor = floor_with(7, true);
        floor.checks = (0..3)
            .map(|i| worker(&format!("API-{i}"), "api", Doing::Working))
            .collect();
        floor.review = (0..4)
            .map(|i| worker(&format!("EDIT-{i}"), "shop", Doing::Working))
            .collect();
        floor.ready = (0..4).map(|i| crate_of(&format!("R-{i}"))).collect();
        floor.shipped = (0..14).map(|i| crate_of(&format!("S-{i}"))).collect();
        floor.returns = (0..6).map(|i| crate_of(&format!("F-{i}"))).collect();
        floor.goals = (0..8)
            .map(|i| Note {
                key: format!("goals/g{i}"),
                project: "api".to_string(),
                title: format!("Goal {i}"),
                landed: i % 3,
                total: if i == 7 { 0 } else { 3 },
                done: i == 2,
            })
            .collect();
        let person = |h: &str, role| Person {
            hire: h.to_string(),
            role,
            idle_minutes: Some(5),
        };
        floor.planners = vec![
            person("pro", Role::Producer),
            person("pro2", Role::Producer),
        ];
        floor.lounge = (0..5)
            .map(|i| person(&format!("idle{i}"), Role::Designer))
            .collect();
        floor.front = vec![person("app", Role::Approver), person("rou", Role::Router)];
        floor.queued = 5;
        floor.trophies = 2;
        floor.rules = 3;
        floor.hires = 4;
        floor
    }

    fn ticket_pins(scene: &Scene) -> Vec<(String, f64, f64)> {
        let mut got: Vec<(String, f64, f64)> = scene
            .pins
            .iter()
            .filter_map(|p| match &p.what {
                PinWhat::Ticket(w) => Some((w.ticket.clone(), p.left, p.top)),
                _ => None,
            })
            .collect();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        got
    }

    #[test]
    fn every_worker_has_one_ticket_pin_and_every_element_one_hint_in_every_turn() {
        let floor = busy();
        for t in TURNS {
            let scene = render(&floor, Turn::new(t));
            let mut want: Vec<String> = floor
                .desks
                .iter()
                .take(9)
                .filter_map(|d| d.worker.as_ref())
                .chain(floor.checks.iter())
                .chain(floor.review.iter().take(3))
                .map(|w| w.ticket.clone())
                .collect();
            want.sort();
            let got: Vec<String> = ticket_pins(&scene).into_iter().map(|p| p.0).collect();
            assert_eq!(want, got, "turn {t}");
            for ticket in &got {
                assert_eq!(
                    count(&scene.svg, &format!(r#"id="w-{ticket}""#)),
                    1,
                    "figure for {ticket}, turn {t}"
                );
                assert_eq!(
                    count(&scene.svg, &format!(r#"data-sel="tickets/{ticket}""#)),
                    1,
                    "control for {ticket}, turn {t}"
                );
            }

            let hints: Vec<Element> = scene
                .pins
                .iter()
                .filter_map(|p| match p.what {
                    PinWhat::Hint(e) => Some(e),
                    _ => None,
                })
                .collect();
            assert_eq!(hints.len(), count(&scene.svg, r#"data-el=""#), "turn {t}");
            for e in Element::ALL {
                let drawn = count(&scene.svg, &format!(r#"data-el="{}""#, e.key()));
                let pinned = hints.iter().filter(|h| **h == e).count();
                assert_eq!(drawn, 1, "{e:?}, turn {t}");
                assert_eq!(drawn, pinned, "{e:?}, turn {t}");
            }
            // The extra pins.
            let has = |f: fn(&PinWhat) -> bool| scene.pins.iter().filter(|p| f(&p.what)).count();
            assert_eq!(has(|w| matches!(w, PinWhat::Ready(_))), 1);
            assert_eq!(has(|w| matches!(w, PinWhat::Queue(5))), 1);
            assert_eq!(has(|w| matches!(w, PinWhat::Planning(1))), 1);
            // Every pin is inside the picture.
            for p in &scene.pins {
                assert!((0.0..=100.0).contains(&p.left), "turn {t}: {p:?}");
                assert!((0.0..=100.0).contains(&p.top), "turn {t}: {p:?}");
            }
        }
    }

    #[test]
    fn crowds_are_capped_in_every_turn() {
        for t in TURNS {
            let svg = render(&busy(), Turn::new(t)).svg;
            assert_eq!(count(&svg, r#"class="note-hit""#), 6, "turn {t}");
            assert_eq!(count(&svg, r#"class="m m-crate ready""#), 3, "turn {t}");
            assert_eq!(count(&svg, "shipped\""), 12, "turn {t}");
            assert_eq!(count(&svg, "failed\""), 4, "turn {t}");
            assert_eq!(count(&svg, "resting"), 4, "turn {t}");
            assert!(svg.contains("animateMotion"));
            assert!(svg.contains(r#"class="note pj-1 done""#));
        }
    }

    #[test]
    fn the_router_walks_only_when_tickets_wait() {
        let mut floor = floor_with(2, false);
        assert!(
            !render(&floor, Turn::default())
                .svg
                .contains("animateMotion")
        );
        floor.queued = 1;
        assert!(
            render(&floor, Turn::default())
                .svg
                .contains("animateMotion")
        );
    }

    #[test]
    fn rendering_twice_gives_the_same_string() {
        let floor = busy();
        for t in TURNS {
            assert_eq!(
                render(&floor, Turn::new(t)),
                render(&floor, Turn::new(t)),
                "turn {t}"
            );
        }
        assert_eq!(
            render(&Floor::default(), Turn::new(1)),
            render(&Floor::default(), Turn::new(1))
        );
    }

    #[test]
    fn the_svg_has_no_script() {
        for t in TURNS {
            assert!(!render(&busy(), Turn::new(t)).svg.contains("<script"));
            let scene = render(&Floor::default(), Turn::new(t));
            assert!(!scene.svg.contains("<script"));
            assert_eq!((scene.width, scene.height), (1064.0, 683.0));
            assert!(
                scene
                    .svg
                    .starts_with(r#"<svg class="iso" viewBox="0 0 1064 683""#)
            );
        }
    }

    #[test]
    fn a_half_turn_mirrors_where_a_worker_is() {
        let floor = floor_with(7, false);
        let a = ticket_pins(&render(&floor, Turn::new(0)));
        let b = ticket_pins(&render(&floor, Turn::new(2)));
        assert_eq!(a.len(), 7);
        for (p, q) in a.iter().zip(&b) {
            assert_eq!(p.0, q.0);
            assert!((p.1 + q.1 - 100.0).abs() < 0.05, "{p:?} {q:?}");
        }
        // A quarter turn moves them elsewhere.
        let c = ticket_pins(&render(&floor, Turn::new(1)));
        assert!(a.iter().zip(&c).any(|(p, q)| (p.1 - q.1).abs() > 1.0));
    }

    #[test]
    fn a_desk_worker_faces_the_screen_from_every_corner() {
        let floor = floor_with(1, false);
        let seen = |t: i64, pose: &str| {
            let svg = render(&floor, Turn::new(t)).svg;
            svg.contains(r#"agent r-eng "#) && svg.contains(&format!(" {pose} working"))
        };
        assert!(seen(0, "sit-back"));
        assert!(seen(1, "sit-front"));
        assert!(seen(2, "sit-front"));
        assert!(seen(3, "sit-back"));
    }

    /// The numbers in a polygon's points.
    fn points(poly: &str) -> Vec<(f64, f64)> {
        let Some(rest) = poly.split("points=\"").nth(1) else {
            return Vec::new();
        };
        let list = rest.split('"').next().unwrap_or("");
        list.split(' ')
            .filter_map(|p| {
                let (a, b) = p.split_once(',')?;
                Some((a.parse().ok()?, b.parse().ok()?))
            })
            .collect()
    }

    #[test]
    fn boxes_turn_as_boxes() {
        for t in TURNS {
            let v = at(t);
            let one = bx(v, 3.0, 2.0, 0.5, 1.6, 0.8, 0.7, "m-desk", "");
            let faces: Vec<&str> = one.split("<polygon").skip(1).collect();
            assert_eq!(faces.len(), 3, "turn {t}");
            let l = points(faces[0]);
            let r = points(faces[1]);
            let top = points(faces[2]);
            assert_eq!(top.len(), 4);
            assert_eq!(l.len(), 4);
            assert_eq!(r.len(), 4);
            // The faces meet the top along its two near edges.
            assert_eq!(r[3], top[1]);
            assert_eq!(r[2], top[2]);
            assert_eq!(l[3], top[3]);
            assert_eq!(l[2], top[2]);
            // The top is the footprint turned: 1.6 by 0.8 or 0.8 by 1.6.
            let (du, dv) = if t % 2 == 0 { (1.6, 0.8) } else { (0.8, 1.6) };
            assert!((top[1].0 - top[0].0 - du * HX).abs() < 0.11, "turn {t}");
            assert!((top[3].0 - top[0].0 + dv * HX).abs() < 0.11, "turn {t}");
            // It does not depend on what else was drawn.
            let other = bx(v, 8.0, 5.0, 0.0, 1.0, 1.0, 1.0, "m-rack", "");
            let again = bx(v, 3.0, 2.0, 0.5, 1.6, 0.8, 0.7, "m-desk", "");
            assert_ne!(one, other);
            assert_eq!(one, again);
        }
    }

    #[test]
    fn a_far_desk_is_drawn_before_a_near_one_in_every_turn() {
        let svg_of = |t: i64| render(&floor_with(9, false), Turn::new(t)).svg;
        for t in TURNS {
            let v = at(t);
            let svg = svg_of(t);
            let foot = |i: usize| {
                let x = DESK_X.get(i % 3).copied().unwrap_or(8.0);
                let y = DESK_Y.get(i / 3).copied().unwrap_or(6.6);
                fp(x, y, 0.0, 1.6, 1.53, 1.6)
            };
            let pos = |i: usize| {
                svg.find(&format!(r#"data-desk="{i}""#))
                    .unwrap_or(usize::MAX)
            };
            for i in 0..9 {
                for j in 0..9 {
                    if v.depth(&foot(i)) + 1.0 < v.depth(&foot(j)) {
                        assert!(pos(i) < pos(j), "desk {i} before desk {j}, turn {t}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_person_at_a_desk_is_drawn_after_the_desk_when_the_chair_is_nearer() {
        // One desk: the chair is nearer the camera from the north and the
        // west corners (turns 0 and 3), farther from the others.
        let floor = floor_with(1, false);
        for t in TURNS {
            let svg = render(&floor, Turn::new(t)).svg;
            let group = svg.find(r#"data-desk="0""#).unwrap_or(0);
            let desk_top = group + svg[group..].find(r#"m-desk""#).unwrap_or(0);
            let worker = svg.find(r#"id="w-SHOP-0""#).unwrap_or(0);
            let nearer = at(t).toward(0.0, 1.0);
            assert_eq!(worker > desk_top, nearer, "turn {t}");
        }
    }

    #[test]
    fn what_hangs_on_a_wall_stands_on_legs_when_the_wall_is_at_the_front() {
        let legs = |t: i64| count(&render(&busy(), Turn::new(t)).svg, r#"class="m m-metal""#);
        let base = legs(0);
        // Turn 1: the posters (north wall) stand. Turn 2: those and the
        // whiteboard, the team board and the trophy shelf (west wall). Turn
        // 3: the west wall things only. Two legs each.
        assert_eq!(legs(1), base + 2);
        assert_eq!(legs(2), base + 8);
        assert_eq!(legs(3), base + 6);
        let at_back = |t: i64, w: Wall| at(t).at_back(w);
        assert!(at_back(0, Wall::West) && at_back(1, Wall::West));
        assert!(!at_back(2, Wall::West) && !at_back(3, Wall::West));
        assert!(at_back(0, Wall::North) && at_back(3, Wall::North));
        assert!(!at_back(1, Wall::North) && !at_back(2, Wall::North));
        // Standing or not, the board and its notes stay controls.
        for t in TURNS {
            let svg = render(&busy(), Turn::new(t)).svg;
            for e in ["board", "corkboard", "posters", "trophies"] {
                assert_eq!(
                    count(&svg, &format!(r#"data-el="{e}""#)),
                    1,
                    "{e}, turn {t}"
                );
            }
            assert_eq!(count(&svg, r#"class="note-hit""#), 6, "turn {t}");
        }
    }

    #[test]
    fn the_windows_are_on_the_walls_at_the_back() {
        for t in TURNS {
            let v = at(t);
            let (a, b) = v.walls();
            let svg = render(&Floor::default(), Turn::new(t)).svg;
            let want =
                windows(v, a).matches("window").count() + windows(v, b).matches("window").count();
            assert_eq!(count(&svg, r#"class="window""#), want, "turn {t}");
        }
    }

    #[test]
    fn racks_follow_the_projects_with_the_free_slot_after_them() {
        for n in 0..8usize {
            let floor = Floor {
                projects: (0..n).map(|i| format!("p{i}")).collect(),
                ..Floor::default()
            };
            for t in TURNS {
                let scene = render(&floor, Turn::new(t));
                let racks = count(&scene.svg, r#"m-rack""#);
                assert_eq!(racks, n.min(MAX_RACKS), "{n} projects, turn {t}");
                assert_eq!(count(&scene.svg, r#"data-el="add-project""#), 1);
                assert_eq!(count(&scene.svg, r#"data-el="racks""#), 1);
                assert!(scene.svg.contains("elements/add-project"));
                // Each rack has a stripe in its project's colour.
                if t == 2 && n > 0 {
                    assert_eq!(count(&scene.svg, "stripe pj-"), racks, "{n} projects");
                }
                if t == 0 {
                    assert_eq!(count(&scene.svg, "stripe pj-"), 2 * racks);
                    for i in 0..racks {
                        assert!(scene.svg.contains(&format!("stripe pj-{i}")), "{n}, {i}");
                    }
                }
            }
            // The slot comes after the last rack, along the lab.
            let scene = render(&floor, Turn::default());
            let left = |e: Element| {
                scene.pins.iter().find_map(|p| match p.what {
                    PinWhat::Hint(h) if h == e => Some(p.left),
                    _ => None,
                })
            };
            let slot = left(Element::AddProject).unwrap_or(0.0);
            if n > 0 {
                assert!(slot > left(Element::Racks).unwrap_or(100.0), "{n}");
            }
        }
    }

    #[test]
    fn posters_cards_and_cups_follow_their_counts() {
        for n in 0..10usize {
            let floor = Floor {
                rules: n,
                hires: n,
                trophies: n,
                ..Floor::default()
            };
            for t in TURNS {
                let svg = render(&floor, Turn::new(t)).svg;
                assert_eq!(
                    count(&svg, r#"class="poster p"#),
                    n.min(MAX_POSTERS),
                    "{n}, turn {t}"
                );
                assert_eq!(count(&svg, "poster blank"), usize::from(n == 0));
                assert_eq!(count(&svg, "poster-band"), n.min(MAX_POSTERS));
                assert_eq!(count(&svg, r#"class="card c"#), n.min(MAX_CARDS), "{n}");
                assert_eq!(count(&svg, r#"class="trophy""#), n.min(MAX_CUPS), "{n}");
            }
        }
    }

    #[test]
    fn an_empty_office_is_a_bare_room_with_its_spots() {
        let floor = Floor {
            room_for_desk: true,
            ..Floor::default()
        };
        for t in TURNS {
            let scene = render(&floor, Turn::new(t));
            for e in Element::ALL {
                assert_eq!(
                    count(&scene.svg, &format!(r#"data-el="{}""#, e.key())),
                    1,
                    "{e:?}, turn {t}"
                );
            }
            assert_eq!(count(&scene.svg, r#"class="plus""#), 3, "turn {t}");
            assert_eq!(count(&scene.svg, "data-desk="), 0);
            assert_eq!(count(&scene.svg, r#"class="agent"#), 0);
            assert_eq!(count(&scene.svg, "m-rack"), 0);
            assert_eq!(count(&scene.svg, "m-crate"), 0);
            assert_eq!(count(&scene.svg, "note-hit"), 0);
            assert!(scene.svg.contains("elements/add-desk"));
            assert!(scene.svg.contains("elements/add-project"));
            assert!(scene.svg.contains("elements/hire"));
        }
    }
}
