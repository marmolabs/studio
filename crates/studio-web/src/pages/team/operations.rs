use studiod::api::{
    Autonomy, ManagerAct, ManagerAction, OperationState, OperationsSettings, RuntimeMode,
};
use topcoat::view::{View, component, view};

use super::hq;
use crate::ui;

#[component]
pub(super) async fn action_row(action: ManagerAction, by: String) -> topcoat::Result<impl View> {
    let decision = format!(
        "@post('/operations/{}/decision', {{contentType: 'form'}})",
        action.id
    );
    let detail = detail(&action.act);
    let waiting = waiting_reason(&action);
    let task = action.act.task().map(|task| {
        let id = task.to_string();
        view! { <a href=(format!("/tickets/{id}")) class="hover:underline">(id)</a> }
    });
    Ok(view! {
        hq::card(tone: "ask", title: action.title.clone(), meta: view! {
            <span class="block">(action.act.label()) " · proposed by " (by)</span>
            if let Some(detail) = detail {
                <span class="block whitespace-pre-wrap text-fg-2 [overflow-wrap:anywhere]">(detail)</span>
            }
            if !action.reason.trim().is_empty() {
                <span class="block">(action.reason.clone())</span>
            }
            if let Some(waiting) = waiting { <span class="block">(waiting)</span> }
            if let Some(task) = task { <span class="block">"Task " (task)</span> }
        },
            <form class="contents" data-on:submit=(decision.clone())>
                <button type="submit" name="decision" value="approve" class=(ui::PRIMARY_SM)>"Approve"</button>
                <button type="submit" name="decision" value="reject" class=(ui::BUTTON_SM)>"Reject"</button>
            </form>
        )
    })
}

/// Why a pending action waits for the owner, when the engine says.
pub(super) fn waiting_reason(action: &ManagerAction) -> Option<String> {
    let why = action.result.trim();
    (action.state == OperationState::Pending && !why.is_empty())
        .then(|| format!("Waiting because: {why}"))
}

/// What an action would say or change, in words, so the owner knows what
/// approving it does.
pub(super) fn detail(act: &ManagerAct) -> Option<String> {
    let said = |text: &str| Some(text.trim().to_string()).filter(|text| !text.is_empty());
    match act {
        ManagerAct::StartGoal { detail, .. } | ManagerAct::CreateTask { detail, .. } => {
            said(detail)
        }
        ManagerAct::SendBack { note, .. } => {
            said(note).map(|note| format!("Sent back with: {note}"))
        }
        ManagerAct::Tell { text, .. } => said(text).map(|text| format!("Message: {text}")),
        ManagerAct::SetPriority { priority, .. } => Some(format!("New priority: {priority}")),
        ManagerAct::SetDependencies { depends_on, .. } if depends_on.is_empty() => {
            Some("It would no longer wait for any other task.".to_string())
        }
        ManagerAct::SetDependencies { depends_on, .. } => {
            Some(format!("It would wait for {}.", depends_on.join(", ")))
        }
        ManagerAct::Pause { .. } | ManagerAct::Resume { .. } | ManagerAct::Cancel { .. } => None,
    }
}

#[component]
pub(super) async fn review_button(project: String, reviewing: bool) -> topcoat::Result<impl View> {
    Ok(view! {
        <form data-on:submit="@post('/operations/review', {contentType: 'form'})">
            <input type="hidden" name="project" value=(project)>
            <button type="submit" class=(ui::BUTTON_SM) disabled=(reviewing)>
                (if reviewing { "Reviewing…" } else { "Run manager review" })
            </button>
        </form>
    })
}

#[component]
pub(super) async fn controls(settings: OperationsSettings) -> topcoat::Result<impl View> {
    let autonomy = settings.autonomy;
    Ok(view! {
        <div class="flex flex-col gap-4">
            <div class="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-line-2 bg-bg-2 px-3 py-2.5">
                <div>
                    <b class="block text-[14px] font-semibold">(if settings.mode == RuntimeMode::Simulation { "Simulation is on · no model cost" } else { "Real models are in use" })</b>
                    <span class="block text-[12.5px] text-fg-3">(SIMULATION_NOTE)</span>
                </div>
                <form data-on:submit="@post('/operations/settings', {contentType: 'form'})">
                    <input type="hidden" name="mode" value=(if settings.mode == RuntimeMode::Simulation { "live" } else { "simulation" })>
                    <button type="submit" class=(ui::BUTTON)>
                        (if settings.mode == RuntimeMode::Simulation { "Use real models" } else { "Switch to simulation" })
                    </button>
                </form>
            </div>
            <div class="flex flex-wrap items-center justify-between gap-3">
                <div>
                    <b class="block text-[14px] font-semibold">(if settings.autopilot { "Automatic reviews are on" } else { "Reviews are manual" })</b>
                    <span class="block text-[12.5px] text-fg-3">"Every " (settings.review_minutes) " minutes while the office is open"</span>
                </div>
                <form data-on:submit="@post('/operations/settings', {contentType: 'form'})">
                    <input type="hidden" name="autopilot" value=(if settings.autopilot { "false" } else { "true" })>
                    <button type="submit" class=(if settings.autopilot { ui::BUTTON } else { ui::PRIMARY })>
                        (if settings.autopilot { "Pause automatic reviews" } else { "Enable automatic reviews" })
                    </button>
                </form>
            </div>
            <form class="flex flex-wrap items-end gap-2" data-on:submit="@post('/operations/settings', {contentType: 'form'})">
                <label class=(ui::FIELD)>
                    <span class=(ui::FIELD_LABEL)>"Review every, minutes"</span>
                    <input name="review_minutes" type="number" min="5" max="1440" required="" value=(settings.review_minutes.to_string()) class=(ui::INPUT)>
                </label>
                <button type="submit" class=(ui::BUTTON)>"Save cadence"</button>
            </form>
            <div class="flex flex-col gap-2">
                <span class=(ui::FIELD_LABEL)>"Autonomy level"</span>
                <div class="grid grid-cols-3 gap-2 max-[560px]:grid-cols-1">
                    for level in [Autonomy::Supervised, Autonomy::Balanced, Autonomy::Autonomous] {
                        <form data-on:submit="@post('/operations/settings', {contentType: 'form'})">
                            <input type="hidden" name="autonomy" value=(autonomy_name(level))>
                            <button type="submit" aria-pressed=(if level == autonomy { "true" } else { "false" }) class=(format!("w-full {} {}", if level == autonomy { "border-primary bg-bg-4" } else { "border-line-2 bg-bg-2" }, "rounded-xl border px-3 py-2.5 text-left hover:bg-bg-3"))>
                                <b class="block text-[13px] font-semibold">(autonomy_title(level))</b>
                                <span class="block text-[12px] leading-4 text-fg-3">(autonomy_note(level))</span>
                            </button>
                        </form>
                    }
                </div>
                <p class="m-0 text-[12.5px] text-fg-3">"Shipping and publishing always need you. Manager reviews have their own spend cap and are all kept in the decision log."</p>
            </div>
        </div>
    })
}

/// What the simulation switch changes, and what it never does.
const SIMULATION_NOTE: &str = "Simulation affects only conversations, rooms, standups, setup and reviews, never coding work or approvals.";

fn autonomy_name(autonomy: Autonomy) -> &'static str {
    match autonomy {
        Autonomy::Supervised => "supervised",
        Autonomy::Balanced => "balanced",
        Autonomy::Autonomous => "autonomous",
    }
}

fn autonomy_title(autonomy: Autonomy) -> &'static str {
    match autonomy {
        Autonomy::Supervised => "Supervised",
        Autonomy::Balanced => "Balanced",
        Autonomy::Autonomous => "Autonomous",
    }
}

fn autonomy_note(autonomy: Autonomy) -> &'static str {
    match autonomy {
        Autonomy::Supervised => "Every action waits for you.",
        Autonomy::Balanced => "Feedback and messages to employees go ahead; everything else waits.",
        Autonomy::Autonomous => {
            "Task management goes ahead, and up to 3 new goals or tasks a day per project; cancelling still waits."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_action_says_what_approving_it_would_do() {
        let tell = ManagerAct::Tell {
            task: "S-1".into(),
            text: " Use small steps ".into(),
        };
        assert_eq!(detail(&tell).as_deref(), Some("Message: Use small steps"));
        let back = ManagerAct::SendBack {
            task: "S-1".into(),
            note: "Too tall on a phone".into(),
        };
        assert_eq!(
            detail(&back).as_deref(),
            Some("Sent back with: Too tall on a phone")
        );
        let priority = ManagerAct::SetPriority {
            task: "S-1".into(),
            priority: 80,
        };
        assert_eq!(detail(&priority).as_deref(), Some("New priority: 80"));
        let waits = ManagerAct::SetDependencies {
            task: "S-3".into(),
            depends_on: vec!["S-1".into(), "S-2".into()],
        };
        assert_eq!(
            detail(&waits).as_deref(),
            Some("It would wait for S-1, S-2.")
        );
        let free = ManagerAct::SetDependencies {
            task: "S-3".into(),
            depends_on: Vec::new(),
        };
        assert!(detail(&free).is_some_and(|words| words.contains("no longer wait")));
        let create = ManagerAct::CreateTask {
            title: "Total".into(),
            detail: "  ".into(),
            hire: None,
        };
        assert_eq!(detail(&create), None);
        assert_eq!(detail(&ManagerAct::Pause { task: "S-1".into() }), None);
        assert_eq!(detail(&ManagerAct::Cancel { task: "S-1".into() }), None);
    }

    #[test]
    fn a_pending_action_says_why_it_waits() {
        let mut action = ManagerAction {
            id: "A-1".into(),
            review: "R-1".into(),
            project: "shop".into(),
            lead: "lead".into(),
            title: "Add a cart".into(),
            reason: String::new(),
            act: ManagerAct::Pause { task: "S-1".into() },
            state: OperationState::Pending,
            at: 0,
            resolved_at: None,
            result: "over today's limit for automatic new work".into(),
            automatic: false,
        };
        assert_eq!(
            waiting_reason(&action).as_deref(),
            Some("Waiting because: over today's limit for automatic new work")
        );
        action.result = "  ".into();
        assert_eq!(waiting_reason(&action), None);
        action.result = "Done".into();
        action.state = OperationState::Executed;
        assert_eq!(waiting_reason(&action), None);
    }

    #[test]
    fn the_simulation_note_lists_what_is_simulated() {
        assert!(SIMULATION_NOTE.contains("standups, setup and reviews"));
    }

    #[test]
    fn each_autonomy_level_is_described_exactly() {
        assert_eq!(
            autonomy_note(Autonomy::Supervised),
            "Every action waits for you."
        );
        assert_eq!(
            autonomy_note(Autonomy::Balanced),
            "Feedback and messages to employees go ahead; everything else waits."
        );
        assert_eq!(
            autonomy_note(Autonomy::Autonomous),
            "Task management goes ahead, and up to 3 new goals or tasks a day per project; cancelling still waits."
        );
    }
}
