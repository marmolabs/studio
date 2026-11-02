//! Where the labels over the company floor hang, so that no sign covers
//! another sign, a task's label, the owner's office, a room or a person.
//!
//! The floor is a picture that scales with the page, while a label is a fixed
//! size in pixels, so the room a label needs on the picture depends on how
//! wide the page is. The page changes what a sign shows at four widths
//! (`WIDTH`), and each width gets its own places, laid out for the narrowest
//! floor of that width: a wider floor only gives every label more room. Where
//! the floor is crowded, signs take a smaller form (`CARD`), as many steps
//! down as it takes.
//!
//! A sign hangs close to its team area, on the side where there is room, with
//! a thin line to the area when it can't hang right beside it.

use super::model::{Doing, Pin, PinWhat, Spot};

/// How many widths of the page there are.
pub const TIERS: usize = 4;

/// The narrowest floor, in pixels, of each width of the page: the widths the
/// style sheet changes a sign at.
pub const WIDTH: [f64; TIERS] = [940.0, 700.0, 567.0, 300.0];

/// How many forms a sign has, from its fullest to its smallest: full,
/// compact, a name with its phase, and a round badge.
pub const FORMS: usize = 4;

/// A sign's card in pixels, by width of the page and form, as drawn by
/// `app.css`. A width never shows a fuller form than its own.
pub const CARD: [[(f64, f64); FORMS]; TIERS] = [
    [(224.0, 81.0), (204.0, 57.0), (120.0, 34.0), (30.0, 30.0)],
    [(204.0, 57.0), (204.0, 57.0), (120.0, 34.0), (30.0, 30.0)],
    [(120.0, 34.0), (120.0, 34.0), (120.0, 34.0), (30.0, 30.0)],
    [(30.0, 30.0), (30.0, 30.0), (30.0, 30.0), (30.0, 30.0)],
];

/// How far a bell sticks out over the top and right of a sign's card.
pub const BELL: f64 = 11.0;

/// A task's label folded up, in pixels, and the small one a task that waits
/// for the owner keeps on a phone, where the rest are left out.
pub const TAG: (f64, f64) = (105.0, 26.0);
const TAG_SMALL: (f64, f64) = (36.0, 26.0);

/// What a task's label takes up at `tier`, or `None` where it isn't shown.
pub fn tag_size(tier: usize, asking: bool) -> Option<(f64, f64)> {
    match (tier == TIERS - 1, asking) {
        (false, _) => Some(TAG),
        (true, true) => Some(TAG_SMALL),
        (true, false) => None,
    }
}

/// The "Office closed" banner at the foot of the floor, in pixels, by width
/// of the page, and how far it is from the foot. On a phone it hangs under
/// the floor instead.
pub const BANNER: [Option<(f64, f64)>; TIERS] = [
    Some((374.0, 36.0)),
    Some((374.0, 36.0)),
    Some((374.0, 36.0)),
    None,
];
pub const BANNER_FOOT: f64 = 10.0;

/// How a hint is measured: its words and a little padding on one line, 22
/// pixels high, and on a phone a block of two lines, so that it fits on the
/// floor.
pub fn hint_size(tier: usize, words: &str) -> (f64, f64) {
    if tier == TIERS - 1 {
        return (110.0, 38.0);
    }
    (14.0 + 6.0 * words.chars().count() as f64, 22.0)
}

/// What a smaller form of sign costs for each sign, in pixels of line.
const SMALLER: f64 = 30.0;

/// What a label keeps clear of at least.
const GAP: f64 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
        Rect { x0, y0, x1, y1 }
    }

    /// Whether the two share any room, with `gap` to spare.
    pub fn hits(&self, o: &Rect, gap: f64) -> bool {
        self.x0 < o.x1 + gap && o.x0 < self.x1 + gap && self.y0 < o.y1 + gap && o.y0 < self.y1 + gap
    }

    fn area(&self) -> f64 {
        (self.x1 - self.x0).max(0.0) * (self.y1 - self.y0).max(0.0)
    }

    fn overlap(&self, o: &Rect) -> f64 {
        Rect::new(
            self.x0.max(o.x0),
            self.y0.max(o.y0),
            self.x1.min(o.x1),
            self.y1.min(o.y1),
        )
        .area()
    }

    fn scaled(&self, s: f64) -> Rect {
        Rect::new(self.x0 * s, self.y0 * s, self.x1 * s, self.y1 * s)
    }

    /// The same rectangle with its middle kept and its sides cut to `share`.
    fn core(&self, share: f64) -> Rect {
        let (cx, cy) = ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0);
        let (hw, hh) = (
            (self.x1 - self.x0) / 2.0 * share,
            (self.y1 - self.y0) / 2.0 * share,
        );
        Rect::new(cx - hw, cy - hh, cx + hw, cy + hh)
    }
}

/// A sign's card at `spot` on a floor `width` pixels wide and `height` high:
/// the card alone, without its bell.
pub fn card_at(spot: &Spot, (width, height): (f64, f64), (w, h): (f64, f64)) -> Rect {
    let (x, y) = (spot.left / 100.0 * width, spot.top / 100.0 * height);
    let (x0, y0) = (x - spot.across * w, y - spot.down * h);
    Rect::new(x0, y0, x0 + w, y0 + h)
}

/// What a sign takes up with its bell.
pub fn footprint(card: &Rect) -> Rect {
    Rect::new(card.x0, card.y0 - BELL, card.x1 + BELL, card.y1)
}

/// A label that hangs from a point: it stands on it, in the middle.
pub fn stood_on(spot: (f64, f64), (w, h): (f64, f64)) -> Rect {
    Rect::new(spot.0 - w / 2.0, spot.1 - h, spot.0 + w / 2.0, spot.1)
}

/// A solid seen from the side as `slices` upright rectangles side by side,
/// which together cover everything the corners `points` span, and little
/// more: a box seen from a corner is a hexagon, and a bounding box would
/// take its empty corners too.
pub fn silhouette(points: &[(f64, f64)], slices: usize) -> Vec<Rect> {
    let left = points.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    let right = points.iter().map(|p| p.0).fold(f64::MIN, f64::max);
    let step = (right - left) / slices as f64;
    let mut out = Vec::new();
    for k in 0..slices {
        let (xa, xb) = (left + step * k as f64, left + step * (k + 1) as f64);
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for p in points.iter().filter(|p| p.0 >= xa && p.0 <= xb) {
            lo = lo.min(p.1);
            hi = hi.max(p.1);
        }
        // The edges between corners, where they cross the slice's sides.
        for (i, p) in points.iter().enumerate() {
            for q in &points[i + 1..] {
                for edge in [xa, xb] {
                    if (p.0 - edge) * (q.0 - edge) <= 0.0 && p.0 != q.0 {
                        let y = p.1 + (edge - p.0) / (q.0 - p.0) * (q.1 - p.1);
                        lo = lo.min(y);
                        hi = hi.max(y);
                    }
                }
            }
        }
        if lo <= hi {
            out.push(Rect::new(xa, lo, xb, hi));
        }
    }
    out
}

/// What is on the floor, in the picture's own units.
pub struct Plan {
    /// The picture's size.
    pub extent: (f64, f64),
    /// What labels never cover: the owner's office and the people.
    pub keep: Vec<Rect>,
    /// What labels cover only where nothing else is left: the shared rooms.
    pub rooms: Vec<Rect>,
    /// The corners of each team area, then of the one for a new project.
    pub pods: Vec<[(f64, f64); 4]>,
    /// The office is closed and says so over the floor.
    pub closed: bool,
}

/// A line from a sign to its team area, in the picture's own units, for one
/// width of the page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leader {
    pub tier: usize,
    pub from: (f64, f64),
    pub to: (f64, f64),
    pub team: usize,
}

/// The two ends of a line, in the picture's own units.
type Line = ((f64, f64), (f64, f64));

/// Where each sign hangs, with its line to its area when it needs one.
type Hung = Vec<(Spot, Option<Line>)>;

/// What `lay_out` decided.
pub struct Laid {
    pub pins: Vec<Pin>,
    pub leaders: Vec<Leader>,
    /// The form of the signs at each width of the page.
    pub forms: [usize; TIERS],
}

/// Gives every label its places. `pins` are the task labels and hints in
/// the order they were put; `signs` are the pins of the signs, one for each
/// team area, in the order of `plan.pods`.
pub fn lay_out(plan: &Plan, mut pins: Vec<Pin>, mut signs: Vec<Pin>) -> Laid {
    let mut leaders = Vec::new();
    let mut forms = [0; TIERS];
    for (tier, slot) in forms.iter_mut().enumerate() {
        let Solved { hung, form, spots } = solve(plan, tier, &pins, signs.len());
        *slot = form;
        for (i, spot) in spots {
            pins[i].spots.push(spot);
        }
        for (k, (spot, leader)) in hung.into_iter().enumerate() {
            signs[k].spots.push(spot);
            leaders.extend(leader.map(|(from, to)| Leader {
                tier,
                from,
                to,
                team: k,
            }));
        }
    }
    pins.append(&mut signs);
    Laid {
        pins,
        leaders,
        forms,
    }
}

/// What was decided for one width of the page.
struct Solved {
    /// Where every sign hangs.
    hung: Hung,
    /// The smallest form of sign it took.
    form: usize,
    /// The places of the task labels and hints, by their position in `pins`.
    spots: Vec<(usize, Spot)>,
}

/// One width of the page. Task labels keep their place unless another is in
/// the way; signs hang around the rooms, the people and the labels; and a hint
/// that would cover a sign moves off it.
fn solve(plan: &Plan, tier: usize, pins: &[Pin], signs: usize) -> Solved {
    let width = WIDTH[tier];
    let scale = width / plan.extent.0;
    let height = plan.extent.1 * scale;
    let room = Rect::new(0.0, 0.0, width, height);
    let spot_of = |x: f64, y: f64, across: f64, down: f64| Spot {
        left: (x / width * 10000.0).round() / 100.0,
        top: (y / height * 10000.0).round() / 100.0,
        across,
        down,
    };
    let inside = |r: &Rect| r.x0 >= 0.0 && r.x1 <= width && r.y0 >= 0.0 && r.y1 <= height;

    let mut base: Vec<Rect> = plan.keep.iter().map(|r| r.scaled(scale)).collect();
    let people = base.clone();
    if let (true, Some((w, h))) = (plan.closed, BANNER[tier]) {
        base.push(Rect::new(
            (width - w) / 2.0,
            height - BANNER_FOOT - h,
            (width + w) / 2.0,
            height - BANNER_FOOT,
        ));
    }

    // The task labels: where they hang if nothing is in the way, else the
    // nearest free place. They are placed first without the signs, which then
    // keep clear of them where there is room, and once more round the signs.
    let hints: Vec<(usize, (f64, f64), Rect)> = pins
        .iter()
        .enumerate()
        .filter_map(|(i, pin)| match &pin.what {
            PinWhat::Hint(e) => {
                let at = (pin.left / 100.0 * width, pin.top / 100.0 * height);
                Some((i, at, stood_on(at, hint_size(tier, e.hint()))))
            }
            _ => None,
        })
        .collect();
    let settle = |avoid: &[Rect]| {
        let mut spots: Vec<(usize, Spot)> = Vec::new();
        let mut tags: Vec<Rect> = Vec::new();
        for (i, pin) in pins.iter().enumerate() {
            let PinWhat::Ticket(w) = &pin.what else {
                continue;
            };
            let at = (pin.left / 100.0 * width, pin.top / 100.0 * height);
            let asking = matches!(w.doing, Doing::Asking(_));
            let Some(size) = tag_size(tier, asking) else {
                spots.push((i, spot_of(at.0, at.1, 0.5, 1.0)));
                continue;
            };
            let mut best: Option<(f64, (f64, f64), Rect)> = None;
            for (dx, dy) in nudges() {
                let to = (at.0 + dx, at.1 + dy);
                let r = stood_on(to, size);
                if !inside(&r) || tags.iter().chain(avoid).any(|p| p.hits(&r, GAP)) {
                    continue;
                }
                let on_people = people.iter().filter(|p| p.hits(&r, 0.0)).count() as f64;
                let cost = dx.hypot(dy) + 40.0 * on_people;
                if best.as_ref().is_none_or(|b| cost < b.0) {
                    best = Some((cost, to, r));
                }
            }
            let (to, r) = best.map_or((at, stood_on(at, size)), |b| (b.1, b.2));
            tags.push(r);
            spots.push((i, spot_of(to.0, to.1, 0.5, 1.0)));
        }
        (spots, tags)
    };
    let (_, tags) = settle(&[]);

    // Signs keep clear of the hints too when there is room; with none, the
    // hints move.
    let hint_rects: Vec<Rect> = hints.iter().map(|h| h.2).collect();
    // Hints are left out on a phone, where only the one picked shows.
    let hint_rects: Vec<Rect> = if tier == TIERS - 1 {
        Vec::new()
    } else {
        hint_rects
    };
    let with_hints = [base.as_slice(), &tags, &hint_rects].concat();
    let without_hints = [base.as_slice(), &tags].concat();
    let place = |form: usize, fixed: &[Rect]| {
        hang(plan, tier, form, signs, fixed, scale, (&room, &spot_of))
    };
    let mut chosen: Option<(usize, Hung)> = None;
    for fixed in [&with_hints, &without_hints, &base] {
        // The form that costs least, a smaller one costing a little for each
        // sign.
        let mut best: Option<(f64, usize, Hung)> = None;
        for form in 0..FORMS {
            if let Some((cost, found)) = place(form, fixed) {
                let cost = cost + SMALLER * (form * signs) as f64;
                if best.as_ref().is_none_or(|b| cost < b.0) {
                    best = Some((cost, form, found));
                }
            }
        }
        if let Some((_, form, found)) = best {
            chosen = Some((form, found));
            break;
        }
    }
    // Too crowded even then: a sign goes over the top of its area.
    let (form, hung) = chosen.unwrap_or_else(|| {
        let over = plan
            .pods
            .iter()
            .take(signs)
            .map(|p| {
                let b = bounds(p);
                let at = ((b.x0 + b.x1) / 2.0 * scale, b.y0 * scale);
                (spot_of(at.0, at.1, 0.5, 1.0), None)
            })
            .collect();
        (FORMS - 1, over)
    });

    let card = CARD[tier][form];
    let feet: Vec<Rect> = hung
        .iter()
        .map(|(s, _)| footprint(&card_at(s, (width, height), card)))
        .collect();
    let (mut spots, shown) = settle(&feet);
    // What a hint never covers: the signs, and the hints put before it.
    let mut never = feet.clone();
    for (i, at, rect) in hints {
        // A hint stays where its subject is, unless it would cover a sign, a
        // hint or a task's label: then it moves to the nearest free place
        // near by. Where there is none, it may cover a task's label, which
        // it is drawn over.
        let size = hint_size(tier, hint_words(&pins[i]));
        let free = |r: &Rect, avoid: &[Rect]| inside(r) && !avoid.iter().any(|f| f.hits(r, GAP));
        let nearest = |avoid: &[Rect], within: f64| {
            let mut best: Option<(f64, (f64, f64))> = None;
            let mut y = 0.0;
            while y <= height {
                let mut x = 0.0;
                while x <= width {
                    let d = (x - at.0).hypot(y - at.1);
                    if d <= within
                        && best.as_ref().is_none_or(|b| d < b.0)
                        && free(&stood_on((x, y), size), avoid)
                    {
                        best = Some((d, (x, y)));
                    }
                    x += 10.0;
                }
                y += 10.0;
            }
            best.map(|b| b.1)
        };
        let clear = [never.as_slice(), &shown].concat();
        let spot = if free(&rect, &clear) {
            at
        } else if let Some(near) = nearest(&clear, 70.0) {
            near
        } else if free(&rect, &never) {
            at
        } else {
            nearest(&never, f64::MAX).unwrap_or(at)
        };
        // On a phone only the picked hint shows, so they never meet there.
        if tier != TIERS - 1 {
            never.push(stood_on(spot, size));
        }
        spots.push((i, spot_of(spot.0, spot.1, 0.5, 1.0)));
    }
    Solved { hung, form, spots }
}

/// What a hint says.
fn hint_words(pin: &Pin) -> &'static str {
    match &pin.what {
        PinWhat::Hint(e) => e.hint(),
        _ => "",
    }
}

/// Nudges of a label, in pixels, nearest first.
fn nudges() -> Vec<(f64, f64)> {
    let mut all = Vec::new();
    for i in -12..=12 {
        for j in -9..=9 {
            all.push((f64::from(i) * 23.0, f64::from(j) * 14.0));
        }
    }
    all.sort_by(|a, b| a.0.hypot(a.1).total_cmp(&b.0.hypot(b.1)));
    all
}

/// A place for a sign: where its card sits, which point of it, its cost and
/// its line to the area.
struct Cand {
    spot: Spot,
    foot: Rect,
    cost: f64,
    line: Option<Line>,
}

type SpotOf<'a> = (&'a Rect, &'a dyn Fn(f64, f64, f64, f64) -> Spot);

/// Hangs every sign at `form`, or `None` when they don't all fit.
fn hang(
    plan: &Plan,
    tier: usize,
    form: usize,
    signs: usize,
    fixed: &[Rect],
    scale: f64,
    (room, spot_of): SpotOf<'_>,
) -> Option<(f64, Hung)> {
    let card = CARD[tier][form];
    let pods: Vec<[(f64, f64); 4]> = plan
        .pods
        .iter()
        .map(|p| p.map(|(x, y)| (x * scale, y * scale)))
        .collect();
    let cores: Vec<Rect> = pods
        .iter()
        .map(|p| bounds(p).core(0.55))
        .chain(plan.rooms.iter().map(|r| r.scaled(scale)))
        .collect();

    let mut options: Vec<Vec<Cand>> = Vec::new();
    for k in 0..signs {
        let poly = pods.get(k)?;
        let mut list: Vec<Cand> = candidates(poly, room)
            .into_iter()
            .filter_map(|(anchor, across, down)| {
                let x0 = anchor.0 - across * card.0;
                let y0 = anchor.1 - down * card.1;
                let c = Rect::new(x0, y0, x0 + card.0, y0 + card.1);
                let foot = Rect::new(c.x0, c.y0 - BELL, c.x1 + BELL, c.y1);
                if foot.x0 < 0.0 || foot.y0 < 0.0 || foot.x1 > room.x1 || foot.y1 > room.y1 {
                    return None;
                }
                if fixed.iter().any(|f| f.hits(&foot, GAP)) {
                    return None;
                }
                let (near, away) = nearest(poly, anchor);
                let mut soft = 0.0;
                for (j, core) in cores.iter().enumerate() {
                    let share = c.overlap(core) / c.area();
                    soft += share * if j == k { 2.0 } else { 1.0 };
                }
                let cost = away + 320.0 * soft;
                let line = (away > 14.0).then_some((anchor, near));
                Some(Cand {
                    spot: spot_of(anchor.0, anchor.1, across, down),
                    foot,
                    cost,
                    line,
                })
            })
            .collect();
        list.sort_by(|a, b| a.cost.total_cmp(&b.cost));
        if list.is_empty() {
            return None;
        }
        options.push(list);
    }

    // Fewest places first, then each sign takes its cheapest place that
    // doesn't touch the signs before it.
    let mut order: Vec<usize> = (0..signs).collect();
    order.sort_by_key(|k| options[*k].len());
    let mut chosen: Vec<usize> = vec![0; signs];
    let mut nodes = 0;
    if !search(&order, 0, &options, &mut chosen, &mut nodes) {
        return None;
    }
    // Then each sign in turn moves to a cheaper place where one is free.
    for _ in 0..4 {
        for k in 0..signs {
            let now = options[k][chosen[k]].cost;
            let free = options[k].iter().position(|c| {
                c.cost < now
                    && (0..signs).all(|o| o == k || !options[o][chosen[o]].foot.hits(&c.foot, GAP))
            });
            if let Some(i) = free {
                chosen[k] = i;
            }
        }
    }
    let cost = (0..signs).map(|k| options[k][chosen[k]].cost).sum();
    let hung = (0..signs)
        .map(|k| {
            let c = &options[k][chosen[k]];
            let line = c.line.map(|(from, to)| {
                (
                    (from.0 / scale, from.1 / scale),
                    (to.0 / scale, to.1 / scale),
                )
            });
            (c.spot, line)
        })
        .collect();
    Some((cost, hung))
}

/// Depth first, cheapest place first; gives up after a few thousand tries.
fn search(
    order: &[usize],
    at: usize,
    options: &[Vec<Cand>],
    chosen: &mut [usize],
    nodes: &mut usize,
) -> bool {
    let Some(&k) = order.get(at) else {
        return true;
    };
    for (i, c) in options[k].iter().enumerate() {
        *nodes += 1;
        if *nodes > 40_000 {
            return false;
        }
        let clash = order[..at]
            .iter()
            .any(|&o| options[o][chosen[o]].foot.hits(&c.foot, GAP));
        if clash {
            continue;
        }
        chosen[k] = i;
        if search(order, at + 1, options, chosen, nodes) {
            return true;
        }
    }
    false
}

fn bounds(poly: &[(f64, f64); 4]) -> Rect {
    let mut r = Rect::new(f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in poly {
        r.x0 = r.x0.min(*x);
        r.y0 = r.y0.min(*y);
        r.x1 = r.x1.max(*x);
        r.y1 = r.y1.max(*y);
    }
    r
}

/// The places to hang a card from: points round the area, further and further
/// out, with the side of the card that faces it on the point, then every
/// spot of the floor on a grid, for the card's middle.
fn candidates(poly: &[(f64, f64); 4], room: &Rect) -> Vec<((f64, f64), f64, f64)> {
    let b = bounds(poly);
    let (cx, cy) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
    let (hw, hh) = ((b.x1 - b.x0) / 2.0, (b.y1 - b.y0) / 2.0);
    let mut out = Vec::new();
    for step in 0..24 {
        let angle = f64::from(step) * std::f64::consts::TAU / 24.0;
        let (c, s) = (angle.cos(), angle.sin());
        let across = if c > 0.4 {
            0.0
        } else if c < -0.4 {
            1.0
        } else {
            0.5
        };
        let down = if s > 0.4 {
            0.0
        } else if s < -0.4 {
            1.0
        } else {
            0.5
        };
        for extra in [0.0, 12.0, 30.0, 60.0, 100.0, 150.0, 220.0, 300.0] {
            // The edge of the area in this direction, then out.
            let reach = 1.0 / (c.abs() / hw.max(1.0) + s.abs() / hh.max(1.0));
            let r = reach + 4.0 + extra;
            out.push(((cx + c * r, cy + s * r), across, down));
        }
    }
    let mut y = 8.0;
    while y < room.y1 {
        let mut x = 8.0;
        while x < room.x1 {
            out.push(((x, y), 0.5, 0.5));
            x += 16.0;
        }
        y += 16.0;
    }
    out
}

/// The point of the area's outline nearest `p`, and how far it is.
fn nearest(poly: &[(f64, f64); 4], p: (f64, f64)) -> ((f64, f64), f64) {
    let mut best = (poly[0], f64::MAX);
    for i in 0..4 {
        let (a, b) = (poly[i], poly[(i + 1) % 4]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        let q = (a.0 + t * dx, a.1 + t * dy);
        let d = (p.0 - q.0).hypot(p.1 - q.1);
        if d < best.1 {
            best = (q, d);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_overlap_only_where_they_share_room() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(a.hits(&Rect::new(9.0, 9.0, 20.0, 20.0), 0.0));
        // Touching is not overlapping, but a gap asks for room between.
        assert!(!a.hits(&Rect::new(10.0, 0.0, 20.0, 10.0), 0.0));
        assert!(a.hits(&Rect::new(12.0, 0.0, 20.0, 10.0), 3.0));
        assert!(!a.hits(&Rect::new(0.0, 11.0, 10.0, 20.0), 0.0));
    }

    #[test]
    fn a_box_seen_from_a_corner_is_a_hexagon_not_its_bounding_box() {
        // A hexagon: 20 wide, 20 high, its top and bottom corners cut.
        let hexagon = [
            (10.0, 0.0),
            (20.0, 5.0),
            (20.0, 15.0),
            (10.0, 20.0),
            (0.0, 15.0),
            (0.0, 5.0),
        ];
        let slices = silhouette(&hexagon, 4);
        assert_eq!(slices.len(), 4);
        // The outer slices are shorter than the middle ones.
        assert!(slices[0].y1 - slices[0].y0 < slices[1].y1 - slices[1].y0);
        // Together they cover the whole hexagon.
        for (x, y) in [(10.0, 0.5), (19.5, 6.0), (0.5, 14.0), (10.0, 19.5)] {
            assert!(
                slices
                    .iter()
                    .any(|s| x >= s.x0 && x <= s.x1 && y >= s.y0 && y <= s.y1),
                "{x},{y}"
            );
        }
        // And leave its empty corner free.
        assert!(!slices.iter().any(|s| s.x0 <= 1.0 && s.y0 <= 0.5));
    }

    #[test]
    fn a_hint_is_one_line_and_on_a_phone_two() {
        let (w, h) = hint_size(0, "Add a project");
        assert!((70.0..110.0).contains(&w) && h == 22.0);
        assert_eq!(hint_size(TIERS - 1, "Add a project"), (110.0, 38.0));
    }

    #[test]
    fn a_card_hangs_from_its_spot_by_the_point_the_spot_names() {
        let spot = Spot {
            left: 50.0,
            top: 50.0,
            across: 1.0,
            down: 0.5,
        };
        // On a floor 1000 by 400, the card's right edge and middle are on
        // (500, 200).
        let card = card_at(&spot, (1000.0, 400.0), (200.0, 60.0));
        assert_eq!(card, Rect::new(300.0, 170.0, 500.0, 230.0));
        // Its bell sticks out over the top and right.
        let foot = footprint(&card);
        assert_eq!((foot.x1, foot.y0), (500.0 + BELL, 170.0 - BELL));
    }
}
