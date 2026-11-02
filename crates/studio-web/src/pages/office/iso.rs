//! Isometric drawing: a room's grid turned a quarter turn at a time onto the
//! screen, and the boxes, flat shapes, people, furniture and wall items put
//! on it, drawn back to front. `scene` draws the office with it and
//! `hq_scene` the company floor.
//!
//! The floor is a grid in world coordinates: x runs along the long side, y
//! along the short side, z is up. A `View` turns the room a quarter turn at
//! a time and maps the grid onto the screen: u and v are the grid as seen
//! from the chosen corner, u running right and down the screen and v left
//! and down. Everything is drawn back to front: ground, walls, then objects
//! by depth, then overlays. Colours come only from CSS classes (and the
//! gradients in `<defs>`), so both themes work.

use super::model::{Doing, Element, Pin, PinWhat, Role, Turn, Worker};
use super::{key, safe};

/// Tile width on screen.
pub(super) const TW: f64 = 56.0;
pub(super) const HX: f64 = TW / 2.0;
pub(super) const HY: f64 = TW / 4.0;
/// One unit of height.
pub(super) const ZH: f64 = 30.0;
pub(super) const PAD: f64 = 28.0;
pub(super) const OY: f64 = 2.9 * ZH + PAD + 6.0;

/// How far into the room a board stands when its wall is at the front.
pub(super) const STAND: f64 = 0.5;
/// What tells two depths apart.
pub(super) const EPS: f64 = 1e-6;
/// The step that puts one thing just before or after another in depth.
pub(super) const NUDGE: f64 = 1e-3;

pub(super) type Pt = (f64, f64);

// ---------- the view ----------

/// The room seen from one of its four corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct View {
    pub(super) t: u8,
    /// The room's size in tiles along x and along y.
    pub(super) w: f64,
    pub(super) d: f64,
}

impl View {
    /// A room `w` tiles along x and `d` along y, seen from `turn`.
    pub(super) fn in_room(turn: Turn, w: f64, d: f64) -> View {
        View {
            t: turn.index(),
            w,
            d,
        }
    }

    /// A world point as view coordinates.
    pub(super) fn uv(self, x: f64, y: f64) -> Pt {
        match self.t {
            1 => (self.d - y, x),
            2 => (self.w - x, self.d - y),
            3 => (y, self.w - x),
            _ => (x, y),
        }
    }

    /// View coordinates as a world point.
    pub(super) fn xy(self, u: f64, v: f64) -> Pt {
        match self.t {
            1 => (v, self.d - u),
            2 => (self.w - u, self.d - v),
            3 => (self.w - v, u),
            _ => (u, v),
        }
    }

    /// How far the room reaches along u and along v.
    pub(super) fn size(self) -> Pt {
        if self.t.is_multiple_of(2) {
            (self.w, self.d)
        } else {
            (self.d, self.w)
        }
    }

    pub(super) fn ox(self) -> f64 {
        self.size().1 * HX + PAD
    }

    /// The picture's size in SVG units: width and height.
    pub(super) fn extent(self) -> Pt {
        (
            (self.w + self.d) * HX + 2.0 * PAD,
            (self.w + self.d) * HY + OY + 0.8 * ZH + PAD + 6.0,
        )
    }

    /// A view point on the screen.
    pub(super) fn s(self, u: f64, v: f64, z: f64) -> Pt {
        (self.ox() + (u - v) * HX, OY + (u + v) * HY - z * ZH)
    }

    /// A grid point on the screen.
    pub(super) fn p(self, x: f64, y: f64, z: f64) -> Pt {
        let (u, v) = self.uv(x, y);
        self.s(u, v, z)
    }

    /// A screen point as a percentage of the view box.
    pub(super) fn pct_of(self, (sx, sy): Pt) -> Pt {
        let (vw, vh) = self.extent();
        (
            (sx / vw * 10000.0).round() / 100.0,
            (sy / vh * 10000.0).round() / 100.0,
        )
    }

    /// A grid point as a percentage of the view box.
    pub(super) fn pct(self, x: f64, y: f64, z: f64) -> Pt {
        self.pct_of(self.p(x, y, z))
    }

    /// Like `pct`, moved by (du, dv) in view space, so a label sits the
    /// same way from its subject in every view.
    pub(super) fn pct_off(self, at: (f64, f64, f64), off: Pt) -> Pt {
        let (u, v) = self.uv(at.0, at.1);
        self.pct_of(self.s(u + off.0, v + off.1, at.2))
    }

    /// A world direction as a view direction.
    pub(super) fn dir(self, dx: f64, dy: f64) -> Pt {
        let (a, b) = self.uv(dx, dy);
        let (o, q) = self.uv(0.0, 0.0);
        (a - o, b - q)
    }

    /// Whether a world direction points toward the camera.
    pub(super) fn toward(self, dx: f64, dy: f64) -> bool {
        let (du, dv) = self.dir(dx, dy);
        du + dv > 0.0
    }

    /// The two walls at the back: the one on the plane u = 0 and the one on
    /// the plane v = 0.
    pub(super) fn walls(self) -> (Wall, Wall) {
        match self.t {
            1 => (Wall::South, Wall::West),
            2 => (Wall::East, Wall::South),
            3 => (Wall::North, Wall::East),
            _ => (Wall::West, Wall::North),
        }
    }

    pub(super) fn at_back(self, wall: Wall) -> bool {
        let (a, b) = self.walls();
        wall == a || wall == b
    }

    /// The pose of a figure facing the world direction `face`: from behind
    /// when it faces away from the camera, from the front when it faces it.
    /// A figure in profile is seen from the side when it faces away along u.
    pub(super) fn pose(self, sit: bool, face: Pt, profile: bool) -> &'static str {
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
    pub(super) fn depth(self, f: &Fp) -> f64 {
        let (u, v) = self.uv((f.x0 + f.x1) / 2.0, (f.y0 + f.y1) / 2.0);
        u + v
    }

    /// Whether `a` is drawn before `b`: when they stand apart along an
    /// axis, the one farther from the camera; when they overlap, the lower.
    pub(super) fn before(self, a: &Fp, b: &Fp) -> bool {
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
    pub(super) fn depth_on(self, f: &Fp, host: &Fp) -> f64 {
        let own = self.depth(f);
        let at = self.depth(host);
        if self.before(f, host) {
            own.min(at - NUDGE)
        } else {
            own.max(at + NUDGE)
        }
    }

    /// The box around a solid on the screen: left, top, right, bottom.
    pub(super) fn bbox(self, f: &Fp) -> [f64; 4] {
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
pub(super) struct Fp {
    pub(super) x0: f64,
    pub(super) y0: f64,
    pub(super) x1: f64,
    pub(super) y1: f64,
    pub(super) z0: f64,
    pub(super) z1: f64,
}

/// The footprint of a box, with the arguments `bx` takes.
pub(super) fn fp(x: f64, y: f64, z: f64, w: f64, d: f64, h: f64) -> Fp {
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
pub(super) fn spot(x: f64, y: f64, z: f64) -> Fp {
    fp(x - 0.2, y - 0.2, z, 0.4, 0.4, 1.2)
}

/// One of the four walls round the room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Wall {
    /// The plane y = 0.
    North,
    /// The plane x = 0.
    West,
    /// The plane y = the room's depth.
    South,
    /// The plane x = the room's width.
    East,
}

impl Wall {
    /// A point `a` along the wall of a room `w` by `d` tiles, `off` into the
    /// room and `z` up.
    pub(super) fn at_in(self, (w, d): Pt, a: f64, off: f64, z: f64) -> (f64, f64, f64) {
        match self {
            Wall::North => (a, off, z),
            Wall::West => (off, a, z),
            Wall::South => (a, d - off, z),
            Wall::East => (w - off, a, z),
        }
    }

    pub(super) fn along_x(self) -> bool {
        matches!(self, Wall::North | Wall::South)
    }

    /// The footprint of a thing `thick` deep, centred `off` into the room,
    /// from `a0` to `a1` along the wall.
    pub(super) fn fp(self, room: Pt, off: f64, (a0, a1): Pt, thick: f64, z: f64, h: f64) -> Fp {
        let c = self.at_in(room, a0, off, z);
        if self.along_x() {
            fp(c.0, c.1 - thick / 2.0, z, a1 - a0, thick, h)
        } else {
            fp(c.0 - thick / 2.0, c.1, z, thick, a1 - a0, h)
        }
    }
}

pub(super) fn pts(ps: &[Pt]) -> String {
    ps.iter()
        .map(|(a, b)| format!("{a:.1},{b:.1}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` safe to put in XML text or an attribute.
pub(super) fn esc(text: &str) -> String {
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
pub(super) fn control(sel: &str, label: &str) -> String {
    format!(
        r#"data-sel="{}" tabindex="0" role="button" aria-label="{}""#,
        esc(sel),
        esc(label)
    )
}

pub(super) fn line(a: Pt, b: Pt) -> String {
    format!(
        r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}"/>"#,
        a.0, a.1, b.0, b.1
    )
}

pub(super) fn poly(class: &str, ps: &[Pt]) -> String {
    format!(r#"<polygon class="{class}" points="{}"/>"#, pts(ps))
}

/// The left, right and top faces of a box. The footprint is turned as a
/// rectangle: the right face is the one on the far-u side and the left face
/// the one on the far-v side, in the view.
pub(super) fn faces(v: View, foot: Foot, z: f64, h: f64) -> [[Pt; 4]; 3] {
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
pub(super) fn bx(
    v: View,
    x: f64,
    y: f64,
    z: f64,
    w: f64,
    d: f64,
    h: f64,
    mat: &str,
    extra: &str,
) -> String {
    let [l, r, t] = faces(v, (x, y, w, d), z, h);
    format!(
        r#"<g class="m {mat}"{extra}>{}{}{}</g>"#,
        poly("l", &l),
        poly("r", &r),
        poly("t", &t)
    )
}

/// The dashed outline of a box that is not there yet.
pub(super) fn ghost_box(v: View, foot: Foot, z: f64, h: f64) -> String {
    faces(v, foot, z, h)
        .iter()
        .map(|f| poly("ghost", f))
        .collect()
}

/// A box given in view coordinates.
pub(super) fn bx_view(v: View, (u0, u1): Pt, (v0, v1): Pt, z: f64, h: f64, mat: &str) -> String {
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
pub(super) type Foot = (f64, f64, f64, f64);

/// Where a box's face on the far-v side has `a` along it from its near-u
/// corner, at height `z`: for marks drawn on the face that shows.
pub(super) fn vface_pt(v: View, foot: Foot, a: f64, z: f64) -> Pt {
    let (x, y, w, d) = foot;
    let (ua, va) = v.uv(x, y);
    let (ub, vb) = v.uv(x + w, y + d);
    v.s(ua.min(ub) + a, va.max(vb), z)
}

/// A rectangle on the face of a box that is on the far-v side.
pub(super) fn vface(v: View, foot: Foot, (a0, a1): Pt, (z0, z1): Pt, class: &str) -> String {
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
pub(super) fn flat(v: View, x0: f64, y0: f64, x1: f64, y1: f64, class: &str) -> String {
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

pub(super) fn shadow(v: View, x: f64, y: f64, rx: f64, ry: f64) -> String {
    let (sx, sy) = v.p(x, y, 0.0);
    format!(
        r#"<ellipse class="shade" cx="{sx:.1}" cy="{sy:.1}" rx="{:.1}" ry="{:.1}"/>"#,
        rx * HX,
        ry * HX
    )
}

/// A rectangle on a wall, `off` into the room (0 for on the wall itself),
/// from `a0` to `a1` along it.
pub(super) fn wall_rect(
    v: View,
    wall: Wall,
    off: f64,
    (a0, a1): Pt,
    (z0, z1): Pt,
    class: &str,
) -> String {
    let at = |a: f64, z: f64| {
        let (x, y, z) = wall.at_in((v.w, v.d), a, off, z);
        v.p(x, y, z)
    };
    poly(class, &[at(a0, z0), at(a1, z0), at(a1, z1), at(a0, z1)])
}

/// Text laid on the floor, reading along the view's u axis.
pub(super) fn floor_text(v: View, x: f64, y: f64, text: &str) -> String {
    let (sx, sy) = v.p(x, y, 0.0);
    format!(
        r#"<text class="paint" transform="matrix(1 0.5 -1 0.5 {sx:.1} {sy:.1})" text-anchor="middle">{text}</text>"#
    )
}

/// A "+" lying at a point.
pub(super) fn plus(v: View, x: f64, y: f64, z: f64) -> String {
    let (cx, cy) = v.p(x, y, z);
    format!(
        r#"<g class="plus" transform="translate({cx:.1} {cy:.1})"><rect x="-1.5" y="-8" width="3" height="16" rx="1.5"/><rect x="-8" y="-1.5" width="16" height="3" rx="1.5"/></g>"#
    )
}

// ---------- drawing order ----------

/// The parts of one thing, drawn back to front for the view they are seen
/// in. Its shadows go first.
pub(super) struct Group {
    pub(super) v: View,
    pub(super) shade: String,
    pub(super) parts: Vec<(Fp, String)>,
}

impl Group {
    pub(super) fn new(v: View) -> Group {
        Group {
            v,
            shade: String::new(),
            parts: Vec::new(),
        }
    }

    pub(super) fn shade(&mut self, svg: &str) {
        self.shade.push_str(svg);
    }

    pub(super) fn put(&mut self, f: Fp, svg: String) {
        self.parts.push((f, svg));
    }

    /// The parts in an order that draws what is behind first. Parts that
    /// don't overlap on the screen stay in the order they were put.
    pub(super) fn finish(self) -> String {
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
pub(super) enum Place {
    /// With the transform attribute.
    Attr,
    /// With a style, so the page can animate it walking.
    Style,
}

pub(super) struct Fig {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) z: f64,
    pub(super) role: Role,
    /// What the figure's looks are picked from.
    pub(super) seed: String,
    pub(super) pose: &'static str,
    pub(super) state: &'static str,
    pub(super) place: Place,
    pub(super) id: Option<String>,
    /// The selection key and the label that make it a control.
    pub(super) sel: Option<(String, String)>,
    pub(super) long_hair: bool,
    /// How much bigger than the usual size it is drawn.
    pub(super) scale: f64,
}

impl Fig {
    pub(super) fn new(x: f64, y: f64, z: f64, role: Role, seed: &str, pose: &'static str) -> Fig {
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
            scale: 1.0,
        }
    }

    pub(super) fn scaled(mut self, scale: f64) -> Fig {
        self.scale = scale;
        self
    }

    pub(super) fn state(mut self, state: &'static str) -> Fig {
        self.state = state;
        self
    }

    pub(super) fn place(mut self, place: Place) -> Fig {
        self.place = place;
        self
    }

    pub(super) fn control(mut self, sel: String, label: String) -> Fig {
        self.sel = Some((sel, label));
        self
    }

    pub(super) fn id(mut self, id: String) -> Fig {
        self.id = Some(id);
        self
    }
}

/// A skin and a hair tone, from a name.
pub(crate) fn looks(seed: &str) -> (char, char) {
    let h = seed
        .bytes()
        .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    let pick = |n: u32| "abcd".chars().nth((n % 4) as usize).unwrap_or('a');
    (pick(h), pick(h / 4))
}

/// A small figure. Poses: sit-back (seen from behind at a desk), sit-front,
/// sit-side, stand-back, stand-front and stand-side.
pub(super) fn person(v: View, f: &Fig) -> String {
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
    let grown = if (f.scale - 1.0).abs() < 1e-9 {
        String::new()
    } else {
        format!(" scale({})", f.scale)
    };
    match f.place {
        Place::Attr => attrs.push_str(&format!(
            r#" transform="translate({sx:.1} {sy:.1}){grown}""#
        )),
        Place::Style => attrs.push_str(&format!(
            r#" style="transform: translate({sx:.1}px, {sy:.1}px){grown}""#
        )),
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
    // A tie, in a darker shade, so leads and the owner read as managers.
    if matches!(f.role, Role::Lead | Role::Owner) && !back {
        g.push_str(&format!(
            r#"<path class="tie" d="M-1.8 {} h3.6 l1.2 9 l-3 3 l-3 -3z"/>"#,
            body_y + 1.0
        ));
    }
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

pub(super) fn doing_state(d: &Doing) -> &'static str {
    match d {
        Doing::Working => "working",
        Doing::Asking(_) => "asking",
        Doing::Held(_) => "held",
    }
}

pub(super) fn doing_screen(d: &Doing) -> &'static str {
    match d {
        Doing::Working => "on",
        Doing::Asking(_) => "ask",
        Doing::Held(_) => "off",
    }
}

pub(super) fn doing_words(d: &Doing) -> String {
    match d {
        Doing::Working => "working".to_string(),
        Doing::Asking(what) if what.trim().is_empty() => "waiting for you".to_string(),
        Doing::Asking(what) => format!("waiting for you: {}", what.trim()),
        Doing::Held(why) if why.trim().is_empty() => "on hold".to_string(),
        Doing::Held(why) => format!("on hold: {}", why.trim()),
    }
}

/// A ticket's figure: a control that the page can walk to another room.
pub(super) fn ticket_fig(
    w: &Worker,
    x: f64,
    y: f64,
    z: f64,
    pose: &'static str,
    state: &'static str,
) -> Fig {
    let label = format!(
        "{}, {}: {}, {}",
        w.name,
        w.ticket,
        w.title,
        doing_words(&w.doing)
    );
    Fig::new(x, y, z, w.role, &w.hire, pose)
        .state(state)
        .place(Place::Style)
        .id(format!("w-{}", safe(&w.ticket)))
        .control(key("tickets", &w.ticket), label)
}

pub(super) fn minutes(n: u64) -> String {
    if n == 1 {
        "1 minute".to_string()
    } else {
        format!("{n} minutes")
    }
}

// ---------- furniture ----------

/// Facing the back wall at t = 0: a desk whose monitor faces the worker,
/// who sits on the +y side.
pub(super) fn desk(g: &mut Group, x: f64, y: f64, screen: &str) {
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

pub(super) fn chair(g: &mut Group, x: f64, y: f64) {
    let v = g.v;
    g.shade(&shadow(v, x, y, 0.42, 0.22));
    g.put(
        fp(x - 0.28, y - 0.28, 0.0, 0.56, 0.56, 0.4),
        bx(v, x - 0.28, y - 0.28, 0.0, 0.56, 0.56, 0.4, "m-chair", ""),
    );
}

pub(super) fn plant(v: View, x: f64, y: f64, size: f64) -> String {
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
pub(super) fn plant_fp(x: f64, y: f64, size: f64) -> Fp {
    fp(
        x - 0.22 * size,
        y - 0.22 * size,
        0.0,
        0.44 * size,
        0.44 * size,
        1.2 * size,
    )
}

// ---------- the drawing ----------

/// Which pass a drawn piece belongs to: the ground, the walls and what is
/// mounted on them, then the objects in the room.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Layer {
    Ground,
    Wall,
    Object,
}

/// Where a piece goes in the order: its layer and its depth.
pub(super) type Slot = (Layer, f64);

/// What a drawing needs to know about whatever it draws: the colour of a
/// project.
pub(super) trait Tints {
    fn tint(&self, project: &str) -> usize;
}

/// A drawing of `floor`: the office or the company floor.
pub(super) struct Drawing<'a, F: Tints> {
    pub(super) floor: &'a F,
    pub(super) v: View,
    /// Everything drawn, in the order it was put.
    pub(super) items: Vec<(Slot, String)>,
    pub(super) pins: Vec<Pin>,
    pub(super) hints: Vec<Pin>,
    /// Where people stand, as grid points: what labels keep clear of.
    pub(super) people: Vec<Pt>,
}

/// Something mounted on a wall: on it when the wall is at the back, on two
/// legs a little into the room when it is at the front.
pub(super) struct Mount {
    pub(super) wall: Wall,
    /// How far into the room: 0 on the wall.
    pub(super) off: f64,
    pub(super) slot: Slot,
    pub(super) standing: bool,
    /// The room's size, to place points on its walls.
    pub(super) room: Pt,
}

impl Mount {
    /// A rectangle on the board.
    pub(super) fn rect(&self, v: View, a: Pt, z: Pt, class: &str) -> String {
        wall_rect(v, self.wall, self.off, a, z, class)
    }

    /// The legs of a board from `a0` to `a1`, `shift` further into the room,
    /// up to height `top`: none while it is on the wall.
    pub(super) fn legs(&self, v: View, (a0, a1): Pt, shift: f64, top: f64) -> String {
        if !self.standing {
            return String::new();
        }
        let mut s = String::new();
        for a in [a0 + 0.06, a1 - 0.14] {
            let f = self
                .wall
                .fp(self.room, self.off + shift, (a, a + 0.08), 0.08, 0.0, top);
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
}

impl<'a, F: Tints> Drawing<'a, F> {
    pub(super) fn new(floor: &'a F, v: View) -> Drawing<'a, F> {
        Drawing {
            floor,
            v,
            items: Vec::new(),
            pins: Vec::new(),
            hints: Vec::new(),
            people: Vec::new(),
        }
    }

    pub(super) fn put(&mut self, slot: Slot, svg: String) {
        self.items.push((slot, svg));
    }

    /// The slot of an object in the room.
    pub(super) fn obj(&self, f: &Fp) -> Slot {
        (Layer::Object, self.v.depth(f))
    }

    /// The slot of something that stands with `host` and is drawn after it
    /// unless it is behind it.
    pub(super) fn with(&self, f: &Fp, host: &Fp) -> Slot {
        (Layer::Object, self.v.depth_on(f, host))
    }

    /// The slot of something that is always drawn after `host`.
    pub(super) fn after(&self, f: &Fp, host: &Fp) -> Slot {
        let at = self.v.depth(host) + NUDGE;
        (Layer::Object, self.v.depth(f).max(at))
    }

    /// A board on `wall` from `a0` to `a1`.
    pub(super) fn mount(&self, wall: Wall, span: Pt) -> Mount {
        let room = (self.v.w, self.v.d);
        if self.v.at_back(wall) {
            return Mount {
                wall,
                off: 0.0,
                slot: (Layer::Wall, 0.0),
                standing: false,
                room,
            };
        }
        let f = wall.fp(room, STAND, span, 0.1, 0.0, 2.5);
        Mount {
            wall,
            off: STAND,
            slot: self.obj(&f),
            standing: true,
            room,
        }
    }

    /// A pin moved by (du, dv) in view space from the point it is for.
    pub(super) fn pin_off(&mut self, at: (f64, f64, f64), off: Pt, what: PinWhat) {
        let (left, top) = self.v.pct_off(at, off);
        self.pins.push(Pin {
            left,
            top,
            what,
            spots: Vec::new(),
        });
    }

    /// Draws a game element as a control, with its hint pinned at (x, y, z).
    pub(super) fn el(
        &mut self,
        e: Element,
        slot: Slot,
        inner: &str,
        label: &str,
        at: (f64, f64, f64),
    ) {
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
            spots: Vec::new(),
        });
    }

    pub(super) fn tint(&self, project: &str) -> usize {
        self.floor.tint(project)
    }

    /// Wraps `inner` in a group for `project`.
    pub(super) fn pj_group(&self, project: &str, inner: &str) -> String {
        format!(r#"<g data-pj="{}">{inner}</g>"#, self.tint(project))
    }

    /// The drawing, ground first, then walls, objects by depth and
    /// overlays; pieces at the same depth stay as they were put.
    pub(super) fn svg(mut self) -> (String, Vec<Pin>) {
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

pub(super) const GROUND: Slot = (Layer::Ground, 0.0);
pub(super) const WALL: Slot = (Layer::Wall, 0.0);

#[cfg(test)]
mod tests {
    use super::*;

    const TURNS: [i64; 4] = [0, 1, 2, 3];

    #[test]
    fn every_room_turns_inside_its_own_box() {
        for (w, d) in [(23.0, 21.0), (23.0, 29.2), (23.0, 37.4), (6.0, 4.0)] {
            for t in TURNS {
                let v = View::in_room(Turn::new(t), w, d);
                let (vw, vh) = v.extent();
                for (x, y) in [(0.0, 0.0), (w, 0.0), (0.0, d), (w, d), (w / 2.0, d / 2.0)] {
                    let (u, q) = v.uv(x, y);
                    let (bx_, by_) = v.xy(u, q);
                    assert!((bx_ - x).abs() < 1e-9 && (by_ - y).abs() < 1e-9);
                    let (sx, sy) = v.p(x, y, 0.0);
                    assert!(
                        (PAD - 1e-9..=vw - PAD + 1e-9).contains(&sx),
                        "{w}x{d}, turn {t}"
                    );
                    assert!((OY - 1e-9..=OY + (w + d) * HY + 1e-9).contains(&sy));
                    assert!(sy <= vh);
                }
                // The back walls are the planes u = 0 and v = 0.
                let (a, b) = v.walls();
                let (x, y, _) = a.at_in((w, d), 1.0, 0.0, 0.0);
                assert!(v.uv(x, y).0.abs() < 1e-9, "{w}x{d}, turn {t}");
                let (x, y, _) = b.at_in((w, d), 1.0, 0.0, 0.0);
                assert!(v.uv(x, y).1.abs() < 1e-9, "{w}x{d}, turn {t}");
            }
        }
    }

    #[test]
    fn a_figure_can_be_drawn_bigger_and_leads_wear_a_tie() {
        let v = View::in_room(Turn::default(), 23.0, 21.0);
        let lead = || Fig::new(1.0, 1.0, 0.0, Role::Lead, "lead", "stand-front");
        let plain = person(v, &lead());
        let grown = person(v, &lead().scaled(1.12));
        assert!(!plain.contains("scale("));
        assert!(grown.contains(") scale(1.12)\""), "{grown}");
        // Leads and the owner wear a tie, seen from the front.
        assert!(plain.contains(r#"class="tie""#));
        let behind = Fig::new(1.0, 1.0, 0.0, Role::Owner, "me", "stand-back");
        assert!(!person(v, &behind).contains(r#"class="tie""#));
        let maker = Fig::new(1.0, 1.0, 0.0, Role::Engineer, "me", "stand-front");
        assert!(!person(v, &maker).contains(r#"class="tie""#));
    }
}
