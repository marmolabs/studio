//! What the isometric office floor shows, as plain data: `plan` builds a
//! `Floor` from studiod's state and `scene` draws it.

/// Everything on the floor at one moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Floor {
    /// One desk per run slot, in order. A desk with no worker is free.
    pub desks: Vec<Desk>,
    /// Whether the office has room for another desk, shown as a "+" spot.
    pub room_for_desk: bool,
    /// Tickets running their checks, in the checks lab.
    pub checks: Vec<Worker>,
    /// Tickets being reviewed or judged by a critic, in the review room.
    pub review: Vec<Worker>,
    /// Ready to land, on the pallet at the dock.
    pub ready: Vec<Crate>,
    /// Landed in the last 24 hours, newest first.
    pub shipped: Vec<Crate>,
    /// Failed in the last 24 hours, newest first.
    pub returns: Vec<Crate>,
    /// Open goals, as notes on the whiteboard.
    pub goals: Vec<Note>,
    /// Hires planning a goal, at the whiteboard.
    pub planners: Vec<Person>,
    /// Hires with nothing to do, in the lounge.
    pub lounge: Vec<Person>,
    /// The approver and the router, at the front desk.
    pub front: Vec<Person>,
    /// Tickets waiting for a free desk.
    pub queued: usize,
    /// The owner closed the office: nothing new starts.
    pub closed: bool,
    /// Every project in a stable order; a project's colour is its index.
    pub projects: Vec<String>,
    /// Trophies earned: one cup each on the shelf.
    pub trophies: usize,
    /// House rule files: one poster each.
    pub rules: usize,
    /// Hires other than the approver and the router: one card each on the
    /// team board.
    pub hires: usize,
}

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

impl Floor {
    /// The colour index of `project`: its place in `projects` (or 0), wrapped
    /// to the colours there are.
    pub fn tint(&self, project: &str) -> usize {
        super::facts::tint(&self.projects, project)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Desk {
    pub worker: Option<Worker>,
}

/// Someone at work on a ticket.
#[derive(Clone, Debug, PartialEq)]
pub struct Worker {
    pub ticket: String,
    pub project: String,
    pub title: String,
    pub hire: String,
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

/// A ticket at the dock: ready, shipped or returned.
#[derive(Clone, Debug, PartialEq)]
pub struct Crate {
    pub ticket: String,
    pub project: String,
    pub title: String,
}

/// An open goal on the whiteboard.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// The goal's selection key, as the inspector takes it (`goals/<id>`).
    pub key: String,
    pub project: String,
    pub title: String,
    pub landed: usize,
    pub total: usize,
    /// Every ticket has landed: the goal waits to be merged.
    pub done: bool,
}

/// A hire who isn't at a desk: planning, resting or at the front desk.
#[derive(Clone, Debug, PartialEq)]
pub struct Person {
    pub hire: String,
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
            Role::Other => "r-oth",
        }
    }
}

/// The office's game elements: each is a control for part of the studio,
/// selected as `elements/<key>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Element {
    /// Goals in every project.
    Board,
    /// The team and hiring.
    Corkboard,
    /// House rules.
    Posters,
    /// Every project's checks.
    Racks,
    /// One more desk: one more agent at once.
    AddDesk,
    /// Who's free.
    Lounge,
    /// Close or reopen the office.
    Coffee,
    /// Achievements from the studio's history.
    Trophies,
    /// Reviewers and critics.
    Review,
    /// What shipped today.
    Crates,
    /// Failed work.
    Returns,
    /// The approver and the router.
    Counter,
    /// What's up next.
    Queue,
    /// The budget.
    Safe,
    /// An empty rack slot in the checks lab: add a project, which gets a
    /// rack of its own.
    AddProject,
    /// A free seat in the lounge: hire someone.
    Hire,
}

impl Element {
    pub const ALL: [Element; 16] = [
        Element::Board,
        Element::Corkboard,
        Element::Posters,
        Element::Racks,
        Element::AddDesk,
        Element::Lounge,
        Element::Coffee,
        Element::Trophies,
        Element::Review,
        Element::Crates,
        Element::Returns,
        Element::Counter,
        Element::Queue,
        Element::Safe,
        Element::AddProject,
        Element::Hire,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Element::Board => "board",
            Element::Corkboard => "corkboard",
            Element::Posters => "posters",
            Element::Racks => "racks",
            Element::AddDesk => "add-desk",
            Element::Lounge => "lounge",
            Element::Coffee => "coffee",
            Element::Trophies => "trophies",
            Element::Review => "review",
            Element::Crates => "crates",
            Element::Returns => "returns",
            Element::Counter => "counter",
            Element::Queue => "queue",
            Element::Safe => "safe",
            Element::AddProject => "add-project",
            Element::Hire => "hire",
        }
    }

    pub fn from_key(key: &str) -> Option<Element> {
        Element::ALL.into_iter().find(|e| e.key() == key)
    }

    /// What it is, on hover or with "What's what".
    pub fn hint(self) -> &'static str {
        match self {
            Element::Board => "Whiteboard · goals",
            Element::Corkboard => "Team board · hire",
            Element::Posters => "House rules",
            Element::Racks => "Checks · every project",
            Element::AddDesk => "Add a desk",
            Element::Lounge => "Lounge · who's free",
            Element::Coffee => "Coffee · close the office",
            Element::Trophies => "Trophies",
            Element::Review => "Review · reviewers and critics",
            Element::Crates => "Shipped today",
            Element::Returns => "Returns · failed work",
            Element::Counter => "Front desk · approver, router",
            Element::Queue => "Up next",
            Element::Safe => "Safe · budget",
            Element::AddProject => "Add a project",
            Element::Hire => "Hire someone",
        }
    }
}

/// A label pinned over the floor, placed in percent of the scene's box.
#[derive(Clone, Debug, PartialEq)]
pub struct Pin {
    pub left: f64,
    pub top: f64,
    pub what: PinWhat,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PinWhat {
    /// A ticket at a desk, in checks or in review.
    Ticket(Worker),
    /// A ticket ready to land, with its Land button.
    Ready(Crate),
    /// How many tickets wait for a desk, at the front desk.
    Queue(usize),
    /// How many goals are being planned, at the whiteboard.
    Planning(usize),
    /// What a game element is, shown on hover or with "What's what".
    Hint(Element),
}

/// The floor drawn: an SVG for the page, its size in SVG units, and the
/// labels to pin over it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub width: f64,
    pub height: f64,
    pub svg: String,
    pub pins: Vec<Pin>,
}
