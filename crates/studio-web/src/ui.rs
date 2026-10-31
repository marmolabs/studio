//! Pieces every page uses.

use studio_config::draft::Draft;
use studio_core::ticket::{FailureClass, HeldReason, TicketState};
use topcoat::{
    context::Cx,
    datastar::{ElementPatchMode, PatchElements},
    view::{View, ViewExt, component, view},
};

// Text tones. Body text is the page default (zinc-900 / zinc-100); SECONDARY
// is for supporting text and MUTED for hints and meta only.
pub const PAGE_TITLE: &str = "text-lg font-semibold tracking-tight";
pub const SECTION_TITLE: &str = "text-sm font-semibold text-zinc-900 dark:text-zinc-100";
pub const SECONDARY: &str = "text-zinc-700 dark:text-zinc-300";
pub const MUTED: &str = "text-zinc-600 dark:text-zinc-400";
/// A bordered surface for a group of related content.
pub const CARD: &str =
    "rounded-lg border border-zinc-300 bg-white dark:border-zinc-700 dark:bg-zinc-900";

// Buttons share size, focus ring and wrapping: a long label wraps inside its
// button instead of widening the page.
pub const BUTTON: &str = "inline-flex max-w-full items-center justify-center gap-1 rounded-md border \
    border-zinc-300 bg-white px-3 py-1.5 text-[13px] font-medium text-zinc-900 [overflow-wrap:anywhere] \
    hover:bg-zinc-100 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-500 \
    dark:border-zinc-600 dark:bg-zinc-800 dark:text-zinc-100 dark:hover:bg-zinc-700";
pub const PRIMARY: &str = "inline-flex max-w-full items-center justify-center gap-1 rounded-md border \
    border-transparent bg-indigo-600 px-3 py-1.5 text-[13px] font-medium text-white [overflow-wrap:anywhere] \
    hover:bg-indigo-700 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-indigo-500 \
    dark:bg-indigo-500 dark:hover:bg-indigo-400";
/// Outlined, so it is quieter than `PRIMARY`.
pub const DANGER: &str = "inline-flex max-w-full items-center justify-center gap-1 rounded-md border \
    border-rose-300 bg-white px-3 py-1.5 text-[13px] font-medium text-rose-700 [overflow-wrap:anywhere] \
    hover:bg-rose-50 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-rose-500 \
    dark:border-rose-800 dark:bg-transparent dark:text-rose-300 dark:hover:bg-rose-950";
/// Text only, for low-priority actions such as Cancel.
pub const QUIET: &str = "inline-flex max-w-full items-center justify-center gap-1 rounded-md border \
    border-transparent px-3 py-1.5 text-[13px] font-medium text-zinc-700 [overflow-wrap:anywhere] \
    hover:bg-zinc-100 hover:text-zinc-900 focus-visible:outline-2 focus-visible:outline-offset-2 \
    focus-visible:outline-indigo-500 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-zinc-100";
pub const INPUT: &str = "w-full rounded-md border border-zinc-300 bg-white px-2 py-1 text-[13px] \
    dark:border-zinc-600 dark:bg-zinc-900 focus:outline-2 focus:outline-indigo-500";

/// Renders a view to HTML, for patches sent over a stream or a POST reply.
pub async fn html(cx: &Cx, view: impl View) -> topcoat::Result<String> {
    Ok(view.single().await?.render(cx))
}

/// Puts a short message in the page's `#notice` slot.
pub async fn notice(cx: &Cx, message: String, error: bool) -> topcoat::Result<PatchElements> {
    let text = html(
        cx,
        view! { cx =>
            if error {
                <p class="whitespace-pre-wrap text-rose-700 dark:text-rose-400">(message)</p>
            } else {
                <p class="text-zinc-600 dark:text-zinc-400">(message)</p>
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
            <p class="text-zinc-600 dark:text-zinc-400">
                (message) " "
                <a href=(href) class="underline hover:text-zinc-900 dark:hover:text-zinc-100">(label)</a>
            </p>
        },
    )
    .await?;
    Ok(PatchElements::new(text)
        .selector("#notice")
        .mode(ElementPatchMode::Inner))
}

pub fn usd(amount: f64) -> String {
    if amount != 0.0 && amount.abs() < 0.01 {
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

/// "~12m" for a finish time that is still ahead; `None` when unknown.
pub fn eta(eta_ms: Option<u64>) -> Option<String> {
    let now = now_ms();
    let minutes = eta_ms?.saturating_sub(now).div_ceil(60_000).max(1);
    Some(if minutes >= 120 {
        format!("~{}h", minutes.div_ceil(60))
    } else {
        format!("~{minutes}m")
    })
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

#[component]
pub async fn state_pill(state: &'static str) -> topcoat::Result<impl View> {
    // Literal class strings so Tailwind finds them.
    let tone = match state {
        "claimed" | "working" | "checking" | "reviewing" | "planning" | "in progress" => {
            "bg-sky-100 text-sky-900 ring-sky-300 dark:bg-sky-950 dark:text-sky-200 dark:ring-sky-800"
        }
        "asking" | "held" | "needs attention" => {
            "bg-amber-100 text-amber-900 ring-amber-300 dark:bg-amber-950 dark:text-amber-200 dark:ring-amber-800"
        }
        "ready" | "landed" | "done" => {
            "bg-emerald-100 text-emerald-900 ring-emerald-300 dark:bg-emerald-950 dark:text-emerald-200 dark:ring-emerald-800"
        }
        "failed" => {
            "bg-rose-100 text-rose-900 ring-rose-300 dark:bg-rose-950 dark:text-rose-200 dark:ring-rose-800"
        }
        _ => {
            "bg-zinc-100 text-zinc-800 ring-zinc-300 dark:bg-zinc-800 dark:text-zinc-200 dark:ring-zinc-600"
        }
    };
    Ok(view! {
        <span class=(format!("inline-flex shrink-0 items-center rounded px-1.5 py-px text-[11px] font-medium ring-1 ring-inset {tone}"))>
            (state)
        </span>
    })
}

/// A number that wants attention, such as the pending asks.
#[component]
pub async fn count_badge(count: u64) -> topcoat::Result<impl View> {
    Ok(view! {
        <span class="inline-flex min-w-5 shrink-0 items-center justify-center rounded-full bg-amber-500 px-1.5 text-[11px] font-semibold leading-5 tabular-nums text-zinc-950">
            (count)
        </span>
    })
}

/// A small neutral label for a fact such as a project, hire or base branch.
#[component]
pub async fn badge(text: String) -> topcoat::Result<impl View> {
    Ok(view! {
        <span title=(text.clone()) class="inline-block max-w-full truncate rounded bg-zinc-100 px-1.5 py-px text-[11px] font-medium text-zinc-700 ring-1 ring-inset ring-zinc-300 dark:bg-zinc-800 dark:text-zinc-300 dark:ring-zinc-600">
            (text)
        </span>
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_amounts_keep_their_digits() {
        assert_eq!(usd(0.0019), "$0.0019");
        assert_eq!(usd(0.0), "$0.00");
        assert_eq!(usd(1.5), "$1.50");
    }

    #[test]
    fn elapsed_time_reads_short() {
        assert_eq!(elapsed(45_000), "45s");
        assert_eq!(elapsed(12 * 60_000 + 59_000), "12m");
        assert_eq!(elapsed(3_900_000), "1h 05m");
    }
}
