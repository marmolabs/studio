//! Pieces every page uses, styled after `design/prototype.html`.
//!
//! Colours come from the theme tokens in `app.css` (`bg`, `bg-2`…`bg-4`,
//! `line`…`line-3`, `fg`…`fg-3`, and `attn`, `run`, `ok`, `bad` with their
//! `-fg`, `-bg` and `-line` shades). Class strings are literals so Tailwind
//! finds them.

use std::sync::atomic::{AtomicU64, Ordering};
use studio_config::draft::Draft;
use studio_core::machine::MAX_ATTEMPTS;
use studio_core::ticket::{FailureClass, HeldReason, Ticket, TicketState};
use topcoat::{
    context::Cx,
    datastar::{ElementPatchMode, PatchElements},
    view::{Child, View, ViewExt, component, view},
};

/// The state glyphs and the logo as `<symbol>`s, inlined once per page next
/// to the Lucide sprite. `state_icon` and the brand mark use them.
pub const GLYPHS: &str = r#"<svg width="0" height="0" style="position:absolute" aria-hidden="true">
<symbol id="s-queued" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" stroke-width="1.5" stroke-dasharray="2.4 2.2"/></symbol>
<symbol id="s-attn" viewBox="0 0 16 16"><circle cx="8" cy="8" r="7" fill="currentColor"/><rect x="7.1" y="3.9" width="1.8" height="5.2" rx=".9" style="fill:var(--bg)"/><circle cx="8" cy="11.4" r="1.05" style="fill:var(--bg)"/></symbol>
<symbol id="s-held" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" stroke-width="1.5"/><rect x="5.6" y="5.2" width="1.6" height="5.6" rx=".6" fill="currentColor"/><rect x="8.8" y="5.2" width="1.6" height="5.6" rx=".6" fill="currentColor"/></symbol>
<symbol id="s-wait" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" stroke-width="1.5"/><rect x="5.6" y="5.2" width="1.6" height="5.6" rx=".6" fill="currentColor"/><rect x="8.8" y="5.2" width="1.6" height="5.6" rx=".6" fill="currentColor"/></symbol>
<symbol id="s-ready" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M5.2 8.3l1.9 1.9 3.8-4.1" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></symbol>
<symbol id="s-landed" viewBox="0 0 16 16"><circle cx="8" cy="8" r="7" fill="currentColor"/><path d="M5.1 8.3l1.95 1.95 3.85-4.15" fill="none" style="stroke:var(--bg)" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></symbol>
<symbol id="s-failed" viewBox="0 0 16 16"><circle cx="8" cy="8" r="7" fill="currentColor"/><path d="M5.6 5.6l4.8 4.8M10.4 5.6l-4.8 4.8" fill="none" style="stroke:var(--bg)" stroke-width="1.7" stroke-linecap="round"/></symbol>
<symbol id="s-cancelled" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M4.2 11.8l7.6-7.6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></symbol>
<symbol id="logo" viewBox="0 0 16 16"><rect x="2.5" y="5.5" width="8" height="8" rx="2" fill="none" stroke="currentColor" stroke-width="1.6"/><path d="M5.5 2.5h6a2 2 0 0 1 2 2v6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></symbol>
</svg>"#;

// Text.
pub const PAGE_TITLE: &str = "text-[26px] font-semibold leading-[34px] tracking-[-0.02em] [text-wrap:balance] max-[760px]:text-[22px] max-[760px]:leading-[30px]";
pub const SECTION_TITLE: &str = "text-[14px] font-semibold leading-5 text-fg";
pub const SECONDARY: &str = "text-fg-2";
pub const MUTED: &str = "text-fg-3";
/// Ids, branches, paths, commands and models.
pub const MONO: &str = "font-mono text-[12.5px]";
/// Inline code in a line of text, such as a command in a row's detail.
pub const CODE: &str = "mx-0.5 whitespace-nowrap rounded-[5px] border border-line bg-bg-3 px-[5px] font-mono text-[12px] leading-[18px] text-fg";
/// Lines added and removed.
pub const ADD: &str = "text-ok-fg";
pub const DEL: &str = "text-bad-fg";
/// Why something waits for the owner, after a row's detail.
pub const REASON: &str = "ml-1.5 text-attn-fg";
/// A button's words, hidden on a phone where the icon is enough.
pub const LABEL: &str = "max-[760px]:hidden";

// Containers: one bordered box per group, 10px corners.
pub const CARD: &str = "rounded-[10px] border border-line bg-bg";
/// The box around a grouped list.
pub const LIST: &str = "overflow-hidden rounded-[10px] border border-line bg-bg";

// Tables: wrap in TABLE_BOX; no vertical lines.
pub const TABLE_BOX: &str = "overflow-x-auto rounded-[10px] border border-line";
pub const TABLE: &str = "w-full border-collapse text-left text-[13px]";
pub const TH: &str = "whitespace-nowrap border-b border-line bg-bg-2 px-4 py-2.5 text-[12px] font-medium leading-4 text-fg-3";
pub const TD: &str = "whitespace-nowrap border-t border-line px-4 py-[11px] align-middle";
/// A title cell, which may wrap.
pub const TD_TITLE: &str =
    "min-w-[220px] border-t border-line px-4 py-[11px] align-middle font-medium";
pub const TR: &str = "hover:bg-hover";

// Buttons: 32px (28px small), 7px corners; at most one primary per page.
macro_rules! button {
    ($size:literal, $look:expr) => {
        concat!(
            "inline-flex max-w-full shrink-0 cursor-pointer items-center justify-center gap-1.5 whitespace-nowrap border font-medium leading-none transition-colors disabled:cursor-default disabled:opacity-50 ",
            $size,
            " ",
            $look
        )
    };
}
macro_rules! looks {
    (default) => {
        "border-line-2 bg-bg-2 text-fg hover:border-line-3 hover:bg-bg-3"
    };
    (primary) => {
        "border-transparent bg-primary text-on-primary hover:bg-primary-hover"
    };
    (ghost) => {
        "border-transparent bg-transparent text-fg-2 hover:bg-bg-3 hover:text-fg"
    };
    (danger) => {
        "border-line-2 bg-bg-2 text-fg hover:border-bad-line hover:bg-bad-bg hover:text-bad-fg"
    };
    (ghost_danger) => {
        "border-transparent bg-transparent text-fg-2 hover:bg-bad-bg hover:text-bad-fg"
    };
}
pub const BUTTON: &str = button!("h-8 rounded-[7px] px-3 text-[13px]", looks!(default));
pub const PRIMARY: &str = button!("h-8 rounded-[7px] px-3 text-[13px]", looks!(primary));
pub const GHOST: &str = button!("h-8 rounded-[7px] px-3 text-[13px]", looks!(ghost));
/// Quiet until hovered, for Delete and Cancel.
pub const DANGER: &str = button!("h-8 rounded-[7px] px-3 text-[13px]", looks!(danger));
pub const BUTTON_SM: &str = button!("h-7 rounded-md px-2.5 text-[12.5px]", looks!(default));
pub const PRIMARY_SM: &str = button!("h-7 rounded-md px-2.5 text-[12.5px]", looks!(primary));
pub const GHOST_SM: &str = button!("h-7 rounded-md px-2.5 text-[12.5px]", looks!(ghost));
/// Quiet until hovered, then red: Delete in an editor's footer.
pub const GHOST_DANGER_SM: &str =
    button!("h-7 rounded-md px-2.5 text-[12.5px]", looks!(ghost_danger));
pub const ICON_BUTTON_SM: &str = button!("size-7 rounded-md px-0", looks!(ghost));
/// A line in a popover `menu`.
pub const MENU_ITEM: &str = button!(
    "h-8 w-full justify-start! rounded-[7px] px-3 text-[13px]",
    looks!(ghost)
);
pub const MENU_ITEM_DANGER: &str = button!(
    "h-8 w-full justify-start! rounded-[7px] px-3 text-[13px]",
    looks!(ghost_danger)
);

// Forms.
pub const INPUT: &str = "min-h-9 w-full rounded-lg border border-line-2 bg-bg px-3 py-[7px] text-[14px] leading-5 text-fg focus:border-ring focus:shadow-[0_0_0_3px_color-mix(in_srgb,var(--ring)_25%,transparent)] focus:outline-none";
pub const TEXTAREA: &str = "min-h-26 w-full resize-y rounded-lg border border-line-2 bg-bg px-3 py-2.5 text-[14px] leading-5 text-fg focus:border-ring focus:shadow-[0_0_0_3px_color-mix(in_srgb,var(--ring)_25%,transparent)] focus:outline-none";
/// A label above its input, with an optional `HINT` below.
pub const FIELD: &str = "flex flex-col gap-1.5";
pub const FIELD_LABEL: &str = "text-[12.5px] font-medium leading-4 text-fg-2";
pub const HINT: &str = "text-[12px] text-fg-3";

// Inside a `dialog`.
pub const DIALOG_BODY: &str = "flex flex-col gap-3.5 px-[18px] pb-[18px] pt-2.5";
pub const DIALOG_FOOT: &str =
    "flex items-center gap-2 border-t border-line bg-bg-2 px-[18px] py-3 [&>.spacer]:flex-1";

// Inside an `ask_frame`.
/// A command or a single value the agent wants to use, wrapped so the owner
/// sees all of it before allowing it.
pub const CMD: &str = "overflow-y-auto whitespace-pre-wrap rounded-lg border border-line bg-bg-2 px-3.5 py-2.5 font-mono text-[13px] leading-5 text-fg [overflow-wrap:anywhere]";
/// The rule or reason that sent it to the owner.
pub const WHY: &str = "text-[13px] leading-5 text-fg-3";
pub const ASK_ACTIONS: &str = "flex flex-wrap items-center gap-2";

// Inside a `kpi`.
pub const KPI_SUB: &str = "truncate text-[12.5px] text-fg-3";
/// A `KPI_SUB` of parts that may wrap, a part at a time, each after a dot.
/// A line's first dot sits in the clipped margin, so no line starts or ends
/// with one.
pub const KPI_PARTS: &str = "-ml-3 flex flex-wrap text-[12.5px] text-fg-3";
pub const KPI_PART: &str =
    "whitespace-nowrap before:inline-block before:w-3 before:text-center before:content-['·']";
// On a `row`'s right: a figure such as an age or a cost.
pub const FIGURE: &str = "min-w-11 text-right max-[760px]:min-w-0 max-[760px]:text-left";

/// Renders a view to HTML, for patches sent over a stream or a POST reply.
pub async fn html(cx: &Cx, view: impl View) -> topcoat::Result<String> {
    Ok(view.single().await?.render(cx))
}

/// Numbers each notice.
static NOTICES: AtomicU64 = AtomicU64::new(0);

/// Puts a short message in the page's `#notice` slot.
pub async fn notice(cx: &Cx, message: String, error: bool) -> topcoat::Result<PatchElements> {
    // A new id each time, so the page takes it as a new message (the office
    // fades each one out) rather than changing the text of the last one.
    let id = format!("notice-{}", NOTICES.fetch_add(1, Ordering::Relaxed));
    let text = html(
        cx,
        view! { cx =>
            if error {
                <p id=(id) class="whitespace-pre-wrap text-bad-fg">(message)</p>
            } else {
                <p id=(id) class="text-fg-3">(message)</p>
            }
        },
    )
    .await?;
    Ok(PatchElements::new(text)
        .selector("#notice")
        .mode(ElementPatchMode::Inner))
}

/// A notice followed by a link to the new-rule editor, filled in with `draft`.
pub async fn rule_notice(
    cx: &Cx,
    message: String,
    label: String,
    draft: &Draft,
) -> topcoat::Result<PatchElements> {
    let encode = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let href = format!(
        "/rules?new=1&name={}&text={}",
        encode(&draft.name),
        encode(&draft.text)
    );
    let text = html(
        cx,
        view! { cx =>
            <p class="text-fg-3">
                (message) " "
                <a href=(href) class="underline hover:text-fg">(label)</a>
            </p>
        },
    )
    .await?;
    Ok(PatchElements::new(text)
        .selector("#notice")
        .mode(ElementPatchMode::Inner))
}

/// Dollars to the cent, or four places below a cent. Anything that would
/// print as zero (including negative zero) is "$0.00".
pub fn usd(amount: f64) -> String {
    if (amount * 10_000.0).round() == 0.0 {
        "$0.00".into()
    } else if amount.abs() < 0.01 {
        format!("${amount:.4}")
    } else {
        format!("${amount:.2}")
    }
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// "45s", "12m" or "1h 05m" for how long a run has gone on.
pub fn elapsed(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h {:02}m", secs / 3600, secs % 3600 / 60),
    }
}

/// Whether a held ticket waits for money or the provider rather than the owner.
pub fn waits_on_its_own(state: &TicketState) -> bool {
    matches!(
        state,
        TicketState::Held {
            reason: HeldReason::Budget | HeldReason::Quota
        }
    )
}

/// Whether a ticket waits for the owner: it asks, or it is held for a reason
/// that doesn't end on its own.
pub fn needs_owner(state: &TicketState) -> bool {
    match state.kind() {
        "asking" => true,
        "held" => !waits_on_its_own(state),
        _ => false,
    }
}

/// The state a ticket is drawn as: a hold that ends on its own is "on hold",
/// in grey, since it doesn't wait for the owner.
pub fn shown(state: &TicketState) -> &'static str {
    if waits_on_its_own(state) {
        "on hold"
    } else {
        state.kind()
    }
}

/// What a queued ticket waits for, as studiod's scheduler sees it.
#[derive(Clone, Debug, PartialEq)]
pub enum Wait {
    /// The owner closed the office, so nothing new starts.
    Closed,
    /// `max_runs` is 0, so nothing new starts.
    Paused,
    /// Another ticket on its goal, which runs one at a time.
    After(String),
    /// Money for a run.
    Budget,
    /// A free run slot.
    Slot,
}

impl Wait {
    pub fn of(ticket: &Ticket, tickets: &[Ticket], status: &crate::client::Status) -> Wait {
        let (out_of_budget, max_runs) = (status.out_of_budget, status.max_runs);
        let goal = |t: &Ticket| {
            if t.goal.trim().is_empty() {
                t.id.to_string()
            } else {
                t.goal.clone()
            }
        };
        let same_goal = |t: &&Ticket| {
            t.id != ticket.id && t.project == ticket.project && goal(t) == goal(ticket)
        };
        let busy =
            |t: &&Ticket| !matches!(t.state.kind(), "queued" | "landed" | "failed" | "cancelled");
        // Queued tickets start oldest first, which is the order studiod lists them in.
        let here = tickets.iter().position(|t| t.id == ticket.id).unwrap_or(0);
        let blocker = tickets.iter().filter(same_goal).find(busy).or_else(|| {
            tickets[..here]
                .iter()
                .filter(same_goal)
                .find(|t| t.state.kind() == "queued")
        });
        match blocker {
            _ if status.closed => Wait::Closed,
            _ if max_runs == 0 => Wait::Paused,
            Some(t) => Wait::After(t.id.to_string()),
            None if out_of_budget => Wait::Budget,
            None => Wait::Slot,
        }
    }

    /// A few words, for a row.
    pub fn short(&self) -> String {
        match self {
            Wait::Closed => "Office closed".into(),
            Wait::Paused => "Paused".into(),
            Wait::After(id) => format!("After {id}"),
            Wait::Budget => "Waiting for budget".into(),
            Wait::Slot => "Waiting for a free desk".into(),
        }
    }

    /// A sentence, for a panel.
    pub fn sentence(&self) -> String {
        match self {
            Wait::Closed => "The office is closed: it starts when you reopen it.".into(),
            Wait::Paused => "Paused: max_runs is 0, so nothing new starts.".into(),
            Wait::After(id) => {
                format!("Starts after {id} is done: tickets on a goal run one at a time.")
            }
            Wait::Budget => "Waits for budget: the studio can't fund another run today.".into(),
            Wait::Slot => "Queued. It starts when a desk frees up.".into(),
        }
    }
}

/// Where a goal stands, from its plan ticket and the tickets it filed
/// (cancelled ones left out). A landed plan only means its tickets were
/// filed; one that filed nothing leaves the goal for the owner to look at.
pub fn goal_state(plan: &Ticket, filed: &[Ticket], merged: bool) -> &'static str {
    let any = |kinds: &[&str]| filed.iter().any(|t| kinds.contains(&t.state.kind()));
    let trouble = |t: &Ticket| t.state.kind() == "failed" || needs_owner(&t.state);
    let running = ["claimed", "working", "checking", "reviewing"];
    let landed = filed.iter().all(|t| t.state.kind() == "landed");
    match plan.state.kind() {
        "failed" => "failed",
        "cancelled" => "cancelled",
        _ if trouble(plan) || filed.iter().any(trouble) => "needs attention",
        "landed" if filed.is_empty() => "needs attention",
        "landed" if landed && merged => "merged",
        "landed" if landed => "done",
        "queued" if filed.is_empty() => "queued",
        _ if filed.is_empty() => "planning",
        plan if running.contains(&plan) || any(&running) => "in progress",
        _ if any(&["queued"]) => "queued",
        _ => "in progress",
    }
}

/// A failed ticket's headline and why: ("Checks failed", "cargo clippy: 2
/// warnings").
pub fn failure(state: &TicketState) -> Option<(String, String)> {
    let detail = state_detail(state).filter(|_| state.kind() == "failed")?;
    let (class, why) = detail.split_once(": ").unwrap_or((&detail, ""));
    let mut chars = class.chars();
    let first = chars.next()?;
    Some((first.to_uppercase().chain(chars).collect(), why.to_string()))
}

/// A limit in whole dollars when it is one ("$10"), else to the cent, for
/// "of $10" and limit figures; amounts spent use `usd`.
pub fn limit(amount: f64) -> String {
    if amount.fract().abs() < 1e-9 {
        format!("${amount:.0}")
    } else {
        usd(amount)
    }
}

/// The state name, plus why for held and failed tickets.
pub fn state_detail(state: &TicketState) -> Option<String> {
    match state {
        TicketState::Held { reason } => Some(
            match reason {
                HeldReason::Budget => "out of budget",
                HeldReason::Quota => "provider quota",
                HeldReason::Auth => "needs login",
                HeldReason::Conflict => "merge conflict",
                HeldReason::Paused => "paused",
            }
            .to_string(),
        ),
        TicketState::Failed { class, detail } => {
            let class = match class {
                FailureClass::Crash => "crashed",
                FailureClass::Timeout => "timed out",
                FailureClass::Checks => "checks failed",
                FailureClass::Review => "review failed",
                FailureClass::Tooling => "tooling",
                FailureClass::Denied => "denied",
            };
            Some(if detail.is_empty() {
                class.to_string()
            } else {
                format!("{class}: {detail}")
            })
        }
        _ => None,
    }
}

/// How a state reads: a ticket's (`TicketState::kind`) or a goal's
/// (`planning`, `in progress`, `needs attention`, `done`, `merged`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// `attn`, `run`, `ok`, `bad` or `neutral`.
    pub tone: &'static str,
    /// The `s-*` glyph, or `None` for a running state, drawn as a ring.
    pub glyph: Option<&'static str>,
    /// How full the running ring is, in percent.
    pub ring: u8,
    /// The state in words, sentence case.
    pub label: &'static str,
}

pub fn look(state: &str) -> Look {
    let still = |tone, glyph, label| Look {
        tone,
        glyph: Some(glyph),
        ring: 0,
        label,
    };
    let running = |percent, label| Look {
        tone: "run",
        glyph: None,
        ring: percent,
        label,
    };
    match state {
        "claimed" => running(25, "Starting"),
        "working" => running(25, "Working"),
        "checking" => running(50, "Checks"),
        "reviewing" => running(85, "Review"),
        "planning" => running(25, "Planning"),
        "in progress" => running(50, "Running"),
        "asking" | "needs attention" => still("attn", "attn", "Needs you"),
        "held" => still("attn", "held", "Held"),
        "on hold" => still("neutral", "wait", "On hold"),
        "ready" => still("ok", "ready", "Ready to land"),
        "done" => still("ok", "ready", "Done"),
        "landed" => still("ok", "landed", "Landed"),
        "merged" => still("ok", "landed", "Merged"),
        "failed" => still("bad", "failed", "Failed"),
        "cancelled" => still("neutral", "cancelled", "Cancelled"),
        _ => still("neutral", "queued", "Queued"),
    }
}

/// A Lucide icon from the sprite, 16px (14px when `small`).
#[component]
pub async fn icon(name: &'static str, #[default] small: bool) -> topcoat::Result<impl View> {
    let class = if small { "i sm" } else { "i" };
    Ok(view! {
        <svg class=(class) aria-hidden="true"><use href=(format!("#i-{name}"))></use></svg>
    })
}

/// The conic ring of a running ticket, filled to `percent`.
#[component]
pub async fn ring(percent: u8, #[default] small: bool) -> topcoat::Result<impl View> {
    let class = if small { "prog sm" } else { "prog" };
    Ok(view! {
        <span class=(class) style=(format!("--p:{}", percent.min(100))) aria-hidden="true"></span>
    })
}

/// A state's glyph: an `s-*` symbol in the state's colour, or the running
/// ring at its step.
#[component]
pub async fn state_icon(state: &'static str, #[default] small: bool) -> topcoat::Result<impl View> {
    let look = look(state);
    Ok(view! {
        if let Some(glyph) = look.glyph {
            <svg class=(format!("st {glyph}{}", if small { " sm" } else { "" })) aria-hidden="true">
                <use href=(format!("#s-{glyph}"))></use>
            </svg>
        } else {
            ring(percent: look.ring, small: small)
        }
    })
}

/// Literal pill classes per tone.
fn pill_class(tone: &str) -> &'static str {
    match tone {
        "attn" => "text-attn-fg bg-attn-bg",
        "run" => "text-run-fg bg-run-bg",
        "ok" => "text-ok-fg bg-ok-bg",
        "bad" => "text-bad-fg bg-bad-bg",
        _ => "text-fg-2 bg-bg-3",
    }
}

const PILL: &str = "inline-flex h-6 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-full pl-[7px] pr-[9px] text-[12.5px] font-medium leading-none";

/// The state's glyph and its name on a soft fill of its colour.
#[component]
pub async fn state_pill(state: &'static str) -> topcoat::Result<impl View> {
    let look = look(state);
    Ok(view! {
        <span class=(format!("{PILL} {}", pill_class(look.tone)))>
            state_icon(state: state, small: true)
            (look.label)
        </span>
    })
}

/// A pill with your own words: `tone` is `attn`, `run`, `ok`, `bad` or
/// `neutral`, and `state` picks the glyph (empty for none).
#[component]
pub async fn pill(
    tone: &'static str,
    #[into] label: String,
    #[default] state: &'static str,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <span class=(format!("{PILL} {}", pill_class(tone)))>
            if !state.is_empty() {
                state_icon(state: state, small: true)
            }
            (label)
        </span>
    })
}

/// An 8px dot in the state's colour, as in the KPI labels.
#[component]
pub async fn dot(state: &'static str) -> topcoat::Result<impl View> {
    let colour = match look(state).tone {
        "attn" => "bg-attn",
        "run" => "bg-run",
        "ok" => "bg-ok",
        "bad" => "bg-bad",
        _ => "bg-fg-3",
    };
    Ok(view! {
        <span class=(format!("inline-block size-2 shrink-0 rounded-full {colour}"))></span>
    })
}

/// Up to two letters for a hire: "EN" for `engineer`, "DC" for `design-critic`.
pub fn initials(hire: &str) -> String {
    let words: Vec<&str> = hire
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let letters: String = match words.as_slice() {
        [] => "?".into(),
        [one] => one.chars().take(2).collect(),
        [first, second, ..] => first
            .chars()
            .take(1)
            .chain(second.chars().take(1))
            .collect(),
    };
    letters.to_uppercase()
}

/// The hue a hire always gets, from its id.
pub fn hue(hire: &str) -> &'static str {
    const HUES: [&str; 5] = ["cyan", "violet", "pink", "teal", "gold"];
    // FNV-1a, so the choice is the same on every run and machine.
    let hash = hire.bytes().fold(0x811c_9dc5_u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    HUES[hash as usize % HUES.len()]
}

/// A hire's initials in a circle of its hue; 24px when `large`. A hire that
/// works for the studio (`staff`) gets a neutral circle.
#[component]
pub async fn avatar(
    #[into] hire: String,
    #[default] large: bool,
    #[default] staff: bool,
) -> topcoat::Result<impl View> {
    let hue = if staff { "" } else { hue(&hire) };
    let class = format!("av {hue}{}", if large { " lg" } else { "" });
    Ok(view! {
        <span class=(class) title=(hire.clone()) aria-hidden="true">(initials(&hire))</span>
    })
}

/// A hire's avatar and name.
#[component]
pub async fn who(#[into] hire: String, #[default] staff: bool) -> topcoat::Result<impl View> {
    Ok(view! {
        <span class="inline-flex items-center gap-[7px] whitespace-nowrap">
            avatar(hire: hire.clone(), staff: staff)
            (hire)
        </span>
    })
}

/// A small outlined label with an icon, such as a goal or a filter.
#[component]
pub async fn chip(symbol: &'static str, #[into] text: String) -> topcoat::Result<impl View> {
    Ok(view! {
        <span class="inline-flex h-[22px] max-w-full items-center gap-[5px] whitespace-nowrap rounded-md border border-line px-[7px] text-[12px] text-fg-2 [&>.i]:size-3 [&>.i]:text-fg-3">
            icon(name: symbol)
            <span class="truncate">(text)</span>
        </span>
    })
}

/// A goal's chip: the target icon and the goal's first line.
#[component]
pub async fn goal_chip(#[into] goal: String) -> topcoat::Result<impl View> {
    let name = goal.lines().next().unwrap_or_default().to_string();
    Ok(view! {
        <span title=(goal.clone()) class="contents">chip(symbol: "target", text: name)</span>
    })
}

/// A model's name in mono on a quiet fill.
#[component]
pub async fn model_tag(#[into] model: String) -> topcoat::Result<impl View> {
    Ok(view! {
        <span class="whitespace-nowrap rounded-[5px] bg-bg-3 px-1.5 py-1 font-mono text-[11.5px] font-medium leading-none text-fg-2">(model)</span>
    })
}

/// A neutral label for a fact such as a project or a rule's level.
#[component]
pub async fn badge(#[into] text: String) -> topcoat::Result<impl View> {
    Ok(view! {
        <span title=(text.clone()) class="inline-block max-w-full truncate rounded-[5px] bg-bg-3 px-1.5 py-px text-[12px] font-medium text-fg-2">
            (text)
        </span>
    })
}

/// "+184 −12" in green and red.
#[component]
pub async fn diff_size(added: u64, removed: u64) -> topcoat::Result<impl View> {
    Ok(view! {
        <span class="whitespace-nowrap tabular-nums">
            <span class=(ADD)>"+" (added)</span> " " <span class=(DEL)>"−" (removed)</span>
        </span>
    })
}

/// The width of a fill as a percentage, clamped to 0..=100.
fn percent(value: f64, max: f64) -> f64 {
    if max > 0.0 && value > 0.0 {
        (value / max * 100.0).min(100.0)
    } else {
        0.0
    }
}

/// A 4px track filled `value` of `max`. `tone` is empty for the neutral
/// fill, or `faint`, `ok`, `run`, `attn` or `bad`. It fills its container;
/// give the container a width.
#[component]
pub async fn meter(
    value: f64,
    max: f64,
    #[default] tone: &'static str,
) -> topcoat::Result<impl View> {
    let fill = match tone {
        "faint" => "bg-fg-3",
        "ok" => "bg-ok",
        "run" => "bg-run",
        "attn" => "bg-attn",
        "bad" => "bg-bad",
        _ => "bg-fg-2",
    };
    let share = percent(value, max);
    Ok(view! {
        <div class="h-1 overflow-hidden rounded-sm bg-bg-4" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow=(format!("{share:.0}"))>
            <i class=(format!("block h-full rounded-sm {fill}")) style=(format!("width:{share:.1}%"))></i>
        </div>
    })
}

/// The four steps of a run, with the current one half full, then the step's
/// name and which try this is: "Checks · try 1 of 3". `step` is 0 to 3, as
/// `run_step` gives; `attempt` counts from 1.
#[component]
pub async fn steps(step: usize, attempt: u32) -> topcoat::Result<impl View> {
    let label = STEPS.get(step).copied().unwrap_or_default();
    Ok(view! {
        <div class="flex w-[132px] flex-col gap-[5px]">
            step_bar(step: step)
            <span class="whitespace-nowrap text-[12px] text-fg-3">
                <b class="font-medium text-run-fg">(label)</b> " · try " (attempt) " of " (MAX_ATTEMPTS)
            </span>
        </div>
    })
}

/// The bar of `steps` alone; it fills its container.
#[component]
pub async fn step_bar(step: usize) -> topcoat::Result<impl View> {
    let bars = (0..STEPS.len()).map(move |i| match i.cmp(&step) {
        std::cmp::Ordering::Less => "done",
        std::cmp::Ordering::Equal => "now",
        std::cmp::Ordering::Greater => "",
    });
    Ok(view! {
        <div class="steps-bar grid grid-cols-4 gap-[3px]">
            for class in bars { <i class=(class)></i> }
        </div>
    })
}

/// What a plan shows where a work ticket shows its `steps`: a plan runs
/// once, with no checks or review.
#[component]
pub async fn planning() -> topcoat::Result<impl View> {
    Ok(view! {
        <span class="w-[132px] whitespace-nowrap text-[12px] font-medium text-run-fg">"Planning"</span>
    })
}

/// The steps of a run, in the order studiod runs them: the reviewer reads
/// the change before the critics of any gates that apply.
pub const STEPS: [&str; 4] = ["Working", "Checks", "Review", "Critics"];

/// Which of `STEPS` a running ticket is on. A reviewing ticket is on
/// "Review" until a critic takes over, which only its run log says.
pub fn run_step(state: &str) -> Option<usize> {
    match state {
        "claimed" | "working" => Some(0),
        "checking" => Some(1),
        "reviewing" => Some(2),
        _ => None,
    }
}

/// The step of a critic at work, after the reviewer.
pub const CRITICS: usize = 3;

/// A row of figures at the top of a page; put `kpi`s in it.
#[component]
pub async fn kpis(child: Child<'_>) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="grid grid-cols-4 rounded-[10px] border border-line bg-bg-2 max-[760px]:grid-cols-2">
            (child)
        </div>
    })
}

const KPI: &str = "relative flex min-w-0 flex-col gap-1 border-line px-[18px] pb-4 pt-3.5 [&:not(:first-child)]:border-l max-[760px]:[&:nth-child(3)]:border-l-0 max-[760px]:[&:nth-child(n+3)]:border-t";

/// One figure: a label with the `dot` of an optional `state`, the value with an
/// optional `unit` ("of $100"), then `child` (a `KPI_SUB` line or a
/// `sparkline`). With `href` the whole figure is a link.
#[component]
pub async fn kpi(
    label: &'static str,
    #[default] state: &'static str,
    #[into] value: String,
    #[into]
    #[default]
    unit: String,
    #[into]
    #[default]
    href: String,
    child: Child<'_>,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class=(KPI)>
            <span class="flex items-center gap-2 text-[12.5px] font-medium leading-4 text-fg-2">
                if !state.is_empty() { dot(state: state) }
                if href.is_empty() {
                    (label)
                } else {
                    <a href=(href) class="after:absolute after:inset-0 after:content-['']">(label)</a>
                }
            </span>
            <span class="text-2xl font-semibold leading-8 tracking-[-0.02em] tabular-nums">
                (value)
                if !unit.is_empty() {
                    <small class="ml-1 text-[13px] font-medium tracking-normal text-fg-3">(unit)</small>
                }
            </span>
            (child)
        </div>
    })
}

/// The points of a sparkline in a 120 by 20 box, oldest first, scaled to
/// `max` or the largest value if that is more.
fn spark_points(values: &[f64], max: f64) -> Vec<(f64, f64)> {
    let max = values.iter().copied().fold(max, f64::max);
    let step = if values.len() > 1 {
        120.0 / (values.len() - 1) as f64
    } else {
        0.0
    };
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = if values.len() > 1 {
                i as f64 * step
            } else {
                120.0
            };
            let share = if max > 0.0 { v.max(0.0) / max } else { 0.0 };
            (x, 17.0 - 14.0 * share)
        })
        .collect()
}

/// A line of `values`, oldest first, with a dot on the latest. `max` sets
/// the scale, such as a daily limit, so small values stay low.
#[component]
pub async fn sparkline(
    values: Vec<f64>,
    #[into] label: String,
    #[default] max: f64,
) -> topcoat::Result<impl View> {
    let points = spark_points(&values, max);
    let line = points
        .iter()
        .map(|(x, y)| format!("{x:.1},{y:.1}"))
        .collect::<Vec<_>>()
        .join(" ");
    let last = points.last().copied();
    Ok(view! {
        <svg class="mt-0.5 block h-5 w-full" viewBox="0 0 120 20" preserveAspectRatio="none" role="img" aria-label=(label.clone())>
            <title>(label)</title>
            <polyline points=(line) fill="none" stroke="var(--fg-3)" stroke-width="1.5" vector-effect="non-scaling-stroke"></polyline>
            if let Some((x, y)) = last {
                <circle cx=(format!("{x:.1}")) cy=(format!("{y:.1}")) r="2.2" fill="var(--fg)"></circle>
            }
        </svg>
    })
}

/// A group's header inside a `LIST`: its glyph, title and count, and an
/// optional link on the right.
#[component]
pub async fn group_head(
    state: &'static str,
    title: &'static str,
    count: usize,
    #[into]
    #[default]
    href: String,
    #[into]
    #[default]
    link: String,
    /// Lets a link elsewhere lead to this group.
    #[into]
    #[default]
    id: String,
) -> topcoat::Result<impl View> {
    let id = (!id.is_empty()).then_some(id);
    Ok(view! {
        <div id=(id) class="scroll-mt-16 flex h-[38px] items-center gap-2.5 border-t border-line bg-bg-2 px-4 text-[13px] font-medium leading-none first:border-t-0">
            state_icon(state: state)
            (title)
            <span class="font-normal text-fg-3 tabular-nums">(count)</span>
            if !href.is_empty() {
                <a href=(href) class="ml-auto text-[12.5px] font-normal text-fg-3 hover:text-fg">(link)</a>
            }
        </div>
    })
}

/// A ticket's row inside a `LIST`: glyph, id, the title as a link with a
/// `detail` line under it, and `child` on the right (avatars, `FIGURE`s,
/// buttons). `compact` rows are one line and 44px tall.
#[component]
pub async fn row(
    state: &'static str,
    #[into] id: String,
    #[into] href: String,
    #[into] title: String,
    #[into]
    #[default]
    detail: Child<'_>,
    #[default] compact: bool,
    child: Child<'_>,
) -> topcoat::Result<impl View> {
    let height = if compact { "min-h-11" } else { "min-h-14" };
    Ok(view! {
        <div class=(format!("grid grid-cols-[16px_64px_minmax(0,1fr)_auto] items-center gap-x-3 border-t border-line px-4 py-[9px] transition-colors first:border-t-0 hover:bg-hover max-[760px]:grid-cols-[16px_minmax(0,1fr)] max-[760px]:gap-y-2 {height}"))>
            state_icon(state: state)
            <span class="font-mono text-[12.5px] font-medium leading-none text-fg-3 max-[760px]:hidden">(id)</span>
            <div class="flex min-w-0 flex-col gap-[3px]">
                <a href=(href) class="truncate font-medium text-fg decoration-line-3 underline-offset-3 hover:underline">(title)</a>
                <span class="block min-w-0 truncate text-[12.5px] leading-5 text-fg-3 empty:hidden max-[760px]:whitespace-normal">(detail)</span>
            </div>
            <div class="flex items-center gap-4 text-[12.5px] text-fg-2 tabular-nums max-[760px]:col-start-2 max-[760px]:flex-wrap max-[760px]:justify-start max-[760px]:gap-2.5">
                (child)
            </div>
        </div>
    })
}

/// A ticket's glyph and id, for a table's first cell.
#[component]
pub async fn id_cell(state: &'static str, #[into] id: String) -> topcoat::Result<impl View> {
    Ok(view! {
        <span class="flex items-center gap-2">
            state_icon(state: state)
            <span class="font-mono text-[12.5px] text-fg-3">(id)</span>
        </span>
    })
}

/// A section heading: the title, a quiet count, and an optional link on the
/// right. Pass an empty `href` for no link.
#[component]
pub async fn section_head(
    title: &'static str,
    count: Option<u64>,
    href: String,
    label: String,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="mb-2.5 flex items-baseline gap-2">
            <h2 class=(SECTION_TITLE)>(title)</h2>
            if let Some(count) = count {
                <span class="text-fg-3 tabular-nums">(count)</span>
            }
            if !href.is_empty() {
                <a href=(href) class="ml-auto inline-flex items-center gap-1 self-center text-[13px] text-fg-3 hover:text-fg">
                    (label)
                    icon(name: "arrow-right", small: true)
                </a>
            }
        </div>
    })
}

/// A page's right rail, sticky beside the main column on a wide screen;
/// put `prop`s and sections in it.
#[component]
pub async fn rail(child: Child<'_>) -> topcoat::Result<impl View> {
    Ok(view! {
        <aside class="sticky top-20 flex flex-col gap-[22px] text-[13px] max-[860px]:static">
            (child)
        </aside>
    })
}

/// One property: a quiet name, then the value.
#[component]
pub async fn prop(label: &'static str, child: Child<'_>) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="grid min-h-[30px] grid-cols-[84px_minmax(0,1fr)] items-center gap-x-2.5">
            <dt class="text-fg-3">(label)</dt>
            <dd class="m-0 flex min-w-0 items-center gap-[7px] text-fg">(child)</dd>
        </div>
    })
}

/// The amber card of something waiting for the owner: `head` says what it
/// wants, `ago` how long it has waited, and `child` holds the body (`CMD`,
/// what the agent said, `WHY`, then `ASK_ACTIONS`).
#[component]
pub async fn ask_frame(
    #[into] head: Child<'_>,
    #[into] ago: String,
    child: Child<'_>,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <section class="overflow-hidden rounded-xl border border-attn-line bg-bg" aria-label="Waiting for you">
            <div class="flex items-center gap-2.5 bg-attn-bg px-4 py-[11px] font-medium text-attn-fg">
                icon(name: "shield-alert")
                <span class="min-w-0">(head)</span>
                <span class="ml-auto shrink-0 text-[12.5px] font-normal opacity-85">(ago)</span>
            </div>
            <div class="flex flex-col gap-3.5 p-4">(child)</div>
        </section>
    })
}

/// A "more" button and the popover menu it opens; put `MENU_ITEM` buttons
/// in it. `id` must be unique on the page.
#[component]
pub async fn menu(
    id: &'static str,
    label: &'static str,
    child: Child<'_>,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <button type="button" class=(ICON_BUTTON_SM) popovertarget=(id) aria-label=(label) style=(format!("anchor-name:--{id}"))>
            icon(name: "ellipsis")
        </button>
        <div popover="" id=(id) class="menu" style=(format!("position-anchor:--{id}"))>(child)</div>
    })
}

/// A popover dialog with a title and a close button. Open it with a button
/// that has `popovertarget` set to `id`; put `DIALOG_BODY` and `DIALOG_FOOT`
/// blocks (or a form holding them) in it.
#[component]
pub async fn dialog(
    id: &'static str,
    title: &'static str,
    child: Child<'_>,
) -> topcoat::Result<impl View> {
    Ok(view! {
        <div popover="" id=(id) role="dialog" aria-label=(title) class="m-auto w-[min(560px,calc(100vw-32px))] rounded-[14px] border-0 bg-bg p-0 text-fg [box-shadow:var(--pop)] backdrop:bg-black/45">
            <div class="flex items-center gap-2.5 px-[18px] pb-1.5 pt-4">
                <h2 class="text-[16px] font-semibold leading-6">(title)</h2>
                <button type="button" class=(format!("ml-auto {ICON_BUTTON_SM}")) popovertarget=(id) popovertargetaction="hide" aria-label="Close">
                    icon(name: "x")
                </button>
            </div>
            (child)
        </div>
    })
}

/// A page's actions at the right of the shell's top bar. Pages render it
/// anywhere in their content, so live patches keep working; it is fixed to
/// the top bar on a wide screen and placed over it on a phone. Wrap button
/// words in `LABEL` so a phone shows only the icons.
#[component]
pub async fn top_actions(child: Child<'_>) -> topcoat::Result<impl View> {
    Ok(view! {
        <div class="fixed right-8 top-0 z-10 flex h-14 max-w-[60%] items-center gap-2 overflow-x-auto [scrollbar-width:none] max-[760px]:absolute max-[760px]:right-4 max-[760px]:h-[52px] max-[760px]:max-w-[55%]">
            (child)
        </div>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(id: &str, state: serde_json::Value, goal: &str) -> Ticket {
        serde_json::from_value(serde_json::json!({
            "id": id, "project": "game", "title": id, "goal": goal, "hire": "dev",
            "state": state, "attempts": 0, "base_branch": "main", "created_at": 0,
        }))
        .unwrap()
    }

    #[test]
    fn queued_tickets_wait_for_their_goal_a_slot_or_the_owner() {
        use serde_json::json;
        let a = ticket("G-1", json!("ready"), "Menus");
        let b = ticket("G-2", json!("queued"), "Menus");
        let c = ticket("G-3", json!("queued"), "");
        let d = ticket("G-4", json!("queued"), "Saves");
        let e = ticket("G-5", json!("queued"), "Saves");
        let all = vec![a, b.clone(), c.clone(), d, e.clone()];
        let status = |broke: bool, slots: u64, closed: bool| crate::client::Status {
            out_of_budget: broke,
            max_runs: slots,
            closed,
            ..Default::default()
        };
        let wait = |t: &Ticket, broke: bool, slots: u64| {
            Wait::of(t, &all, &status(broke, slots, false)).short()
        };
        assert_eq!(wait(&b, false, 2), "After G-1");
        assert_eq!(wait(&c, false, 2), "Waiting for a free desk");
        assert_eq!(wait(&c, true, 2), "Waiting for budget");
        assert_eq!(wait(&e, false, 2), "After G-4");
        assert_eq!(wait(&c, false, 0), "Paused");
        assert_eq!(
            Wait::of(&c, &all, &status(false, 0, false)).sentence(),
            "Paused: max_runs is 0, so nothing new starts."
        );
        // A closed office starts nothing, whatever else the ticket waits for.
        assert_eq!(
            Wait::of(&b, &all, &status(false, 2, true)).short(),
            "Office closed"
        );
    }

    #[test]
    fn only_holds_that_wait_on_the_owner_need_the_owner() {
        use serde_json::json;
        let needs = |state| needs_owner(&ticket("G-1", state, "").state);
        assert!(needs(json!({"asking": {"ask": "a1"}})));
        assert!(needs(json!({"held": {"reason": "paused"}})));
        assert!(!needs(json!({"held": {"reason": "budget"}})));
        assert!(!needs(json!({"held": {"reason": "quota"}})));
        assert!(!needs(json!("working")));
    }

    #[test]
    fn a_hold_that_ends_on_its_own_is_drawn_grey() {
        use serde_json::json;
        let shown_as = |state| shown(&ticket("G-1", state, "").state);
        assert_eq!(shown_as(json!({"held": {"reason": "budget"}})), "on hold");
        assert_eq!(shown_as(json!({"held": {"reason": "quota"}})), "on hold");
        assert_eq!(shown_as(json!({"held": {"reason": "paused"}})), "held");
        assert_eq!(look("on hold").tone, "neutral");
        assert_eq!(look("held").tone, "attn");
    }

    #[test]
    fn a_goal_reads_the_same_everywhere() {
        use serde_json::json;
        let plan = |state| ticket("P-1", state, "Menus");
        let landed = ticket("G-1", json!("landed"), "Menus");
        let working = ticket("G-2", json!("working"), "Menus");
        let broke = ticket("G-3", json!({"held": {"reason": "budget"}}), "Menus");
        let paused = ticket("G-4", json!({"held": {"reason": "paused"}}), "Menus");
        let done = plan(json!("landed"));
        assert_eq!(
            goal_state(&done, std::slice::from_ref(&landed), false),
            "done"
        );
        assert_eq!(
            goal_state(&done, std::slice::from_ref(&landed), true),
            "merged"
        );
        assert_eq!(goal_state(&done, &[landed, working], false), "in progress");
        assert_eq!(goal_state(&done, &[broke], false), "in progress");
        assert_eq!(goal_state(&done, &[paused], false), "needs attention");
        // A plan that landed having filed nothing is not done.
        assert_eq!(goal_state(&done, &[], false), "needs attention");
        assert_eq!(goal_state(&plan(json!("working")), &[], false), "planning");
        assert_eq!(goal_state(&plan(json!("queued")), &[], false), "queued");
    }

    #[test]
    fn limits_read_in_whole_dollars_when_they_are() {
        assert_eq!(limit(10.0), "$10");
        assert_eq!(limit(7.5), "$7.50");
    }
    use topcoat::context::Cx;

    async fn render(view: impl View) -> String {
        html(&Cx::default(), view).await.unwrap()
    }

    #[test]
    fn small_amounts_keep_their_digits() {
        assert_eq!(usd(0.0019), "$0.0019");
        assert_eq!(usd(0.0), "$0.00");
        assert_eq!(usd(1.5), "$1.50");
    }

    #[test]
    fn zero_never_prints_a_minus_sign() {
        assert_eq!(usd(-0.0), "$0.00");
        assert_eq!(usd(-0.000_001), "$0.00");
        assert_eq!(usd(0.000_001), "$0.00");
        assert_eq!(usd(-0.001), "$-0.0010");
    }

    #[test]
    fn elapsed_time_reads_short() {
        assert_eq!(elapsed(45_000), "45s");
        assert_eq!(elapsed(12 * 60_000 + 59_000), "12m");
        assert_eq!(elapsed(3_900_000), "1h 05m");
    }

    #[test]
    fn fills_are_clamped_and_safe_for_zero() {
        assert_eq!(percent(3.0, 5.0), 60.0);
        assert_eq!(percent(0.0, 0.0), 0.0);
        assert_eq!(percent(9.0, 5.0), 100.0);
        assert_eq!(percent(-1.0, 5.0), 0.0);
    }

    #[test]
    fn hires_get_initials_and_a_steady_hue() {
        assert_eq!(initials("engineer"), "EN");
        assert_eq!(initials("design-critic"), "DC");
        assert_eq!(initials("x"), "X");
        assert_eq!(initials(""), "?");
        assert_eq!(hue("engineer"), hue("engineer"));
        let hues: std::collections::BTreeSet<_> = [
            "dev", "artist", "producer", "composer", "tester", "reviewer",
        ]
        .iter()
        .map(|h| hue(h))
        .collect();
        assert!(hues.len() > 2, "{hues:?}");
    }

    #[test]
    fn running_states_have_a_step() {
        assert_eq!(run_step("working"), Some(0));
        assert_eq!(run_step("checking"), Some(1));
        assert_eq!(run_step("reviewing"), Some(2));
        assert_eq!(run_step("queued"), None);
    }

    #[test]
    fn sparklines_fit_their_box() {
        let points = spark_points(&[0.0, 2.0, 4.0], 0.0);
        assert_eq!(points, [(0.0, 17.0), (60.0, 10.0), (120.0, 3.0)]);
        assert_eq!(spark_points(&[0.0], 0.0), [(120.0, 17.0)]);
        assert!(spark_points(&[], 0.0).is_empty());
        // Against a limit, small values stay low.
        let points = spark_points(&[0.0, 1.0], 4.0);
        assert_eq!(points, [(0.0, 17.0), (120.0, 13.5)]);
    }

    #[tokio::test]
    async fn states_get_their_glyph_and_colour() {
        let cx = Cx::default();
        let failed = render(view! { cx => state_pill(state: "failed") }).await;
        assert!(
            failed.contains("bg-bad-bg")
                && failed.contains("#s-failed")
                && failed.contains("Failed"),
            "{failed}"
        );
        let working = render(view! { cx => state_icon(state: "checking") }).await;
        assert!(
            working.contains(r#"class="prog""#) && working.contains("--p:50"),
            "{working}"
        );
    }

    #[tokio::test]
    async fn icons_use_the_sprite() {
        let cx = Cx::default();
        let html = render(view! { cx => icon(name: "plus", small: true) }).await;
        assert!(html.contains(r##"<use href="#i-plus">"##), "{html}");
        assert!(html.contains(r#"class="i sm""#), "{html}");
    }

    #[tokio::test]
    async fn meters_fill_to_their_share() {
        let cx = Cx::default();
        let html = render(view! { cx => meter(value: 1.0, max: 4.0, tone: "ok") }).await;
        assert!(
            html.contains("width:25.0%") && html.contains("bg-ok"),
            "{html}"
        );
        let over = render(view! { cx => meter(value: 9.0, max: 5.0) }).await;
        assert!(over.contains(r#"aria-valuenow="100""#), "{over}");
    }

    #[tokio::test]
    async fn steps_mark_done_and_current() {
        let cx = Cx::default();
        let html = render(view! { cx => steps(step: 1, attempt: 2) }).await;
        assert!(
            html.contains(r#"<i class="done"></i><i class="now"></i>"#),
            "{html}"
        );
        assert!(
            html.contains("Checks") && html.contains(" · try 2 of 3"),
            "{html}"
        );
    }

    #[tokio::test]
    async fn rows_link_their_title_and_hold_their_parts() {
        let cx = Cx::default();
        let html = render(view! { cx =>
            row(state: "asking", id: "GAME-3", href: "/tickets/GAME-3", title: "Waves <now>",
                detail: view! { "Wants to run " <span class=(CODE)>"cargo run"</span> },
                <button>"Allow"</button>)
        })
        .await;
        assert!(html.contains(r#"href="/tickets/GAME-3""#), "{html}");
        assert!(html.contains("Waves &lt;now&gt;"), "{html}");
        assert!(
            html.contains(">cargo run</span>") && html.contains(">Allow</button>"),
            "{html}"
        );
        assert!(
            html.contains("#s-attn") && html.contains("min-h-14"),
            "{html}"
        );
    }

    #[tokio::test]
    async fn a_kpi_is_a_link_only_with_an_href() {
        let cx = Cx::default();
        let plain = render(view! { cx =>
            kpi(label: "Running", state: "working", value: "3", <span class=(KPI_SUB)>"2 working"</span>)
        })
        .await;
        assert!(
            !plain.contains("<a ") && plain.contains("2 working"),
            "{plain}"
        );
        let link = render(view! { cx =>
            kpi(label: "Spend", value: "$1.00", unit: "of $10", href: "/budget",
                sparkline(values: vec![1.0, 2.0], label: "Spend by day"))
        })
        .await;
        assert!(
            link.contains(r#"href="/budget""#) && link.contains("of $10"),
            "{link}"
        );
        assert!(
            link.contains("<polyline") && link.contains("<circle"),
            "{link}"
        );
    }

    #[tokio::test]
    async fn avatars_show_initials_in_a_hue() {
        let cx = Cx::default();
        let html = render(view! { cx => who(hire: "engineer") }).await;
        assert!(html.contains(">EN<") && html.contains("engineer"), "{html}");
        assert!(html.contains(&format!("av {}", hue("engineer"))), "{html}");
        let staff = render(view! { cx => who(hire: "approver", staff: true) }).await;
        assert!(staff.contains(r#"class="av ""#), "{staff}");
    }

    #[tokio::test]
    async fn menus_and_dialogs_are_popovers() {
        let cx = Cx::default();
        let html = render(view! { cx =>
            menu(id: "more", label: "More actions", <button class=(MENU_ITEM)>"Pause"</button>)
            dialog(id: "new-ticket", title: "New ticket", <div class=(DIALOG_BODY)>"Form"</div>)
        })
        .await;
        assert!(html.contains(r#"popovertarget="more""#), "{html}");
        assert!(
            html.contains(r#"id="more""#) && html.contains("popover"),
            "{html}"
        );
        assert!(html.contains(r#"popovertargetaction="hide""#), "{html}");
        assert!(html.contains(">New ticket</h2>"), "{html}");
    }

    #[tokio::test]
    async fn section_head_shows_count_and_link_only_when_given() {
        let cx = Cx::default();
        let full = render(view! { cx =>
            section_head(title: "Running", count: Some(3), href: "/goals".to_string(), label: "All".to_string())
        })
        .await;
        assert!(full.contains("Running") && full.contains(">3<"), "{full}");
        assert!(full.contains(r#"href="/goals""#), "{full}");
        let bare = render(view! { cx =>
            section_head(title: "Running", count: None, href: String::new(), label: String::new())
        })
        .await;
        assert!(!bare.contains("<a "), "{bare}");
    }

    #[tokio::test]
    async fn top_actions_wrap_their_content() {
        let cx = Cx::default();
        let html = render(view! { cx =>
            top_actions(<a href="/x">"New"</a>)
        })
        .await;
        assert!(html.contains("fixed") && html.contains(">New<"), "{html}");
    }
}
