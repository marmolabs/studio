//! Every team page keeps itself up to date from one stream. A page opens it,
//! naming the address it is at; each second the office changed (or the
//! conversation it shows did), the stream draws that page again from the
//! shared snapshot and Datastar morphs it in by the `id`s it has: the sidebar,
//! the main area and the rail together. There is no other live update on a
//! team page.
//!
//! This is the one way a page takes part: draw yourself with `shell::page`,
//! as a function of the team and the office you are given, and have `draw`
//! below call you. What the owner has typed or opened inside the page (a text
//! box, an open note, a choice) is left as it is by the morph: a form that is
//! all the owner's goes under `data-ignore-morph`, a `<details>` keeps its
//! `open` and a toggled element its `style` with `data-preserve-attr`, and a
//! field that is only typed in is kept by Datastar as long as the page does not
//! change its `value`.
//!
//! The office is the one page whose drawing depends on something only the
//! browser knows, who is picked on the floor and the corner it is seen from, so
//! the office page opens the stream itself, again whenever either changes, and
//! the stream reads them from the page's signals.

use futures_util::Stream;
use std::sync::Arc;
use std::time::Instant;
use topcoat::{
    context::{Cx, app_context},
    datastar::Signals,
    router::{
        content::sse::{Event, Sse},
        query_params, route,
    },
    view::{BoxView, ViewExt, view},
};

use super::office::{Picked, clean};
use super::talk::thread::Sizes;
use super::{Team, company, hq, new, office, person, rooms, settings, sheet, talk};
use crate::pages::office::model::Turn;
use crate::pages::office::{Office, SHARED_FOR, safe, shared};
use crate::{AppConfig, live, ui};

/// What opens the stream of the page the browser is at.
pub const OPEN: &str =
    "@get('/team/stream?at=' + encodeURIComponent(location.pathname + location.search))";

/// Which page a stream keeps up to date.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spot {
    Hq,
    /// A lead's conversation, with the work whose sheet is open.
    Lead {
        project: String,
        look: Option<String>,
    },
    /// A hire's direct conversation.
    Person {
        hire: String,
    },
    Room {
        room: String,
        before: Option<u64>,
    },
    /// The company floor, seen from the corner `turn`, with `sel` picked.
    Office {
        sel: String,
        turn: Turn,
    },
    Company,
    New,
    Studio,
}

impl Spot {
    /// The page at `address`, a path with its query, as the browser has it;
    /// none when it is not a team page.
    pub fn of(address: &str) -> Option<Spot> {
        if !address.starts_with('/') {
            return None;
        }
        let (path, query) = address.split_once('?').unwrap_or((address, ""));
        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
        let asked = |key: &str| {
            url::form_urlencoded::parse(query.as_bytes())
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.into_owned())
        };
        match segments.as_slice() {
            [""] => Some(Spot::Hq),
            ["office"] => Some(Spot::Office {
                sel: String::new(),
                turn: Turn::new(asked("turn").and_then(|t| t.parse().ok()).unwrap_or(0)),
            }),
            ["company"] => Some(Spot::Company),
            ["new"] => Some(Spot::New),
            ["studio"] => Some(Spot::Studio),
            ["leads", project] if !project.is_empty() => Some(Spot::Lead {
                project: unescape(project),
                look: asked("look")
                    .map(|look| safe(&look))
                    .filter(|look| !look.is_empty()),
            }),
            ["people", hire] if !hire.is_empty() => Some(Spot::Person {
                hire: unescape(hire),
            }),
            ["rooms", room] if !room.is_empty() => Some(Spot::Room {
                room: unescape(room),
                before: asked("before").and_then(|before| before.parse().ok()),
            }),
            _ => None,
        }
    }
}

/// A path segment with its `%XX` escapes undone.
fn unescape(segment: &str) -> String {
    url::form_urlencoded::parse(format!("s={}", segment.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, text)| text.into_owned())
        .unwrap_or_default()
}

/// What a page is drawn from besides the office: what it reads from studiod.
enum Extra {
    Nothing,
    /// The sizes of the work that is ready to ship, in every project.
    Hq(Sizes),
    /// A conversation, with the sizes of what is ready to ship and the budget.
    Lead(Box<talk::Reads>),
    Person(studiod::api::ConversationView),
    Room(studiod::api::RoomView),
    /// What one task may cost, when a limit is set.
    Studio(Option<f64>),
}

/// What of an `Extra` tells whether the page changed, as the office's version
/// does not tell it: a message can come with nothing else in the office
/// changing.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Seen {
    Nothing,
    Lead(talk::Said),
    Person(talk::Said),
    Room(talk::Said),
    Studio(Option<f64>),
}

impl Extra {
    /// Reads what the page at `spot` shows besides the office.
    async fn of(spot: &Spot, config: &AppConfig, office: &Office, memo: &mut talk::Memo) -> Extra {
        match spot {
            Spot::Lead { project, .. } => {
                Extra::Lead(Box::new(talk::read(config, office, project, memo).await))
            }
            Spot::Person { hire } => Extra::Person(person::read(config, hire).await),
            Spot::Room { room, before } => Extra::Room(rooms::read(config, room, *before).await),
            Spot::Hq => Extra::Hq(talk::sizes_of_all(config, office, memo).await),
            Spot::Studio => Extra::Studio(settings::ticket_usd(config).await),
            Spot::Office { .. } | Spot::Company | Spot::New => Extra::Nothing,
        }
    }

    fn seen(&self) -> Seen {
        match self {
            // A goal's size moves only when more work lands on it, which the
            // office shows, so HQ is as the office says.
            Extra::Nothing | Extra::Hq(_) => Seen::Nothing,
            Extra::Lead(reads) => Seen::Lead(reads.said()),
            Extra::Person(conversation) => Seen::Person(talk::Said::of(conversation)),
            Extra::Room(conversation) => Seen::Room(talk::Said::of_room(conversation)),
            Extra::Studio(usd) => Seen::Studio(*usd),
        }
    }
}

/// The page at `spot` for `team`, `office` and what else it read; none when it
/// has no page, as for a project the studio does not have. `sheet` is the work
/// a conversation looks closer at.
fn draw<'a>(
    cx: &'a Cx,
    spot: &Spot,
    team: Team,
    office: Office,
    extra: Extra,
    sheet: Option<sheet::Sheet>,
) -> Option<BoxView<'a>> {
    Some(match (spot.clone(), extra) {
        (Spot::Hq, Extra::Hq(sizes)) => {
            view! { cx => hq::hq(team: team, office: office, sizes: sizes) }.boxed()
        }
        (Spot::Lead { project, .. }, Extra::Lead(reads)) => {
            let parts = talk::parts(&team, &office, &project, &reads)?;
            view! { cx => talk::talk(team: team, parts: parts, sheet: sheet) }.boxed()
        }
        (Spot::Person { hire }, Extra::Person(conversation)) => {
            let parts = person::parts(&office, &hire, conversation)?;
            view! { cx => person::person_view(team: team, parts: parts) }.boxed()
        }
        (Spot::Room { room, .. }, Extra::Room(conversation)) => {
            if !office.rooms.iter().any(|summary| summary.id == room) {
                return None;
            }
            view! { cx => rooms::room_view(team: team, office: office, room: room, conversation: conversation) }.boxed()
        }
        (Spot::Office { sel, turn }, _) => {
            view! { cx => office::office_view(team: team, office: office, turn: turn, sel: sel) }
                .boxed()
        }
        (Spot::Company, _) => view! { cx => company::company(team: team, office: office) }.boxed(),
        (Spot::New, _) => view! { cx => new::new_project(team: team, office: office) }.boxed(),
        (Spot::Studio, Extra::Studio(ticket_usd)) => {
            view! { cx => settings::settings(team: team, office: office, ticket_usd: ticket_usd) }
                .boxed()
        }
        (
            Spot::Hq | Spot::Lead { .. } | Spot::Person { .. } | Spot::Room { .. } | Spot::Studio,
            _,
        ) => return None,
    })
}

#[query_params(error = bad_request)]
struct Watching {
    /// The address of the page, with its query.
    at: Option<String>,
}

/// The page the stream was opened from, drawn again whenever what it shows
/// changes.
#[route(GET "/team/stream")]
async fn team_stream(
    cx: &Cx,
    signals: Option<Signals<Picked>>,
) -> topcoat::Result<Sse<impl Stream<Item = topcoat::Result<Event>> + Send + use<>>> {
    let config = app_context::<AppConfig>(cx).clone();
    let mut spot = query_params::<Watching>(cx)?
        .at
        .as_deref()
        .and_then(Spot::of);
    // The floor is drawn as the owner has it: the corner it is seen from and
    // who is picked, as the page's signals say when it opens the stream again.
    if let (Some(Spot::Office { sel, turn }), Some(Signals(picked))) = (&mut spot, signals) {
        *sel = clean(&picked.sel);
        *turn = Turn::new(picked.turn);
    }
    let cx = cx.clone();
    let kept = Arc::new(tokio::sync::Mutex::new(Kept::default()));
    Ok(live::watch_changes(move || {
        let (config, cx, spot, kept) = (config.clone(), cx.clone(), spot.clone(), kept.clone());
        async move {
            match spot {
                Some(spot) => render(&cx, &config, &spot, &mut *kept.lock().await).await,
                None => Ok(None),
            }
        }
    }))
}

/// What a stream keeps from one drawing to the next.
#[derive(Default)]
struct Kept {
    /// What a page keeps of what it read: the sizes of the goals that are
    /// ready to ship, which move only when more work lands on a goal, and the
    /// budget, which moves slowly.
    memo: talk::Memo,
    /// When the last drawing was made, the snapshot it was made from (as
    /// `shared` numbers it) and what else it was made from.
    drawn: Option<(Instant, u64, Seen)>,
}

/// The page drawn from the shared snapshot, unless what it is drawn from is
/// what the last drawing was made from and under `live::QUIET_MAX` has passed
/// since. Then only the ages ("5m") would be newer.
async fn render(
    cx: &Cx,
    config: &AppConfig,
    spot: &Spot,
    kept: &mut Kept,
) -> topcoat::Result<Option<String>> {
    let (version, office) = shared(config, SHARED_FOR).await?;
    let extra = Extra::of(spot, config, &office, &mut kept.memo).await;
    let seen = extra.seen();
    if let Some((at, last, last_seen)) = &kept.drawn
        && *last == version
        && *last_seen == seen
        && at.elapsed() < live::QUIET_MAX
    {
        return Ok(None);
    }
    let team = Team::of(&office);
    // The work a conversation looks closer at is read only for a drawing.
    let sheet = match spot {
        Spot::Lead {
            project,
            look: Some(look),
        } => match team.lead(project) {
            Some(lead) => sheet::gather(config, &office, project, look, lead.tint).await,
            None => None,
        },
        _ => None,
    };
    let Some(page) = draw(cx, spot, team, office, extra, sheet) else {
        return Ok(None);
    };
    kept.drawn = Some((Instant::now(), version, seen));
    Ok(Some(ui::html(cx, page).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn office(sel: &str, turn: i64) -> Option<Spot> {
        Some(Spot::Office {
            sel: sel.into(),
            turn: Turn::new(turn),
        })
    }

    #[test]
    fn a_stream_knows_the_team_page_it_was_opened_from() {
        assert_eq!(Spot::of("/"), Some(Spot::Hq));
        assert_eq!(Spot::of("/?x=1"), Some(Spot::Hq));
        assert_eq!(Spot::of("/office"), office("", 0));
        assert_eq!(Spot::of("/new"), Some(Spot::New));
        assert_eq!(Spot::of("/studio"), Some(Spot::Studio));
        let lead = |project: &str, look: Option<&str>| {
            Some(Spot::Lead {
                project: project.into(),
                look: look.map(str::to_string),
            })
        };
        assert_eq!(Spot::of("/leads/shop"), lead("shop", None));
        assert_eq!(
            Spot::of("/leads/shop?look=SHOP-1"),
            lead("shop", Some("SHOP-1"))
        );
        // The project's name is unescaped, the look kept to what is safe.
        assert_eq!(Spot::of("/leads/my%20shop"), lead("my shop", None));
        assert_eq!(Spot::of("/leads/a%2Bb"), lead("a+b", None));
        assert_eq!(
            Spot::of("/leads/shop?look=a%27b%3Cc"),
            lead("shop", Some("abc"))
        );
        assert_eq!(Spot::of("/leads/shop?look="), lead("shop", None));
        assert_eq!(
            Spot::of("/people/dev"),
            Some(Spot::Person { hire: "dev".into() })
        );
        assert_eq!(
            Spot::of("/people/a%2Bb"),
            Some(Spot::Person { hire: "a+b".into() })
        );
    }

    #[test]
    fn the_office_is_seen_from_the_corner_its_address_names() {
        assert_eq!(Spot::of("/office?turn=2"), office("", 2));
        assert_eq!(Spot::of("/office?turn=-1"), office("", 3));
        assert_eq!(Spot::of("/office?turn=x"), office("", 0));
    }

    #[test]
    fn other_addresses_are_not_team_pages() {
        for address in [
            "/leads",
            "/leads/",
            "/leads/shop/talk",
            "/hires",
            "/rules",
            "/tickets/SHOP-1",
            "/office/classic",
            "/office/stream",
            "/team/stream",
            "",
        ] {
            assert_eq!(Spot::of(address), None, "{address}");
        }
    }
}
