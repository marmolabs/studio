//! What the company floor shows, as plain data: `plan` builds an `Hq` from
//! studiod's state and `hq_scene` draws it.

/// Which corner the office is seen from. 0 is the first view; each step
/// moves the camera a quarter turn around the room, clockwise seen from
/// above, so the room turns the other way on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Turn(u8);

impl Turn {
    /// Any whole number of quarter turns, wrapped to 0..=3.
    pub fn new(n: i64) -> Turn {
        Turn(u8::try_from(n.rem_euclid(4)).unwrap_or(0))
    }

    pub fn index(self) -> u8 {
        self.0
    }
}

/// Project colours there are (`pj-0` to `pj-7`); a ninth project shares the
/// first one's.
pub const TINTS: usize = 8;

/// Someone at work on a ticket.
#[derive(Clone, Debug, PartialEq)]
pub struct Worker {
    pub ticket: String,
    pub project: String,
    pub title: String,
    pub hire: String,
    /// What the hire is called: the name in its file, else its id.
    pub name: String,
    pub role: Role,
    pub doing: Doing,
    /// How far through work, checks, review and landing, from 0 to 1.
    pub progress: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Doing {
    Working,
    /// Waiting for the owner, with what it wants in a few words ("Run a
    /// command").
    Asking(String),
    /// On hold, with why in a few words ("budget").
    Held(String),
}

/// A hire who isn't at a desk: a lead, the owner or someone resting.
#[derive(Clone, Debug, PartialEq)]
pub struct Person {
    pub hire: String,
    /// What the hire is called: the name in its file, else its id.
    pub name: String,
    pub role: Role,
    /// Minutes since it last worked, for the lounge.
    pub idle_minutes: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Engineer,
    Writer,
    Designer,
    Producer,
    Reviewer,
    Critic,
    Approver,
    Router,
    /// Leads a project: plans its work, hands it out and checks it.
    Lead,
    /// The owner, at the top of the company.
    Owner,
    Other,
}

impl Role {
    /// From a hire's role, which is free text in its file.
    pub fn of(role: &str) -> Role {
        match role.trim().to_ascii_lowercase().as_str() {
            "engineer" | "developer" | "dev" => Role::Engineer,
            "writer" | "copy" | "marketing" => Role::Writer,
            "designer" | "artist" => Role::Designer,
            "producer" => Role::Producer,
            "reviewer" => Role::Reviewer,
            "critic" => Role::Critic,
            "approver" => Role::Approver,
            "router" => Role::Router,
            "lead" | "manager" => Role::Lead,
            _ => Role::Other,
        }
    }

    /// The CSS class that gives a figure its shirt colour.
    pub fn class(self) -> &'static str {
        match self {
            Role::Engineer => "r-eng",
            Role::Writer => "r-wri",
            Role::Designer => "r-des",
            Role::Producer => "r-pro",
            Role::Reviewer => "r-rev",
            Role::Critic => "r-cri",
            Role::Approver => "r-app",
            Role::Router => "r-rou",
            Role::Lead => "r-lead",
            Role::Owner => "r-own",
            Role::Other => "r-oth",
        }
    }
}

/// What the company floor draws as a control, each for a part of the studio
/// and selected as `elements/<key>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Element {
    /// The owner's office, with what waits on the owner in its tray.
    Owner,
    /// Who is free.
    Lounge,
    /// A free seat in the lounge: hire someone.
    Hire,
    /// Reviewers and critics.
    Review,
    /// Every project's checks.
    Racks,
    /// An empty rack slot in the checks lab: add a project, which gets a
    /// rack of its own.
    AddProject,
}

impl Element {
    /// Every element, in the order the company floor shows them: the owner's
    /// office, the lounge and its free seat, the shared rooms and the spot
    /// for a new project.
    pub const ALL: [Element; 6] = [
        Element::Owner,
        Element::Lounge,
        Element::Hire,
        Element::Review,
        Element::Racks,
        Element::AddProject,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Element::Owner => "owner",
            Element::Lounge => "lounge",
            Element::Hire => "hire",
            Element::Review => "review",
            Element::Racks => "racks",
            Element::AddProject => "add-project",
        }
    }

    pub fn from_key(key: &str) -> Option<Element> {
        Element::ALL.into_iter().find(|e| e.key() == key)
    }

    /// What it is, on hover or with "What's what".
    pub fn hint(self) -> &'static str {
        match self {
            Element::Owner => "Your office · what needs you",
            Element::Lounge => "Lounge · who's free",
            Element::Hire => "Hire someone",
            Element::Review => "Review · reviewers and critics",
            Element::Racks => "Checks · every project",
            Element::AddProject => "Add a project",
        }
    }
}

/// A label pinned over the floor, placed in percent of the scene's box.
#[derive(Clone, Debug, PartialEq)]
pub struct Pin {
    pub left: f64,
    pub top: f64,
    pub what: PinWhat,
    /// Where it hangs at each width of the page, widest first, when that
    /// differs from `left` and `top`.
    pub spots: Vec<Spot>,
}

/// Where a label hangs: a point in percent of the scene's box and which
/// point of the label sits on it, as shares of the label's own width and
/// height (0 is its left or top, 1 its right or bottom).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    pub left: f64,
    pub top: f64,
    pub across: f64,
    pub down: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PinWhat {
    /// A task at a desk, in checks or in review.
    Ticket(Worker),
    /// What a game element is, shown on hover or with "What's what".
    Hint(Element),
    /// A project's sign by its team area on the company floor.
    Sign(Box<Team>),
}

/// The company floor: one team area per project, the owner's office, the
/// lounge, and the rooms every project shares.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hq {
    /// One per project, in the order of `studio.toml`.
    pub teams: Vec<Team>,
    /// What waits on the owner across every project.
    pub owner_needs: usize,
    /// Hires with nothing to do.
    pub lounge: Vec<Person>,
    /// Tickets a reviewer or critic is reading, with who reads them.
    pub review: Vec<Worker>,
    /// Tickets running their checks.
    pub checks: Vec<Worker>,
    /// The owner closed the office: nothing new starts.
    pub closed: bool,
}

/// A project's team area.
#[derive(Clone, Debug, PartialEq)]
pub struct Team {
    pub project: String,
    /// Its colour index (`pj-N`).
    pub tint: usize,
    pub phase: Phase,
    /// Things in this project waiting on the owner.
    pub needs: usize,
    /// Who leads it, at the head of the area.
    pub lead: Option<Person>,
    /// Who is working on it now, at its desks.
    pub members: Vec<Worker>,
    /// The goal it is working towards.
    pub milestone: Option<Milestone>,
    /// The release under way.
    pub release: Option<Launch>,
    /// The version last released.
    pub live: Option<String>,
}

/// Where a project stands, in the order a project moves through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Nothing open and never released.
    Idle,
    Planning,
    Building,
    Testing,
    Releasing,
    /// Released, with nothing open.
    Live,
    /// The office is closed.
    Paused,
}

/// The goal a project is working towards.
#[derive(Clone, Debug, PartialEq)]
pub struct Milestone {
    /// Its selection key, `goals/<plan id>`.
    pub key: String,
    pub title: String,
    pub version: Option<String>,
    pub done: usize,
    pub total: usize,
}

/// A release under way.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub version: String,
    pub done: usize,
    pub total: usize,
    /// It waits for the owner's go-ahead.
    pub waiting: bool,
    pub failed: bool,
}

/// The floor drawn: an SVG for the page, its size in SVG units, and the
/// labels to pin over it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub width: f64,
    pub height: f64,
    pub svg: String,
    pub pins: Vec<Pin>,
    /// How full the signs are at each width of the page, from 0 (all it
    /// can show) down: the floor is crowded when a larger form won't fit.
    pub forms: [usize; super::pins::TIERS],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lead_or_a_manager_is_a_lead() {
        for role in ["lead", "Lead", " manager ", "MANAGER"] {
            assert_eq!(Role::of(role), Role::Lead, "{role}");
        }
        assert_eq!(Role::Lead.class(), "r-lead");
        assert_eq!(Role::Owner.class(), "r-own");
        // The owner is nobody's hire file.
        assert_eq!(Role::of("owner"), Role::Other);
        assert_eq!(Role::of("engineer"), Role::Engineer);
    }

    #[test]
    fn every_element_can_be_picked_by_its_key() {
        assert_eq!(Element::from_key("owner"), Some(Element::Owner));
        assert_eq!(Element::Owner.hint(), "Your office · what needs you");
        for e in Element::ALL {
            assert_eq!(Element::from_key(e.key()), Some(e));
        }
        // What the old floor drew is no longer a control.
        for gone in [
            "board",
            "corkboard",
            "posters",
            "coffee",
            "trophies",
            "crates",
        ] {
            assert_eq!(Element::from_key(gone), None, "{gone}");
        }
        assert_eq!(Element::from_key("nope"), None);
    }
}
