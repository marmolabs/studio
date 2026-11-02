//! The company floor: one team area per project around the owner's office,
//! the lounge and the shared review and checks rooms, drawn from an `Hq`.
//!
//! The floor is two team areas wide and as deep as its rows need. The shared
//! rooms keep their places: the owner's glass office, the checks lab and the
//! review room along the back, the lounge down the left. Each team area has a
//! rug in its project's colour, low partitions, the lead's desk at its head,
//! four desks for the members and one prop that shows the project's phase.
//! The last area is empty and dashed: the place for a new project.

use super::iso::{
    self, Fig, Fp, GROUND, Group, HX, Layer, NUDGE, Pt, Tints, View, WALL, Wall, bx, bx_view,
    chair, control, desk, doing_screen, flat, floor_text, fp, ghost_box, line, minutes, person,
    plant, plant_fp, poly, shadow, spot, ticket_fig, vface, wall_rect,
};
use super::key;
use super::model::{
    Element, Hq, Launch, Phase, Pin, PinWhat, Role, Scene, TINTS, Team, Turn, Worker,
};
use super::pins::{self, Plan, Rect};

/// The drawing of the company floor.
type Drawing<'a> = iso::Drawing<'a, Hq>;

impl Tints for Hq {
    fn tint(&self, project: &str) -> usize {
        self.teams
            .iter()
            .find(|t| t.project == project)
            .map_or(0, |t| t.tint % TINTS)
    }
}

/// How wide the floor is, in tiles.
const FLOOR_W: f64 = 23.0;
/// Team areas side by side, and the most teams drawn: a ninth project would
/// share the first one's colour.
const COLS: usize = 2;
const MAX_TEAMS: usize = 8;
/// A team area's size, and where the first one starts and how far apart they
/// are.
const POD_U: f64 = 7.0;
const POD_V: f64 = 5.6;
const POD_X: f64 = 3.8;
const POD_Y: f64 = 6.0;
const PITCH_X: f64 = 9.0;
const PITCH_Y: f64 = 8.2;
/// The shared rooms: the owner's office, the checks lab, the review room and
/// the lounge, each as x0, y0, x1, y1.
const OFFICE: (f64, f64, f64, f64) = (0.0, 0.0, 5.6, 5.0);
const LAB: (f64, f64, f64, f64) = (12.4, 0.0, 17.6, 5.0);
const REVIEW: (f64, f64, f64, f64) = (18.2, 0.0, FLOOR_W, 5.0);
const LOUNGE: (f64, f64, f64, f64) = (0.0, 13.8, 3.6, 21.0);
/// Where the desks of a team area stand, from its corner, and whether the
/// member at it faces the head of the floor.
const SEATS: [(f64, f64, bool); 4] = [
    (2.5, 1.0, true),
    (4.8, 1.0, true),
    (2.5, 3.7, false),
    (4.8, 3.7, false),
];
/// Which desk the first, second, third and fourth member take.
const SEAT_ORDER: [usize; 4] = [0, 3, 2, 1];
/// The most people drawn on the lounge's sofa.
const MAX_LOUNGERS: usize = 4;
/// People are drawn a little bigger than in the office, to read on a floor
/// this wide.
const FIG_SCALE: f64 = 1.12;
/// Where team area `cell` starts.
fn origin(cell: usize) -> Pt {
    (
        POD_X + PITCH_X * (cell % COLS) as f64,
        POD_Y + PITCH_Y * (cell / COLS) as f64,
    )
}

/// How deep a floor with `cells` areas (teams and the empty one) is.
fn depth_for(cells: usize) -> f64 {
    let rows = cells.div_ceil(COLS).max(2);
    POD_Y + PITCH_Y * (rows - 1) as f64 + POD_V + 1.2
}

const DEFS: &str = concat!(
    "<defs>",
    r##"<radialGradient id="halo" cx="50%" cy="50%" r="50%"><stop offset="0" stop-color="#7aa7ff" stop-opacity=".16"/><stop offset="1" stop-color="#7aa7ff" stop-opacity="0"/></radialGradient>"##,
    r##"<radialGradient id="pool" cx="50%" cy="50%" r="50%"><stop offset="0" stop-color="var(--pool)" stop-opacity="1"/><stop offset="1" stop-color="var(--pool)" stop-opacity="0"/></radialGradient>"##,
    "</defs>"
);

/// The floor seen from `turn`, with a sign for each team area.
pub fn render(hq: &Hq, turn: Turn) -> Scene {
    scene(hq, turn).0
}

/// The floor and what its labels were laid out around.
fn scene(hq: &Hq, turn: Turn) -> (Scene, Plan) {
    let teams = hq.teams.len().min(MAX_TEAMS);
    let depth = depth_for(teams + 1);
    let v = View::in_room(turn, FLOOR_W, depth);
    let mut d = Drawing::new(hq, v);
    shell(&mut d);
    owner_office(&mut d);
    lobby(&mut d);
    checks_lab(&mut d);
    review_room(&mut d);
    lounge(&mut d);
    for (i, team) in hq.teams.iter().take(MAX_TEAMS).enumerate() {
        team_area(&mut d, team, origin(i));
    }
    new_project(&mut d, origin(teams));
    furnish(&mut d, (teams + 1).div_ceil(COLS).max(2));
    let people = std::mem::take(&mut d.people);
    let (body, pins) = d.svg();
    let (width, height) = v.extent();
    let (keep, rooms) = keep_clear(v, &people);
    let plan = Plan {
        extent: (width, height),
        keep,
        rooms,
        pods: (0..=teams).map(|i| pod_corners(v, origin(i))).collect(),
        closed: hq.closed,
    };
    let signs = hq
        .teams
        .iter()
        .take(MAX_TEAMS)
        .enumerate()
        .map(|(i, t)| {
            let (x0, y0) = origin(i);
            let (left, top) = v.pct(x0, y0, 0.0);
            Pin {
                left,
                top,
                what: PinWhat::Sign(Box::new(t.clone())),
                spots: Vec::new(),
            }
        })
        .collect();
    let laid = pins::lay_out(&plan, pins, signs);
    let mut lines = String::new();
    for l in &laid.leaders {
        let tint = hq.teams.get(l.team).map_or(0, |t| t.tint % TINTS);
        lines.push_str(&format!(
            r#"<g class="lead t{} pj-{tint}"><path d="M{:.1} {:.1}L{:.1} {:.1}"/><circle cx="{:.1}" cy="{:.1}" r="3.2"/></g>"#,
            l.tier, l.from.0, l.from.1, l.to.0, l.to.1, l.to.0, l.to.1
        ));
    }
    let svg = format!(
        r#"<svg class="iso" viewBox="0 0 {width:.0} {height:.0}" role="img" aria-label="The company floor: a team area for each project, the owner's office, the lounge, the review room and the checks lab">{DEFS}{body}<g class="leads" pointer-events="none">{lines}</g></svg>"#
    );
    let scene = Scene {
        width: width.round(),
        height: height.round(),
        svg,
        pins: laid.pins,
        forms: laid.forms,
    };
    (scene, plan)
}

/// What labels keep clear of, in the picture's own units, the owner's office
/// and the people; and the shared rooms, which they keep off if they can.
fn keep_clear(v: View, people: &[Pt]) -> (Vec<Rect>, Vec<Rect>) {
    let room = |((x0, y0, x1, y1), h): ((f64, f64, f64, f64), f64)| {
        let corners: Vec<Pt> = [0.0, h]
            .into_iter()
            .flat_map(|z| [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| v.p(x, y, z)))
            .collect();
        pins::silhouette(&corners, 6)
    };
    let mut keep = room((OFFICE, 2.4));
    let rooms = [(LAB, 2.3), (REVIEW, 2.3), (LOUNGE, 1.7)]
        .into_iter()
        .flat_map(room)
        .collect();
    for (x, y) in people {
        let (sx, sy) = v.p(*x, *y, 0.0);
        keep.push(Rect::new(sx - 15.0, sy - 62.0, sx + 15.0, sy + 8.0));
    }
    (keep, rooms)
}

/// The corners of the team area that starts at `(x0, y0)`, on the picture.
fn pod_corners(v: View, (x0, y0): Pt) -> [Pt; 4] {
    [
        v.p(x0, y0, 0.0),
        v.p(x0 + POD_U, y0, 0.0),
        v.p(x0 + POD_U, y0 + POD_V, 0.0),
        v.p(x0, y0 + POD_V, 0.0),
    ]
}

// ---------- the room ----------

/// The slab, the floors, the rugs, the walls and the windows.
fn shell(d: &mut Drawing) {
    let v = d.v;
    let (w, dd) = (v.w, v.d);
    d.put(
        GROUND,
        poly(
            "glow",
            &[
                v.p(-3.0, -3.0, 0.0),
                v.p(w + 3.0, -3.0, 0.0),
                v.p(w + 3.0, dd + 3.0, 0.0),
                v.p(-3.0, dd + 3.0, 0.0),
            ],
        ),
    );
    d.put(GROUND, bx(v, 0.0, 0.0, -0.75, w, dd, 0.75, "m-slab", ""));
    d.put(GROUND, flat(v, 0.0, 0.0, w, dd, "fl fl-open"));
    let mut tiles = String::new();
    let mut k = 2.0;
    while k < w {
        tiles.push_str(&line(v.p(k, 0.0, 0.0), v.p(k, dd, 0.0)));
        k += 2.0;
    }
    k = 2.0;
    while k < dd {
        tiles.push_str(&line(v.p(0.0, k, 0.0), v.p(w, k, 0.0)));
        k += 2.0;
    }
    d.put(GROUND, format!(r#"<g class="tiles">{tiles}</g>"#));
    for ((x0, y0, x1, y1), class) in [
        (OFFICE, "fl fl-owner"),
        (LAB, "fl fl-qa"),
        (REVIEW, "fl fl-review"),
        (LOUNGE, "fl fl-lounge"),
    ] {
        d.put(GROUND, flat(v, x0, y0, x1, y1, class));
    }
    let mut planks = String::new();
    let mut yy = LOUNGE.1 + 0.5;
    while yy < LOUNGE.3 {
        planks.push_str(&line(v.p(LOUNGE.0, yy, 0.0), v.p(LOUNGE.2, yy, 0.0)));
        yy += 0.5;
    }
    yy = OFFICE.1 + 0.5;
    while yy < OFFICE.3 - 0.4 {
        planks.push_str(&line(v.p(OFFICE.0, yy, 0.0), v.p(OFFICE.2, yy, 0.0)));
        yy += 0.5;
    }
    d.put(GROUND, format!(r#"<g class="planks">{planks}</g>"#));
    for (x, y, text) in [(15.0, 4.55, "CHECKS LAB"), (20.6, 4.55, "REVIEW")] {
        d.put(GROUND, floor_text(v, x, y, text));
    }
    for (x, y, r) in [
        (2.8, 2.8, 2.6),
        (15.0, 2.6, 2.3),
        (20.6, 2.6, 2.3),
        (1.8, 17.3, 2.8),
        (8.0, 3.0, 2.6),
    ] {
        d.put(GROUND, pool(v, x, y, r));
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
    let mut city = String::new();
    for wall in [vw, uw] {
        d.put(WALL, windows(v, wall));
        city.push_str(&towers(v, wall));
    }
    d.put(WALL, format!(r#"<g class="city">{city}</g>"#));
}

/// A pool of light on the floor, seen at night.
fn pool(v: View, x: f64, y: f64, r: f64) -> String {
    let (sx, sy) = v.p(x, y, 0.0);
    format!(
        r#"<ellipse class="pool" cx="{sx:.1}" cy="{sy:.1}" rx="{:.1}" ry="{:.1}"/>"#,
        r * HX * 1.4,
        r * HX * 0.7
    )
}

/// A circle of radius `r` tiles lying on the floor.
fn disc(v: View, x: f64, y: f64, r: f64, class: &str, extra: &str) -> String {
    let (sx, sy) = v.p(x, y, 0.0);
    format!(
        r#"<ellipse class="{class}" cx="{sx:.1}" cy="{sy:.1}" rx="{:.1}" ry="{:.1}"{extra}/>"#,
        r * HX * std::f64::consts::SQRT_2,
        r * HX * std::f64::consts::FRAC_1_SQRT_2
    )
}

/// Where the windows of a wall start along it: the same spacing on every wall.
fn window_starts(len: f64) -> Vec<f64> {
    let mut out = Vec::new();
    let mut a = 1.0;
    while a + 2.2 <= len - 0.6 {
        out.push(a);
        a += 2.9;
    }
    out
}

fn wall_len(v: View, wall: Wall) -> f64 {
    if wall.along_x() { v.w } else { v.d }
}

fn windows(v: View, wall: Wall) -> String {
    let mut s = String::new();
    for a0 in window_starts(wall_len(v, wall)) {
        s.push_str(&wall_rect(
            v,
            wall,
            0.0,
            (a0, a0 + 2.2),
            (1.15, 2.55),
            "window",
        ));
        s.push_str(&wall_rect(
            v,
            wall,
            0.0,
            (a0 + 1.07, a0 + 1.13),
            (1.15, 2.55),
            "mullion",
        ));
    }
    s
}

/// The skyline seen through a wall's windows.
fn towers(v: View, wall: Wall) -> String {
    const OFFSETS: [f64; 5] = [0.2, 0.55, 0.95, 1.4, 1.8];
    const HEIGHTS: [f64; 5] = [1.6, 1.95, 1.45, 2.1, 1.7];
    let mut s = String::new();
    for (k, a0) in window_starts(wall_len(v, wall)).into_iter().enumerate() {
        for (j, off) in OFFSETS.iter().enumerate() {
            s.push_str(&wall_rect(
                v,
                wall,
                0.0,
                (a0 + off, a0 + off + 0.22),
                (1.15, HEIGHTS[(j + k) % HEIGHTS.len()]),
                "tower",
            ));
        }
    }
    s
}

// ---------- the owner's office ----------

/// The glass corner office: the owner at a desk facing the floor, an inbox
/// tray on the desk, and a wall of notes, one for each project.
fn owner_office(d: &mut Drawing) {
    let v = d.v;
    let hq = d.floor;
    d.put(GROUND, flat(v, 0.5, 0.5, 5.1, 4.5, "rug rug-owner"));

    // The projects as notes on the west wall.
    let board = d.mount(Wall::West, (0.6, 4.3));
    let mut notes =
        board.legs(v, (0.6, 4.3), 0.0, 1.0) + &board.rect(v, (0.6, 4.3), (1.0, 2.35), "board");
    for (i, t) in hq.teams.iter().take(4).enumerate() {
        let a0 = 0.8 + 0.9 * i as f64;
        notes.push_str(&board.rect(
            v,
            (a0, a0 + 0.75),
            (1.2, 2.15),
            &format!("note pj-{}", t.tint % TINTS),
        ));
        let frac = t.milestone.as_ref().map_or(0.0, |m| share(m.done, m.total));
        if frac > 0.0 {
            notes.push_str(&board.rect(
                v,
                (a0 + 0.1, a0 + 0.1 + 0.55 * frac),
                (1.38, 1.44),
                "note-bar",
            ));
        }
    }
    d.put(board.slot, notes);

    let mut g = Group::new(v);
    let (dx, dy) = (1.4, 2.1);
    g.shade(&shadow(v, dx + 1.4, dy + 0.5, 1.6, 0.5));
    let part = |f: Fp, mat: &str, g: &mut Group| {
        let svg = bx(
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
        g.put(f, svg);
    };
    part(
        fp(dx + 0.05, dy + 0.05, 0.0, 0.1, 0.9, 0.72),
        "m-deskleg",
        &mut g,
    );
    part(
        fp(dx + 2.55, dy + 0.05, 0.0, 0.1, 0.9, 0.72),
        "m-deskleg",
        &mut g,
    );
    part(fp(dx, dy, 0.72, 2.8, 1.0, 0.08), "m-desk", &mut g);
    part(
        fp(dx + 0.7, dy + 0.3, 0.8, 0.62, 0.4, 0.025),
        "m-keys",
        &mut g,
    );
    part(
        fp(dx + 0.7, dy + 0.68, 0.8, 0.62, 0.035, 0.4),
        "m-monitor",
        &mut g,
    );
    part(
        fp(dx + 1.55, dy + 0.34, 0.8, 0.14, 0.14, 0.1),
        "m-mug",
        &mut g,
    );
    part(
        fp(dx + 0.02, dy + 1.0, 0.18, 2.76, 0.04, 0.5),
        "m-tag office-tag",
        &mut g,
    );
    // The tray: a pile of paper for what waits, more when more waits.
    let (tx, ty) = (dx + 1.95, dy + 0.2);
    part(fp(tx, ty, 0.8, 0.8, 0.66, 0.06), "m-metal", &mut g);
    let waiting = hq.owner_needs;
    if waiting > 0 {
        let (sx, sy) = v.p(tx + 0.4, ty + 0.33, 0.8);
        g.put(
            fp(tx, ty, 0.8, 0.8, 0.66, 0.02),
            format!(r#"<ellipse class="needs-glow" cx="{sx:.1}" cy="{sy:.1}" rx="26" ry="13"/>"#),
        );
    }
    let tapes: Vec<usize> = hq
        .teams
        .iter()
        .filter(|t| t.needs > 0)
        .map(|t| t.tint % TINTS)
        .collect();
    for i in 0..waiting.min(2) {
        let y = ty + 0.07 + 0.29 * i as f64;
        let h = 0.07 + 0.04 * waiting.min(4) as f64 * (1 + i) as f64 / 2.0;
        part(fp(tx + 0.07, y, 0.86, 0.66, 0.22, h), "m-paper", &mut g);
        let tape = poly(
            &format!("tape pj-{}", tapes.get(i).copied().unwrap_or(0)),
            &[
                v.p(tx + 0.07, y + 0.07, 0.86 + h + 0.001),
                v.p(tx + 0.73, y + 0.07, 0.86 + h + 0.001),
                v.p(tx + 0.73, y + 0.13, 0.86 + h + 0.001),
                v.p(tx + 0.07, y + 0.13, 0.86 + h + 0.001),
            ],
        );
        g.put(fp(tx + 0.07, y, 0.86 + h, 0.66, 0.22, 0.01), tape);
    }
    // The owner, facing the floor.
    chair(&mut g, 2.9, 1.55);
    let owner = Fig::new(
        2.9,
        1.55,
        0.42,
        Role::Owner,
        "owner",
        v.pose(true, (0.0, 1.0), false),
    )
    .state("idle");
    g.put(spot(2.9, 1.55, 0.42), figure(v, owner));
    g.put(plant_fp(0.7, 4.35, 1.0), plant(v, 0.7, 4.35, 1.0));
    // Glass round the east and south sides.
    part(
        fp(
            OFFICE.2 - 0.06,
            OFFICE.1,
            0.0,
            0.06,
            OFFICE.3 - OFFICE.1,
            2.4,
        ),
        "m-glass",
        &mut g,
    );
    part(
        fp(
            OFFICE.0,
            OFFICE.3 - 0.06,
            0.0,
            OFFICE.2 - OFFICE.0,
            0.06,
            2.4,
        ),
        "m-glass",
        &mut g,
    );
    let slot = d.obj(&fp(OFFICE.0, OFFICE.1, 0.0, OFFICE.2, OFFICE.3, 2.4));
    let label = if waiting == 0 {
        "Your office: nothing waits for you".to_string()
    } else {
        format!("Your office: {waiting} waiting for you")
    };
    d.el(Element::Owner, slot, &g.finish(), &label, (2.9, 1.55, 2.3));
}

/// A person on the company floor.
fn figure(v: View, fig: Fig) -> String {
    person(v, &fig.scaled(FIG_SCALE))
}

/// `done` of `total` as a share from 0 to 1.
fn share(done: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        (done as f64 / total as f64).clamp(0.0, 1.0)
    }
}

/// The bench and the plants between the owner's office and the checks lab.
fn lobby(d: &mut Drawing) {
    let v = d.v;
    let slot = d.obj(&fp(7.2, 3.9, 0.0, 3.4, 0.7, 0.85));
    d.put(
        slot,
        bx(v, 7.2, 3.9, 0.0, 3.4, 0.7, 0.45, "m-sofa", "")
            + &bx(v, 7.2, 3.9, 0.45, 3.4, 0.2, 0.4, "m-sofa2", ""),
    );
    for (x, y, size) in [(6.1, 4.4, 1.1), (11.5, 4.4, 1.0)] {
        let slot = d.obj(&plant_fp(x, y, size));
        d.put(slot, plant(v, x, y, size));
    }
}

// ---------- the checks lab ----------

/// A rack with its lights on the face that looks into the room and a stripe
/// in a project's colour along the top.
fn rack(v: View, x0: f64, h: f64, i: usize, tint: Option<usize>) -> String {
    let mut s = bx(v, x0, 0.15, 0.0, 1.0, 0.9, h, "m-rack", "");
    if v.toward(0.0, 1.0) {
        let mut leds = String::new();
        let rows = ((h - 0.3) / 0.3).floor() as i32;
        for row in 0..rows {
            for col in 0..3 {
                let a = v.p(
                    x0 + 0.2 + f64::from(col) * 0.18,
                    1.05,
                    h - 0.3 - f64::from(row) * 0.3,
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
    }
    if let Some(pj) = tint {
        s.push_str(&poly(
            &format!("stripe pj-{pj}"),
            &[
                v.p(x0 + 0.1, 0.72, h),
                v.p(x0 + 0.9, 0.72, h),
                v.p(x0 + 0.9, 0.92, h),
                v.p(x0 + 0.1, 0.92, h),
            ],
        ));
    }
    s
}

/// A small monitor on a desk, its screen on the face toward +y.
fn monitor(g: &mut Group, x: f64, y: f64, screen: &str) {
    let v = g.v;
    let (w, h, z) = (0.85, 0.5, 0.8);
    g.put(
        fp(x + w / 2.0 - 0.06, y + 0.1, z, 0.12, 0.08, 0.12),
        bx(
            v,
            x + w / 2.0 - 0.06,
            y + 0.1,
            z,
            0.12,
            0.08,
            0.12,
            "m-metal",
            "",
        ),
    );
    let body = fp(x, y, z + 0.12, w, 0.06, h);
    g.put(body, bx(v, x, y, z + 0.12, w, 0.06, h, "m-monitor", ""));
    if v.toward(0.0, 1.0) {
        g.put(
            Fp {
                z0: z + 0.13,
                ..body
            },
            poly(
                &format!("screen {screen}"),
                &[
                    v.p(x + 0.06, y + 0.06, z + 0.17),
                    v.p(x + w - 0.06, y + 0.06, z + 0.17),
                    v.p(x + w - 0.06, y + 0.06, z + 0.12 + h - 0.05),
                    v.p(x + 0.06, y + 0.06, z + 0.12 + h - 0.05),
                ],
            ),
        );
    }
}

/// Racks along the back wall, a bench with screens and devices, and a pin for
/// each ticket running its checks.
fn checks_lab(d: &mut Drawing) {
    let v = d.v;
    let hq = d.floor;
    d.put(GROUND, flat(v, 12.9, 0.5, 17.1, 4.6, "rug rug-qa"));
    let mut g = Group::new(v);
    for (i, (x0, h)) in [(12.7, 2.3), (13.85, 2.3), (15.0, 2.3), (16.2, 1.5)]
        .into_iter()
        .enumerate()
    {
        let tint = hq.teams.get(i).map(|t| t.tint % TINTS);
        g.put(fp(x0, 0.15, 0.0, 1.0, 0.9, h), rack(v, x0, h, i, tint));
    }
    let (bx0, by0) = (13.0, 2.7);
    let screen = if hq.checks.is_empty() { "off" } else { "test" };
    let mut bench = Group::new(v);
    bench.shade(&shadow(v, bx0 + 1.4, by0 + 0.5, 1.7, 0.5));
    for lx in [bx0 + 0.05, bx0 + 2.65] {
        bench.put(
            fp(lx, by0 + 0.05, 0.0, 0.1, 0.8, 0.72),
            bx(v, lx, by0 + 0.05, 0.0, 0.1, 0.8, 0.72, "m-deskleg", ""),
        );
    }
    bench.put(
        fp(bx0, by0, 0.72, 2.8, 0.9, 0.08),
        bx(v, bx0, by0, 0.72, 2.8, 0.9, 0.08, "m-bench", ""),
    );
    monitor(&mut bench, bx0 + 0.2, by0 + 0.2, screen);
    monitor(&mut bench, bx0 + 1.2, by0 + 0.2, screen);
    for (ox, oy) in [(2.2, 0.55), (2.45, 0.35), (2.0, 0.2)] {
        bench.put(
            fp(bx0 + ox, by0 + oy, 0.8, 0.14, 0.24, 0.03),
            bx(v, bx0 + ox, by0 + oy, 0.8, 0.14, 0.24, 0.03, "m-device", ""),
        );
    }
    g.put(fp(bx0, by0, 0.0, 2.8, 0.9, 1.6), bench.finish());
    for f in [
        fp(LAB.2 - 0.06, LAB.1, 0.0, 0.06, LAB.3 - LAB.1, 2.3),
        fp(LAB.0, LAB.3 - 0.06, 0.0, LAB.2 - LAB.0, 0.06, 2.3),
    ] {
        g.put(
            f,
            bx(
                v,
                f.x0,
                f.y0,
                0.0,
                f.x1 - f.x0,
                f.y1 - f.y0,
                2.3,
                "m-glass",
                "",
            ),
        );
    }
    let slot = d.obj(&fp(LAB.0, LAB.1, 0.0, LAB.2 - LAB.0, LAB.3, 2.3));
    d.el(
        Element::Racks,
        slot,
        &g.finish(),
        "Checks lab: every project's checks",
        (14.6, 0.6, 2.75),
    );
    for (i, w) in hq.checks.iter().take(3).enumerate() {
        d.pin_off(
            (14.4, 3.15, 2.0),
            (-0.1 + 1.3 * i as f64, -1.15 - 0.45 * i as f64),
            PinWhat::Ticket(w.clone()),
        );
    }
}

// ---------- the review room ----------

/// The glass room with the round table, and whoever is reading.
fn review_room(d: &mut Drawing) {
    let v = d.v;
    let hq = d.floor;
    d.put(GROUND, flat(v, 18.8, 0.6, 22.5, 4.6, "rug rug-review"));
    for f in [
        fp(
            REVIEW.0,
            REVIEW.3 - 0.06,
            0.0,
            REVIEW.2 - REVIEW.0,
            0.06,
            2.3,
        ),
        fp(REVIEW.0, REVIEW.1, 0.0, 0.06, REVIEW.3 - REVIEW.1, 2.3),
    ] {
        let slot = d.obj(&f);
        d.put(
            slot,
            bx(
                v,
                f.x0,
                f.y0,
                0.0,
                f.x1 - f.x0,
                f.y1 - f.y0,
                2.3,
                "m-glass",
                "",
            ),
        );
    }
    let (cx, cy) = (20.7, 3.0);
    // Seats: the near side, the far side and the right of the table, each
    // facing it.
    let seats: [(f64, f64, (f64, f64), bool); 3] = [
        (cx, cy + 1.45, (0.0, -1.0), false),
        (cx, cy - 1.4, (0.0, 1.0), false),
        (cx + 1.4, cy, (-1.0, 0.0), true),
    ];
    for (i, (x, y, face, profile)) in seats.iter().enumerate() {
        let mut g = Group::new(v);
        chair(&mut g, *x, *y);
        if let Some(w) = hq.review.get(i) {
            d.people.push((*x, *y));
            let state = match &w.doing {
                super::model::Doing::Working => "reading",
                other => iso::doing_state(other),
            };
            let fig = ticket_fig(w, *x, *y, 0.42, v.pose(true, *face, *profile), state);
            g.put(spot(*x, *y, 0.42), d.pj_group(&w.project, &figure(v, fig)));
            d.pin_off((*x, *y, 2.4), (0.0, -1.55), PinWhat::Ticket(w.clone()));
        }
        let slot = d.obj(&fp(x - 0.28, y - 0.28, 0.0, 0.56, 0.56, 1.6));
        d.put(slot, g.finish());
    }
    let (tx, ty) = v.p(cx, cy, 0.75);
    let table_fp = fp(cx - 1.45, cy - 1.45, 0.0, 2.9, 2.9, 0.8);
    let table = shadow(v, cx, cy, 1.5, 0.75)
        + &bx(
            v,
            cx - 0.15,
            cy - 0.15,
            0.0,
            0.3,
            0.3,
            0.75,
            "m-deskleg",
            "",
        )
        + &format!(
            r#"<ellipse class="tabletop" cx="{tx:.1}" cy="{ty:.1}" rx="{:.1}" ry="{:.1}"/>"#,
            1.45 * HX * std::f64::consts::SQRT_2,
            1.45 * HX * std::f64::consts::FRAC_1_SQRT_2
        );
    let slot = d.obj(&table_fp);
    d.el(
        Element::Review,
        slot,
        &table,
        "Review room: reviewers and critics",
        (cx, cy, 1.25),
    );
    let sheet = poly(
        "",
        &[
            v.p(cx - 0.5, cy + 0.2, 0.78),
            v.p(cx + 0.1, cy + 0.2, 0.78),
            v.p(cx + 0.1, cy + 0.65, 0.78),
            v.p(cx - 0.5, cy + 0.65, 0.78),
        ],
    )
    .replacen(r#" class="""#, "", 1);
    let second = poly(
        "p2",
        &[
            v.p(cx + 0.2, cy - 0.5, 0.78),
            v.p(cx + 0.7, cy - 0.5, 0.78),
            v.p(cx + 0.7, cy - 0.1, 0.78),
            v.p(cx + 0.2, cy - 0.1, 0.78),
        ],
    );
    let at = d.after(&fp(cx - 0.5, cy - 0.5, 0.78, 1.2, 1.15, 0.02), &table_fp);
    d.put(at, format!(r#"<g class="papers">{sheet}{second}</g>"#));
    let slot = d.obj(&plant_fp(22.4, 4.5, 0.9));
    d.put(slot, plant(v, 22.4, 4.5, 0.9));
}

// ---------- the lounge ----------

/// A steaming mug standing at a point.
fn mug(v: View, x: f64, y: f64, z: f64) -> String {
    let (sx, sy) = v.p(x, y, z);
    format!(
        r#"<g class="mug"><rect x="{:.1}" y="{:.1}" width="6" height="7" rx="1.5"/><path class="steam" d="M{:.1} {:.1} q2 -4 0 -8 q-2 -4 0 -8"/></g>"#,
        sx - 3.0,
        sy - 7.0,
        sx - 1.0,
        sy - 9.0
    )
}

/// The sofa along the west wall with the free people on it and a free seat to
/// hire into, and a low table.
fn lounge(d: &mut Drawing) {
    let v = d.v;
    let hq = d.floor;
    d.put(GROUND, flat(v, 0.6, 14.4, 3.4, 20.0, "rug rug-lounge"));
    let sofa = fp(0.15, 14.7, 0.0, 0.95, 5.7, 0.92);
    let slot = d.obj(&sofa);
    d.el(
        Element::Lounge,
        slot,
        &(bx(v, 0.15, 14.7, 0.0, 0.95, 5.7, 0.42, "m-sofa", "")
            + &bx(v, 0.15, 14.7, 0.42, 0.28, 5.7, 0.5, "m-sofa2", "")),
        "Lounge: who's free",
        (0.6, 17.5, 1.75),
    );
    // Whoever rests sits facing the floor.
    let mut latest = slot.1;
    for (i, who) in hq.lounge.iter().take(MAX_LOUNGERS).enumerate() {
        let y = 16.45 + 1.1 * i as f64;
        let label = match who.idle_minutes {
            Some(m) => format!("{}, resting {}", who.name, minutes(m)),
            None => format!("{}, resting", who.name),
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
        d.people.push((0.75, y));
        let at = d.with(&spot(0.75, y, 0.42), &sofa);
        latest = latest.max(at.1);
        d.put(at, figure(v, fig));
    }
    // From the west the sofa's back is in front of them.
    if v.toward(-1.0, 0.0) && !hq.lounge.is_empty() {
        let back = bx(v, 0.15, 14.7, 0.42, 0.28, 5.7, 0.5, "m-sofa2", "");
        d.put(
            (Layer::Object, latest + NUDGE),
            format!(r#"<g pointer-events="none">{back}</g>"#),
        );
    }
    // The free seat at the end of the sofa.
    let seat = format!(
        "{}{}",
        ghost_box(v, (0.45, 14.8, 0.55, 0.9), 0.42, 0.14),
        iso::plus(v, 0.72, 15.25, 0.62)
    );
    d.el(
        Element::Hire,
        (Layer::Object, latest + 2.0 * NUDGE),
        &seat,
        "Hire someone",
        (0.72, 15.25, 1.3),
    );
    let table = fp(2.0, 16.3, 0.0, 1.2, 1.6, 0.36);
    let mut g = Group::new(v);
    g.put(table, bx(v, 2.0, 16.3, 0.0, 1.2, 1.6, 0.36, "m-desk", ""));
    for (x, y) in [(2.35, 16.8), (2.8, 17.3)] {
        g.put(spot(x, y, 0.36), mug(v, x, y, 0.36));
    }
    let slot = d.obj(&table);
    d.put(slot, g.finish());
    for (x, y, size) in [(3.2, 20.4, 1.0), (3.15, 14.6, 0.9)] {
        let slot = d.obj(&plant_fp(x, y, size));
        d.put(slot, plant(v, x, y, size));
    }
}

// ---------- the empty area ----------

/// Outlines of the desks that will go in the area for a new project, and a
/// "+": the control that adds one.
fn new_project(d: &mut Drawing, (x0, y0): Pt) {
    let v = d.v;
    let mut s = flat(
        v,
        x0 + 0.1,
        y0 + 0.1,
        x0 + POD_U - 0.1,
        y0 + POD_V - 0.1,
        "ghost",
    );
    // The lead's desk at the head and two for the team.
    for (u, w, dx, dy) in [
        (1.0, 1.45, 1.0, 2.5),
        (2.5, 1.0, 1.6, 0.8),
        (4.8, 1.0, 1.6, 0.8),
    ] {
        s.push_str(&flat(
            v,
            x0 + u,
            y0 + w,
            x0 + u + dx,
            y0 + w + dy,
            "ghost-desk",
        ));
    }
    let (px, py) = v.p(x0 + 2.2, y0 + 3.0, 0.0);
    s.push_str(&format!(
        r#"<g class="plus big" transform="translate({px:.1} {py:.1})"><rect x="-2" y="-11" width="4" height="22" rx="2"/><rect x="-11" y="-2" width="22" height="4" rx="2"/></g>"#
    ));
    d.el(
        Element::AddProject,
        GROUND,
        &s,
        "Add a project",
        (x0 + 2.2, y0 + 3.0, 0.8),
    );
}

/// The plants and the printer round the team areas.
fn furnish(d: &mut Drawing, rows: usize) {
    let v = d.v;
    for r in 0..rows {
        let y0 = POD_Y + PITCH_Y * r as f64;
        for (x, y, size) in [(21.6, y0 + 2.2, 1.1), (21.5, y0 + POD_V + 1.0, 0.9)] {
            let slot = d.obj(&plant_fp(x, y, size));
            d.put(slot, plant(v, x, y, size));
        }
    }
    let f = fp(20.6, 11.0, 0.0, 0.8, 0.6, 0.97);
    let slot = d.obj(&f);
    d.put(
        slot,
        bx(v, 20.6, 11.0, 0.0, 0.8, 0.6, 0.85, "m-metal", "")
            + &bx(v, 20.68, 11.06, 0.85, 0.64, 0.48, 0.12, "m-paper", ""),
    );
    let slot = d.obj(&plant_fp(11.2, v.d - 1.0, 0.9));
    d.put(slot, plant(v, 11.2, v.d - 1.0, 0.9));
}

// ---------- a team area ----------

/// What a phase is called in a label.
pub(super) fn phase_key(phase: Phase) -> &'static str {
    match phase {
        Phase::Idle => "idle",
        Phase::Planning => "planning",
        Phase::Building => "building",
        Phase::Testing => "testing",
        Phase::Releasing => "releasing",
        Phase::Live => "live",
        Phase::Paused => "paused",
    }
}

/// What a team area is doing, in words.
fn phase_words(t: &Team) -> String {
    match t.phase {
        Phase::Idle => "nothing open".to_string(),
        Phase::Planning => "planning".to_string(),
        Phase::Building => "building".to_string(),
        Phase::Testing => "testing".to_string(),
        Phase::Releasing => match &t.release {
            Some(r) => format!("releasing {}", r.version),
            None => "releasing".to_string(),
        },
        Phase::Live => match &t.live {
            Some(v) => format!("live, {v}"),
            None => "live".to_string(),
        },
        Phase::Paused => "paused".to_string(),
    }
}

/// The box of a footprint given in view coordinates, as a world footprint.
fn view_fp(v: View, (u0, u1): Pt, (v0, v1): Pt, z: f64, h: f64) -> Fp {
    let (xa, ya) = v.xy(u0, v0);
    let (xb, yb) = v.xy(u1, v1);
    fp(
        xa.min(xb),
        ya.min(yb),
        z,
        (xa - xb).abs(),
        (ya - yb).abs(),
        h,
    )
}

/// A desk whose worker sits on the far side and faces the head of the floor:
/// a laptop with its lid toward the room.
fn desk_front(g: &mut Group, x: f64, y: f64, screen: &str) {
    let v = g.v;
    g.shade(&shadow(v, x + 0.8, y + 0.4, 1.05, 0.4));
    let mut part = |f: Fp, mat: &str, extra: &str| {
        let svg = bx(
            v,
            f.x0,
            f.y0,
            f.z0,
            f.x1 - f.x0,
            f.y1 - f.y0,
            f.z1 - f.z0,
            mat,
            extra,
        );
        g.put(f, svg);
    };
    part(fp(x, y, 0.0, 1.6, 0.8, 0.07), "m-deskleg", "");
    part(
        fp(x + 0.05, y + 0.05, 0.0, 0.08, 0.7, 0.72),
        "m-deskleg",
        "",
    );
    part(
        fp(x + 1.47, y + 0.05, 0.0, 0.08, 0.7, 0.72),
        "m-deskleg",
        "",
    );
    part(fp(x, y, 0.72, 1.6, 0.8, 0.07), "m-desk", "");
    part(fp(x + 0.5, y + 0.22, 0.79, 0.62, 0.4, 0.025), "m-keys", "");
    part(
        fp(x + 0.5, y + 0.6, 0.79, 0.62, 0.035, 0.4),
        "m-monitor",
        &format!(r#" data-lid="{screen}""#),
    );
    part(fp(x + 1.2, y + 0.3, 0.79, 0.14, 0.14, 0.1), "m-mug", "");
}

/// The lead's bigger desk, long side along y: the lead sits on its west side.
fn lead_desk(g: &mut Group, x: f64, y: f64) {
    let v = g.v;
    g.shade(&shadow(v, x + 0.5, y + 1.25, 0.9, 1.45));
    let mut part = |f: Fp, mat: &str, extra: &str| {
        let svg = bx(
            v,
            f.x0,
            f.y0,
            f.z0,
            f.x1 - f.x0,
            f.y1 - f.y0,
            f.z1 - f.z0,
            mat,
            extra,
        );
        g.put(f, svg);
    };
    part(
        fp(x + 0.05, y + 0.05, 0.0, 0.08, 2.4, 0.72),
        "m-deskleg",
        "",
    );
    part(fp(x + 0.9, y + 0.05, 0.0, 0.08, 2.4, 0.72), "m-deskleg", "");
    part(fp(x, y, 0.72, 1.0, 2.5, 0.07), "m-desk", "");
    part(fp(x + 0.2, y + 0.75, 0.79, 0.42, 0.62, 0.025), "m-keys", "");
    part(
        fp(x + 0.58, y + 0.75, 0.79, 0.04, 0.62, 0.4),
        "m-monitor",
        r#" data-lid="on""#,
    );
    part(fp(x + 0.86, y + 1.85, 0.79, 0.06, 0.46, 0.15), "m-tag", "");
    part(fp(x + 0.3, y + 2.0, 0.79, 0.14, 0.14, 0.1), "m-mug", "");
}

/// The lead's chair: bigger than the rest, its back in the project's colour.
fn lead_chair(g: &mut Group, x: f64, y: f64) {
    let v = g.v;
    g.shade(&shadow(v, x, y, 0.5, 0.26));
    g.put(
        fp(x - 0.34, y - 0.34, 0.0, 0.68, 0.68, 0.42),
        bx(v, x - 0.34, y - 0.34, 0.0, 0.68, 0.68, 0.42, "m-chair", ""),
    );
    g.put(
        fp(x - 0.42, y - 0.34, 0.42, 0.12, 0.68, 0.95),
        bx(v, x - 0.42, y - 0.34, 0.42, 0.12, 0.68, 0.95, "m-tag", ""),
    );
}

/// One team area: the rug, partitions, the lead's desk and the members' desks,
/// and the prop for the project's phase.
fn team_area(d: &mut Drawing, t: &Team, (x0, y0): Pt) {
    let v = d.v;
    let hq = d.floor;
    let pj = t.tint % TINTS;
    let mut g = Group::new(v);

    // Under everything: the rug, its edge and the light over it.
    g.shade(&flat(
        v,
        x0 + 0.1,
        y0 + 0.1,
        x0 + POD_U - 0.1,
        y0 + POD_V - 0.1,
        "team-rug",
    ));
    g.shade(&flat(
        v,
        x0 + 0.3,
        y0 + 0.3,
        x0 + POD_U - 0.3,
        y0 + POD_V - 0.3,
        "rug-edge",
    ));
    g.shade(&pool(v, x0 + POD_U / 2.0, y0 + POD_V / 2.0, 3.0));

    // Low partitions on the head and the left side, with a rail in the
    // project's colour.
    for (x, y, w, dd) in [
        (x0, y0 + 0.15, 0.14, POD_V - 0.3),
        (x0 + 0.15, y0, POD_U - 0.3, 0.14),
    ] {
        g.put(
            fp(x, y, 0.0, w, dd, 0.85),
            bx(v, x, y, 0.0, w, dd, 0.85, "m-part", ""),
        );
        g.put(
            fp(x, y, 0.85, w, dd, 0.07),
            bx(v, x, y, 0.85, w, dd, 0.07, "m-tag", ""),
        );
    }

    // The lead, at the head.
    let mut lg = Group::new(v);
    lead_desk(&mut lg, x0 + 1.0, y0 + 1.45);
    let (lx, ly) = (x0 + 0.62, y0 + 2.7);
    lead_chair(&mut lg, lx, ly);
    if let Some(lead) = &t.lead {
        let state = if t.phase == Phase::Planning {
            "writing"
        } else {
            "idle"
        };
        let fig = Fig::new(
            lx,
            ly,
            0.42,
            Role::Lead,
            &lead.hire,
            v.pose(true, (1.0, 0.0), false),
        )
        .state(state)
        .control(
            key("hires", &lead.hire),
            format!("{}, leads {}", lead.name, t.project),
        );
        lg.put(spot(lx, ly, 0.42), figure(v, fig));
        d.people.push((lx, ly));
    }
    g.put(fp(x0 + 0.2, y0 + 1.45, 0.0, 1.8, 2.5, 1.8), lg.finish());

    // The members, at up to four desks.
    let mut seated: [Option<&Worker>; 4] = [None; 4];
    for (k, w) in t.members.iter().take(SEATS.len()).enumerate() {
        seated[SEAT_ORDER[k]] = Some(w);
    }
    for (i, (u, w, front)) in SEATS.iter().enumerate() {
        let (dx, dy) = (x0 + u, y0 + w);
        let who = seated[i];
        let screen = who.map_or("off", |w| doing_screen(&w.doing));
        let mut sg = Group::new(v);
        let (cx, cy, face) = if *front {
            desk_front(&mut sg, dx, dy, screen);
            (dx + 0.8, dy - 0.4, (0.0, 1.0))
        } else {
            desk(&mut sg, dx, dy, screen);
            (dx + 0.8, dy + 1.25, (0.0, -1.0))
        };
        chair(&mut sg, cx, cy);
        if let Some(w) = who {
            d.people.push((cx, cy));
            let state = super::iso::doing_state(&w.doing);
            let fig = ticket_fig(w, cx, cy, 0.42, v.pose(true, face, false), state);
            sg.put(spot(cx, cy, 0.42), figure(v, fig));
        }
        let foot = if *front {
            fp(dx, dy - 0.7, 0.0, 1.6, 1.5, 1.6)
        } else {
            fp(dx, dy, 0.0, 1.6, 1.53, 1.6)
        };
        g.put(foot, sg.finish());
    }

    prop(&mut g, hq, t, (x0, y0));

    g.put(
        plant_fp(x0 + POD_U - 0.35, y0 + POD_V - 0.4, 0.8),
        plant(v, x0 + POD_U - 0.35, y0 + POD_V - 0.4, 0.8),
    );

    // Members beyond the desks there are.
    let mut inner = g.finish();
    if t.members.len() > SEATS.len() {
        let (sx, sy) = v.p(x0 + POD_U - 1.3, y0 + POD_V - 0.55, 0.0);
        inner.push_str(&format!(
            r#"<g class="more" transform="translate({sx:.1} {sy:.1})"><rect x="-17" y="-9" width="34" height="18" rx="9"/><text y="4" text-anchor="middle">+{}</text></g>"#,
            t.members.len() - SEATS.len()
        ));
    }
    let release = t.release.as_ref().filter(|_| t.phase == Phase::Releasing);
    let mut flags = String::new();
    if release.is_some_and(|r| r.waiting) {
        flags.push_str(" data-waiting");
    }
    if release.is_some_and(|r| r.failed) {
        flags.push_str(" data-failed");
    }
    let label = format!("{}, {}", t.project, phase_words(t));
    let svg = format!(
        r#"<g class="pod pj-{pj}" data-pj="{pj}" data-phase="{}"{flags} {}>{inner}</g>"#,
        phase_key(t.phase),
        control(&key("projects", &t.project), &label)
    );
    let slot = d.obj(&fp(x0, y0, 0.0, POD_U, POD_V, 1.0));
    d.put(slot, svg);
    if tester(hq, t).is_some() {
        d.people.push((x0 + 3.7, y0 + 3.15));
    }
}

// ---------- what a phase puts in the area ----------

/// A phase's prop, marked as such.
fn prop_tag(phase: &str, inner: &str) -> String {
    format!(r#"<g class="prop prop-{phase}">{inner}</g>"#)
}

/// The prop that shows `t`'s phase, between the rows of desks.
fn prop(g: &mut Group, hq: &Hq, t: &Team, (x0, y0): Pt) {
    let v = g.v;
    let (px, py) = (x0 + 3.8, y0 + 2.6);
    match t.phase {
        Phase::Idle => {}
        Phase::Planning => planning_prop(g, (x0 + 4.25, y0 + 2.5)),
        Phase::Building => {
            let (cx, cy) = (x0 + 6.2, y0 + 2.5);
            g.put(plant_fp(cx, cy, 0.8), plant(v, cx, cy, 0.8));
            g.put(
                fp(x0 + 3.4, y0 + 2.2, 0.0, 0.5, 0.5, 0.6),
                prop_tag(
                    "building",
                    &(bx(v, x0 + 3.4, y0 + 2.2, 0.0, 0.5, 0.5, 0.5, "m-desk", "")
                        + &mug(v, x0 + 3.65, y0 + 2.45, 0.5)),
                ),
            );
        }
        Phase::Testing => testing_prop(g, hq, t, (x0, y0)),
        Phase::Releasing => releasing_prop(g, t, (px, py)),
        Phase::Live => live_prop(g, t, (px, py)),
        Phase::Paused => paused_prop(g, (px, py)),
    }
}

/// A whiteboard on a stand with notes, facing the camera from every corner.
fn planning_prop(g: &mut Group, (cx, cy): Pt) {
    let v = g.v;
    let (cu, cv) = v.uv(cx, cy);
    let (u0, u1) = (cu - 1.25, cu + 1.25);
    let (v0, v1) = (cv - 0.035, cv + 0.035);
    g.shade(&poly(
        "mat mat-planning",
        &[
            v.s(u0 - 0.2, cv - 0.5, 0.0),
            v.s(u1 + 0.2, cv - 0.5, 0.0),
            v.s(u1 + 0.2, cv + 0.6, 0.0),
            v.s(u0 - 0.2, cv + 0.6, 0.0),
        ],
    ));
    let mut s = String::new();
    for lu in [u0 + 0.1, u1 - 0.18] {
        s.push_str(&bx_view(
            v,
            (lu, lu + 0.08),
            (cv - 0.04, cv + 0.04),
            0.0,
            0.5,
            "m-metal",
        ));
    }
    s.push_str(&bx_view(v, (u0, u1), (v0, v1), 0.5, 1.1, "m-board"));
    for (i, (du, z)) in [
        (0.2, 1.05),
        (0.95, 1.05),
        (1.7, 1.05),
        (0.2, 0.62),
        (0.95, 0.62),
    ]
    .into_iter()
    .enumerate()
    {
        s.push_str(&poly(
            &format!("stick s{}", i % 2),
            &[
                v.s(u0 + du, v1, z),
                v.s(u0 + du + 0.5, v1, z),
                v.s(u0 + du + 0.5, v1, z + 0.38),
                v.s(u0 + du, v1, z + 0.38),
            ],
        ));
    }
    g.put(
        view_fp(v, (u0, u1), (cv - 0.3, cv + 0.3), 0.0, 1.6),
        prop_tag("planning", &s),
    );
}

/// The first ticket of the project that is running its checks, whose person
/// tests at the bench.
fn tester<'a>(hq: &'a Hq, t: &Team) -> Option<&'a Worker> {
    hq.checks
        .iter()
        .find(|w| w.project == t.project && !t.members.iter().any(|m| m.ticket == w.ticket))
}

/// A bench with two screens and some devices, and whoever tests at it.
fn testing_prop(g: &mut Group, hq: &Hq, t: &Team, (x0, y0): Pt) {
    let v = g.v;
    let (bx0, by0) = (x0 + 2.5, y0 + 1.95);
    g.shade(&flat(
        v,
        bx0 - 0.2,
        by0 - 0.2,
        bx0 + 2.7,
        by0 + 1.75,
        "mat mat-testing",
    ));
    let mut bench = Group::new(v);
    for lx in [bx0 + 0.05, bx0 + 2.35] {
        bench.put(
            fp(lx, by0 + 0.05, 0.0, 0.1, 0.6, 0.72),
            bx(v, lx, by0 + 0.05, 0.0, 0.1, 0.6, 0.72, "m-deskleg", ""),
        );
    }
    bench.put(
        fp(bx0, by0, 0.72, 2.5, 0.7, 0.08),
        bx(v, bx0, by0, 0.72, 2.5, 0.7, 0.08, "m-bench", ""),
    );
    monitor(&mut bench, bx0 + 0.15, by0 + 0.1, "test");
    monitor(&mut bench, bx0 + 1.0, by0 + 0.1, "test");
    for (ox, oy) in [(1.95, 0.4), (2.2, 0.2), (1.85, 0.15)] {
        bench.put(
            fp(bx0 + ox, by0 + oy, 0.8, 0.14, 0.24, 0.03),
            bx(v, bx0 + ox, by0 + oy, 0.8, 0.14, 0.24, 0.03, "m-device", ""),
        );
    }
    g.put(
        fp(bx0, by0, 0.0, 2.5, 0.7, 1.6),
        prop_tag("testing", &bench.finish()),
    );
    if let Some(w) = tester(hq, t) {
        let (tx, ty) = (x0 + 3.7, y0 + 3.15);
        let fig = ticket_fig(
            w,
            tx,
            ty,
            0.0,
            v.pose(false, (0.0, -1.0), false),
            super::iso::doing_state(&w.doing),
        );
        g.put(spot(tx, ty, 0.0), figure(v, fig));
    }
}

/// A launch crate on a pad inside a ring that fills as the release goes, with
/// the version over it.
fn releasing_prop(g: &mut Group, t: &Team, (px, py): Pt) {
    let v = g.v;
    let fallback = Launch {
        version: t
            .milestone
            .as_ref()
            .and_then(|m| m.version.clone())
            .unwrap_or_default(),
        done: 0,
        total: 1,
        waiting: false,
        failed: false,
    };
    let r = t.release.as_ref().unwrap_or(&fallback);
    let pct = (share(r.done, r.total) * 100.0).round() as u32;
    g.shade(&disc(v, px, py, 0.98, "ring-track", ""));
    g.shade(&disc(
        v,
        px,
        py,
        0.98,
        "ring-fill",
        &format!(
            r#" pathLength="100" stroke-dasharray="{pct} {}""#,
            100 - pct
        ),
    ));
    let mut s = bx(v, px - 0.72, py - 0.62, 0.0, 1.44, 1.24, 0.07, "m-pad", "");
    s.push_str(&bx(
        v,
        px - 0.38,
        py - 0.38,
        0.07,
        0.76,
        0.76,
        0.7,
        "m-launch",
        "",
    ));
    s.push_str(&vface(
        v,
        (px - 0.38, py - 0.38, 0.76, 0.76),
        (0.12, 0.64),
        (0.28, 0.46),
        &format!("tape pj-{}", t.tint % TINTS),
    ));
    let (ax, ay) = v.p(px, py, 1.2);
    s.push_str(&format!(
        r#"<g class="rocket" transform="translate({ax:.1} {ay:.1})"><path d="M0 -13 L8 3 L0 -1 L-8 3z"/></g>"#
    ));
    if !r.version.is_empty() {
        let width = 12.0 + 7.0 * r.version.chars().count() as f64;
        s.push_str(&format!(
            r#"<g class="ver" transform="translate({ax:.1} {:.1})"><rect x="{:.1}" y="-9" width="{width:.1}" height="17" rx="8.5"/><text y="3.5" text-anchor="middle">{}</text></g>"#,
            ay - 28.0,
            -width / 2.0,
            iso::esc(&r.version)
        ));
    }
    g.put(
        fp(px - 0.72, py - 0.62, 0.0, 1.44, 1.24, 1.4),
        prop_tag("releasing", &s),
    );
}

/// A plaque with the version on it, and a calm light.
fn live_prop(g: &mut Group, t: &Team, (px, py): Pt) {
    let v = g.v;
    g.shade(&disc(v, px, py, 0.95, "mat mat-live", ""));
    let (cu, cv) = v.uv(px, py);
    let mut s = bx(v, px - 0.3, py - 0.3, 0.0, 0.6, 0.6, 0.1, "m-pad", "");
    s.push_str(&bx(
        v,
        px - 0.05,
        py - 0.05,
        0.1,
        0.1,
        0.1,
        0.55,
        "m-metal",
        "",
    ));
    s.push_str(&bx_view(
        v,
        (cu - 0.75, cu + 0.75),
        (cv - 0.05, cv + 0.05),
        0.65,
        0.55,
        "m-plaque",
    ));
    if let Some(version) = &t.live {
        let (sx, sy) = v.s(cu - 0.66, cv + 0.05, 0.8);
        let len = (version.chars().count() as f64 * 9.0).min(38.0);
        s.push_str(&format!(
            r#"<text class="plaque-text" transform="matrix(1 .5 0 1 {sx:.1} {sy:.1})" textLength="{len:.0}" lengthAdjust="spacingAndGlyphs">{}</text>"#,
            iso::esc(version)
        ));
    }
    s.push_str(&bx(
        v,
        px + 1.0,
        py - 0.08,
        0.0,
        0.16,
        0.16,
        0.45,
        "m-metal",
        "",
    ));
    let (lx, ly) = v.p(px + 1.08, py, 0.62);
    s.push_str(&format!(
        r#"<circle class="calm" cx="{lx:.1}" cy="{ly:.1}" r="5"/>"#
    ));
    g.put(
        fp(px - 0.8, py - 0.6, 0.0, 2.0, 1.2, 1.2),
        prop_tag("live", &s),
    );
}

/// A pause sign on a pad.
fn paused_prop(g: &mut Group, (px, py): Pt) {
    let v = g.v;
    g.shade(&disc(v, px, py, 0.95, "mat mat-paused", ""));
    let (cu, cv) = v.uv(px, py);
    let mut s = bx(v, px - 0.4, py - 0.4, 0.0, 0.8, 0.8, 0.07, "m-pad", "");
    s.push_str(&bx_view(
        v,
        (cu - 0.4, cu + 0.4),
        (cv - 0.035, cv + 0.035),
        0.07,
        0.8,
        "m-plaque",
    ));
    let (ax, ay) = v.s(cu, cv + 0.035, 0.47);
    s.push_str(&format!(
        r#"<g class="pausebars" transform="translate({ax:.1} {ay:.1})"><rect x="-9" y="-9" width="6" height="18" rx="1.5"/><rect x="3" y="-9" width="6" height="18" rx="1.5"/></g>"#
    ));
    g.put(
        fp(px - 0.5, py - 0.5, 0.0, 1.0, 1.0, 1.0),
        prop_tag("paused", &s),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pages::office::model::{Doing, Milestone, Person};

    fn worker(id: &str, project: &str, hire: &str, role: Role, doing: Doing) -> Worker {
        Worker {
            ticket: id.to_string(),
            project: project.to_string(),
            title: format!("Title of {id}"),
            hire: hire.to_string(),
            name: hire.to_string(),
            role,
            doing,
            progress: 0.5,
        }
    }

    fn who(hire: &str, role: Role) -> Person {
        Person {
            hire: hire.to_string(),
            name: hire.to_string(),
            role,
            idle_minutes: Some(5),
        }
    }

    /// A team in `phase` with `members` at work.
    fn team(name: &str, tint: usize, phase: Phase, members: usize) -> Team {
        Team {
            project: name.to_string(),
            tint,
            phase,
            needs: usize::from(tint == 0),
            lead: Some(who(&format!("lead-{name}"), Role::Lead)),
            members: (0..members)
                .map(|i| {
                    worker(
                        &format!("{}-{i}", name.to_uppercase()),
                        name,
                        &format!("maker-{name}-{i}"),
                        [Role::Engineer, Role::Designer, Role::Writer][i % 3],
                        match i % 3 {
                            1 => Doing::Asking("Run a command".to_string()),
                            _ => Doing::Working,
                        },
                    )
                })
                .collect(),
            milestone: Some(Milestone {
                key: format!("goals/{name}-1"),
                title: "Boss fight".to_string(),
                version: Some("v0.4".to_string()),
                done: 6,
                total: 10,
            }),
            release: (phase == Phase::Releasing).then(|| Launch {
                version: "v1.2".to_string(),
                done: 4,
                total: 5,
                waiting: true,
                failed: false,
            }),
            live: (phase == Phase::Live).then(|| "v2.0".to_string()),
        }
    }

    fn floor(phases: &[Phase]) -> Hq {
        let names = ["moonlight", "garden", "studio", "arcade", "harbor", "atlas"];
        Hq {
            teams: phases
                .iter()
                .enumerate()
                .map(|(i, p)| team(names[i % names.len()], i, *p, [3, 1, 0, 5, 2, 4][i % 6]))
                .collect(),
            owner_needs: 2,
            lounge: vec![
                who("tess", Role::Lead),
                who("ida", Role::Designer),
                who("pip", Role::Engineer),
            ],
            review: vec![worker(
                "REX-1",
                "moonlight",
                "rex",
                Role::Reviewer,
                Doing::Working,
            )],
            checks: vec![
                worker("CHK-1", "moonlight", "ci", Role::Engineer, Doing::Working),
                worker("CHK-2", "garden", "ci2", Role::Engineer, Doing::Working),
            ],
            closed: false,
        }
    }

    use Phase::*;

    const ALL_PHASES: [Phase; 7] = [Idle, Planning, Building, Testing, Releasing, Live, Paused];
    const TURNS: [i64; 4] = [0, 1, 2, 3];

    fn count(s: &str, what: &str) -> usize {
        s.matches(what).count()
    }

    fn signs(scene: &Scene) -> Vec<&Team> {
        scene
            .pins
            .iter()
            .filter_map(|p| match &p.what {
                PinWhat::Sign(t) => Some(&**t),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn rendering_twice_gives_the_same_string() {
        for hq in [
            Hq::default(),
            floor(&[Building, Releasing, Live]),
            floor(&[Planning, Testing, Paused, Idle, Building, Live]),
        ] {
            for t in TURNS {
                assert_eq!(render(&hq, Turn::new(t)), render(&hq, Turn::new(t)));
            }
        }
    }

    #[test]
    fn nothing_to_eight_teams_draw_one_sign_each() {
        for n in 0..=MAX_TEAMS {
            let hq = floor(&vec![Building; n]);
            for t in TURNS {
                let scene = render(&hq, Turn::new(t));
                let drawn = signs(&scene);
                assert_eq!(drawn.len(), n, "{n} teams, turn {t}");
                for (sign, team) in drawn.iter().zip(&hq.teams) {
                    assert_eq!(*sign, team);
                }
                assert_eq!(count(&scene.svg, r#"class="pod "#), n);
                for p in &scene.pins {
                    assert!((0.0..=100.0).contains(&p.left), "turn {t}: {p:?}");
                    assert!((0.0..=100.0).contains(&p.top), "turn {t}: {p:?}");
                }
                assert!(!scene.svg.contains("<script"));
            }
        }
        // Teams past the eighth are not drawn.
        let many = floor(&[Building; MAX_TEAMS + 3]);
        assert_eq!(signs(&render(&many, Turn::default())).len(), MAX_TEAMS);
    }

    #[test]
    fn every_element_has_a_hint_in_every_turn() {
        for n in [0, 1, 3, 6] {
            for t in TURNS {
                let scene = render(&floor(&vec![Building; n]), Turn::new(t));
                let hints: Vec<Element> = scene
                    .pins
                    .iter()
                    .filter_map(|p| match p.what {
                        PinWhat::Hint(e) => Some(e),
                        _ => None,
                    })
                    .collect();
                assert_eq!(hints.len(), count(&scene.svg, r#"data-el=""#));
                assert_eq!(hints.len(), Element::ALL.len());
                for e in Element::ALL {
                    let key = format!(r#"data-el="{}""#, e.key());
                    assert_eq!(count(&scene.svg, &key), 1, "{e:?}, {n} teams, turn {t}");
                    assert_eq!(hints.iter().filter(|h| **h == e).count(), 1, "{e:?}");
                    assert!(
                        scene
                            .svg
                            .contains(&format!(r#"data-sel="elements/{}""#, e.key()))
                    );
                }
            }
        }
    }

    #[test]
    fn each_phase_draws_its_prop() {
        for phase in ALL_PHASES {
            for t in TURNS {
                let svg = render(&floor(&[phase]), Turn::new(t)).svg;
                for other in ALL_PHASES {
                    let want = usize::from(other == phase && other != Idle);
                    assert_eq!(
                        count(&svg, &format!(r#"prop-{}""#, phase_key(other))),
                        want,
                        "{phase:?} area, {other:?} prop, turn {t}"
                    );
                }
                assert!(svg.contains(&format!(r#"data-phase="{}""#, phase_key(phase))));
            }
        }
    }

    #[test]
    fn a_release_shows_how_far_it_is_and_its_version() {
        let mut hq = floor(&[Releasing]);
        let svg = render(&hq, Turn::default()).svg;
        assert!(svg.contains(r#"stroke-dasharray="80 20""#), "{svg}");
        assert!(svg.contains(">v1.2<"));
        assert!(svg.contains(" data-waiting"));
        assert!(!svg.contains(" data-failed"));
        if let Some(r) = hq.teams[0].release.as_mut() {
            r.done = 0;
            r.total = 0;
            r.waiting = false;
            r.failed = true;
        }
        let svg = render(&hq, Turn::default()).svg;
        assert!(svg.contains(r#"stroke-dasharray="0 100""#));
        assert!(svg.contains(" data-failed") && !svg.contains(" data-waiting"));
        // A project live shows its version on a plaque.
        let svg = render(&floor(&[Live]), Turn::default()).svg;
        assert!(svg.contains("plaque-text") && svg.contains(">v2.0<"));
    }

    #[test]
    fn the_tester_stands_at_the_bench_once() {
        for t in TURNS {
            let svg = render(&floor(&[Testing, Testing]), Turn::new(t)).svg;
            // Only the first team has a ticket running its checks.
            assert_eq!(count(&svg, r#"id="w-CHK-1""#), 1, "turn {t}");
            assert_eq!(count(&svg, r#"data-sel="tickets/CHK-1""#), 1, "turn {t}");
            assert_eq!(count(&svg, r#"id="w-CHK-2""#), 1, "turn {t}");
        }
        let svg = render(&floor(&[Building]), Turn::default()).svg;
        assert_eq!(count(&svg, r#"id="w-CHK-1""#), 0);
    }

    #[test]
    fn members_past_four_show_as_a_count() {
        for (members, seated) in [(0usize, 0usize), (3, 3), (4, 4), (5, 4), (9, 4)] {
            let mut hq = floor(&[Building]);
            hq.teams[0].members = (0..members)
                .map(|i| {
                    worker(
                        &format!("M-{i}"),
                        "moonlight",
                        &format!("m{i}"),
                        Role::Engineer,
                        Doing::Working,
                    )
                })
                .collect();
            for t in TURNS {
                let svg = render(&hq, Turn::new(t)).svg;
                assert_eq!(count(&svg, r#"data-sel="tickets/M-"#), seated, "{members}");
                let more = members.saturating_sub(4);
                if more > 0 {
                    assert!(svg.contains(&format!(">+{more}<")), "{members}");
                    assert_eq!(count(&svg, r#"class="more""#), 1);
                } else {
                    assert_eq!(count(&svg, r#"class="more""#), 0);
                }
            }
        }
    }

    #[test]
    fn a_team_area_is_a_project_control_with_its_people_in_it() {
        let hq = floor(&[Building, Releasing, Live]);
        for t in TURNS {
            let svg = render(&hq, Turn::new(t)).svg;
            for (i, team) in hq.teams.iter().enumerate() {
                let head = format!(
                    r#"<g class="pod pj-{i}" data-pj="{i}" data-phase="{}"{} data-sel="projects/{}""#,
                    phase_key(team.phase),
                    if team.phase == Releasing {
                        " data-waiting"
                    } else {
                        ""
                    },
                    team.project
                );
                assert_eq!(count(&svg, &head), 1, "{head}, turn {t}");
                assert_eq!(
                    count(&svg, &format!(r#"data-sel="projects/{}""#, team.project)),
                    1
                );
                for w in &team.members {
                    assert_eq!(
                        count(&svg, &format!(r#"data-sel="tickets/{}""#, w.ticket)),
                        1,
                        "{} in turn {t}",
                        w.ticket
                    );
                }
                if let Some(lead) = &team.lead {
                    assert_eq!(
                        count(&svg, &format!(r#"data-sel="hires/{}""#, lead.hire)),
                        1
                    );
                }
            }
            for who in &hq.lounge {
                assert_eq!(count(&svg, &format!(r#"data-sel="hires/{}""#, who.hire)), 1);
            }
            assert!(svg.contains(r#"data-sel="elements/add-project""#));
            assert!(svg.contains(r#"data-sel="elements/hire""#));
        }
    }

    #[test]
    fn leads_wear_a_tie_and_the_owner_sits_in_the_glass_office() {
        let mut hq = floor(&[Building, Planning, Live]);
        hq.lounge.clear();
        for t in TURNS {
            let svg = render(&hq, Turn::new(t)).svg;
            assert_eq!(count(&svg, "agent r-lead"), 3, "turn {t}");
            assert_eq!(count(&svg, "agent r-own"), 1, "turn {t}");
            // Seen from behind there is no tie to see: from the east corner
            // all of them are.
            assert_eq!(count(&svg, r#"class="tie""#) == 0, t == 2, "turn {t}");
        }
        // A team with no lead has an empty chair at the head.
        hq.teams[0].lead = None;
        let svg = render(&hq, Turn::default()).svg;
        assert_eq!(count(&svg, "agent r-lead"), 2);
    }

    #[test]
    fn the_owners_tray_glows_while_something_waits() {
        let mut hq = floor(&[Building]);
        hq.owner_needs = 0;
        assert_eq!(count(&render(&hq, Turn::default()).svg, "needs-glow"), 0);
        hq.owner_needs = 3;
        assert_eq!(count(&render(&hq, Turn::default()).svg, "needs-glow"), 1);
    }

    #[test]
    fn the_areas_stay_clear_of_each_other_and_of_the_shared_rooms() {
        let apart = |a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)| {
            a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1
        };
        for cells in 1..=MAX_TEAMS + 1 {
            let depth = depth_for(cells);
            let rects: Vec<(f64, f64, f64, f64)> = (0..cells)
                .map(|i| {
                    let (x, y) = origin(i);
                    (x, y, x + POD_U, y + POD_V)
                })
                .collect();
            for (i, a) in rects.iter().enumerate() {
                assert!(a.2 <= FLOOR_W - 0.5 && a.3 <= depth - 1.0, "{cells} cells");
                for room in [OFFICE, LAB, REVIEW, LOUNGE] {
                    assert!(apart(*a, room), "{cells} cells, area {i}, {room:?}");
                }
                for b in &rects[i + 1..] {
                    assert!(apart(*a, *b), "{cells} cells, {a:?} {b:?}");
                }
            }
        }
        // Up to four areas and the empty one fit the first two rows.
        assert_eq!(depth_for(1), depth_for(4));
        assert!(depth_for(5) > depth_for(4));
        assert_eq!(depth_for(5), depth_for(6));
    }

    #[test]
    fn the_floor_is_as_big_as_its_rows_need_in_every_turn() {
        for (teams, rows) in [(0, 2), (3, 2), (4, 3), (5, 3), (6, 4)] {
            let sizes: Vec<(f64, f64)> = TURNS
                .iter()
                .map(|t| {
                    let s = render(&floor(&vec![Building; teams]), Turn::new(*t));
                    (s.width, s.height)
                })
                .collect();
            assert!(sizes.iter().all(|s| *s == sizes[0]), "{teams} teams");
            let (w, h) = View::in_room(Turn::default(), FLOOR_W, depth_for(rows * 2)).extent();
            assert_eq!(sizes[0], (w.round(), h.round()), "{teams} teams");
        }
    }

    #[test]
    fn names_and_versions_are_escaped() {
        let mut hq = floor(&[Releasing, Live]);
        hq.teams[0].release = Some(Launch {
            version: "<i>1</i>".to_string(),
            done: 1,
            total: 2,
            waiting: false,
            failed: false,
        });
        hq.teams[1].live = Some("<b>2</b>".to_string());
        hq.teams[1].project = "a \"b\" <c>".to_string();
        for t in TURNS {
            let svg = render(&hq, Turn::new(t)).svg;
            assert!(!svg.contains("<i>") && !svg.contains("<b>"), "turn {t}");
            assert!(svg.contains("&lt;i&gt;1&lt;/i&gt;"));
            assert!(svg.contains("&lt;b&gt;2&lt;/b&gt;"));
            assert!(!svg.contains(r#""a "b""#));
        }
    }

    #[test]
    fn review_and_checks_tickets_are_pinned() {
        for t in TURNS {
            let scene = render(&floor(&[Building]), Turn::new(t));
            let mut tickets: Vec<String> = scene
                .pins
                .iter()
                .filter_map(|p| match &p.what {
                    PinWhat::Ticket(w) => Some(w.ticket.clone()),
                    _ => None,
                })
                .collect();
            tickets.sort();
            assert_eq!(tickets, ["CHK-1", "CHK-2", "REX-1"], "turn {t}");
            assert_eq!(count(&scene.svg, r#"data-sel="tickets/REX-1""#), 1);
        }
    }

    #[test]
    fn only_a_paused_area_is_dimmed_by_its_phase() {
        let svg = render(&floor(&[Paused, Building]), Turn::default()).svg;
        assert_eq!(count(&svg, r#"data-phase="paused""#), 1);
        assert_eq!(count(&svg, r#"data-phase="building""#), 1);
    }

    /// A floor of `n` teams with people at desks, tasks in checks and in
    /// review, and a bell on every sign.
    fn crowd(n: usize, closed: bool) -> Hq {
        let names = [
            "shop",
            "editor",
            "admin",
            "docs",
            "mailer-service",
            "billing",
            "search",
            "gallery",
        ];
        let phases = [
            Building, Testing, Releasing, Live, Planning, Idle, Building, Paused,
        ];
        let mut hq = floor(&phases[..n]);
        for (i, t) in hq.teams.iter_mut().enumerate() {
            t.project = names[i].to_string();
            t.needs = 1;
            for w in &mut t.members {
                w.project = names[i].to_string();
            }
        }
        let first = names[0];
        hq.review = (0..3)
            .map(|i| {
                worker(
                    &format!("REX-{i}"),
                    first,
                    "rex",
                    Role::Reviewer,
                    Doing::Working,
                )
            })
            .collect();
        hq.checks = (0..3)
            .map(|i| {
                worker(
                    &format!("CHK-{i}"),
                    first,
                    "ci",
                    Role::Engineer,
                    Doing::Working,
                )
            })
            .collect();
        hq.closed = closed;
        hq
    }

    /// The width of the floor at each width the page is drawn at: the
    /// narrowest of each, and what the pages of 1440, 1280, 1024 and 390
    /// pixels give.
    const FLOOR_WIDTHS: [f64; 8] = [1028.0, 940.0, 868.0, 700.0, 656.0, 567.0, 378.0, 300.0];

    fn tier_of(width: f64) -> usize {
        pins::WIDTH
            .iter()
            .position(|min| width >= *min)
            .unwrap_or(pins::TIERS - 1)
    }

    #[test]
    fn no_label_covers_another_sign_a_room_or_a_person_in_any_turn() {
        for n in 1..=MAX_TEAMS {
            for t in TURNS {
                for closed in [false, true] {
                    let hq = crowd(n, closed);
                    let (scene, plan) = scene(&hq, Turn::new(t));
                    let what = format!("{n} projects, turn {t}, closed {closed}");
                    for width in FLOOR_WIDTHS {
                        let tier = tier_of(width);
                        let height = width * scene.height / scene.width;
                        let at = |p: &Pin| {
                            let s = p.spots.get(tier).copied();
                            let s =
                                s.unwrap_or_else(|| panic!("{what}: no place for {:?}", p.what));
                            (s.left / 100.0 * width, s.top / 100.0 * height, s)
                        };
                        let card = pins::CARD[tier][scene.forms[tier]];
                        let mut signs = Vec::new();
                        let mut tags = Vec::new();
                        let mut hints = Vec::new();
                        for p in &scene.pins {
                            match &p.what {
                                PinWhat::Sign(_) => {
                                    let (_, _, s) = at(p);
                                    let foot =
                                        pins::footprint(&pins::card_at(&s, (width, height), card));
                                    signs.push(foot);
                                }
                                PinWhat::Ticket(w) => {
                                    let asking = matches!(w.doing, Doing::Asking(_));
                                    if let Some(size) = pins::tag_size(tier, asking) {
                                        let (x, y, _) = at(p);
                                        tags.push(pins::stood_on((x, y), size));
                                    }
                                }
                                PinWhat::Hint(e) => {
                                    let (x, y, _) = at(p);
                                    hints.push(pins::stood_on(
                                        (x, y),
                                        pins::hint_size(tier, e.hint()),
                                    ));
                                }
                            }
                        }
                        let ctx = format!("{what}, floor {width}");
                        assert_eq!(signs.len(), n, "{ctx}");
                        let room = Rect::new(0.0, 0.0, width, height);
                        let k = width / plan.extent.0;
                        let keep: Vec<Rect> = plan
                            .keep
                            .iter()
                            .map(|r| Rect::new(r.x0 * k, r.y0 * k, r.x1 * k, r.y1 * k))
                            .collect();
                        let banner = pins::BANNER[tier].map(|(bw, bh)| {
                            Rect::new(
                                (width - bw) / 2.0,
                                height - pins::BANNER_FOOT - bh,
                                (width + bw) / 2.0,
                                height - pins::BANNER_FOOT,
                            )
                        });
                        for (i, s) in signs.iter().enumerate() {
                            assert!(
                                s.x0 >= 0.0 && s.y0 >= 0.0 && s.x1 <= room.x1 && s.y1 <= room.y1,
                                "{ctx}: sign {i} leaves the floor: {s:?}"
                            );
                            for (j, o) in signs.iter().enumerate().skip(i + 1) {
                                assert!(!s.hits(o, 0.0), "{ctx}: signs {i} and {j} overlap");
                            }
                            for (j, o) in tags.iter().enumerate() {
                                assert!(!s.hits(o, 0.0), "{ctx}: sign {i} covers task label {j}");
                            }
                            for (j, o) in hints.iter().enumerate() {
                                assert!(
                                    !s.hits(o, 0.0),
                                    "{ctx}: sign {i} and hint {j} overlap: {s:?} {o:?} room {room:?}"
                                );
                            }
                            for (j, o) in keep.iter().enumerate() {
                                assert!(
                                    !s.hits(o, 0.0),
                                    "{ctx}: sign {i} covers a room or person {j}"
                                );
                            }
                            assert!(
                                !closed || !banner.is_some_and(|b| s.hits(&b, 0.0)),
                                "{ctx}: sign {i} under the banner"
                            );
                        }
                        for (i, a) in tags.iter().enumerate() {
                            for (j, b) in tags.iter().enumerate().skip(i + 1) {
                                assert!(!a.hits(b, 0.0), "{ctx}: task labels {i} and {j} overlap");
                            }
                        }
                        // "What's what" shows every hint at once, except on a
                        // phone, where only the picked one shows. A hint may
                        // cover a task's label where there is no room, but
                        // never another hint.
                        if tier != pins::TIERS - 1 {
                            for (i, a) in hints.iter().enumerate() {
                                for (j, b) in hints.iter().enumerate().skip(i + 1) {
                                    assert!(!a.hits(b, 0.0), "{ctx}: hints {i} and {j} overlap");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
