use studiod::api::{Autonomy, ManagerAction, OperationsSettings, RuntimeMode};
use topcoat::view::{View, component, view};

use super::hq;
use crate::ui;

#[component]
pub(super) async fn action_row(action: ManagerAction) -> topcoat::Result<impl View> {
    let decision = format!(
        "@post('/operations/{}/decision', {{contentType: 'form'}})",
        action.id
    );
    let task = action.act.task().map(|task| {
        let id = task.to_string();
        view! { <a href=(format!("/tickets/{id}")) class="hover:underline">(id)</a> }
    });
    Ok(view! {
        hq::card(tone: "ask", title: action.title.clone(), meta: view! {
            <span class="block">(action.act.label()) " · proposed by " (action.lead.clone())</span>
            if !action.reason.trim().is_empty() {
                <span class="block">(action.reason.clone())</span>
            }
            if let Some(task) = task { <span class="block">(task)</span> }
        },
            <form class="contents" data-on:submit=(decision.clone())>
                <button type="submit" name="decision" value="approve" class=(ui::PRIMARY_SM)>"Approve"</button>
                <button type="submit" name="decision" value="reject" class=(ui::BUTTON_SM)>"Reject"</button>
            </form>
        )
    })
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
                    <b class="block text-[14px] font-semibold">(if settings.mode == RuntimeMode::Simulation { "Simulation · zero model cost" } else { "Configured runtimes" })</b>
                    <span class="block text-[12.5px] text-fg-3">"Applies to new runs; agents already running finish their current run."</span>
                </div>
                <form data-on:submit="@post('/operations/settings', {contentType: 'form'})">
                    <input type="hidden" name="mode" value=(if settings.mode == RuntimeMode::Simulation { "live" } else { "simulation" })>
                    <button type="submit" class=(ui::BUTTON)>
                        (if settings.mode == RuntimeMode::Simulation { "Use configured runtimes" } else { "Switch to simulation" })
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
                            <button type="submit" class=(format!("w-full {} {}", if level == autonomy { "border-primary bg-bg-4" } else { "border-line-2 bg-bg-2" }, "rounded-xl border px-3 py-2.5 text-left hover:bg-bg-3"))>
                                <b class="block text-[13px] font-semibold">(autonomy_title(level))</b>
                                <span class="block text-[12px] leading-4 text-fg-3">(autonomy_note(level))</span>
                            </button>
                        </form>
                    }
                </div>
                <p class="m-0 text-[12.5px] text-fg-3">"Shipping, publishing and cancellation always need you. Manager reviews have their own spend cap and audit trail."</p>
            </div>
        </div>
    })
}

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
        Autonomy::Supervised => "Every action needs approval.",
        Autonomy::Balanced => "Feedback can go ahead.",
        Autonomy::Autonomous => "Ordinary management can go ahead.",
    }
}
