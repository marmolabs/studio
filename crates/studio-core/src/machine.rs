use crate::error::{ReplayError, TransitionError};
use crate::event::Event;
use crate::ticket::{FailureClass, HeldReason, Ticket, TicketKind, TicketState};

pub const MAX_ATTEMPTS: u32 = 3;

pub fn apply(ticket: &Ticket, event: &Event) -> Result<Ticket, TransitionError> {
    let next = match (&ticket.state, event) {
        (TicketState::Queued, Event::Claimed) => TicketState::Claimed,

        (TicketState::Claimed, Event::RunStarted { .. }) => TicketState::Working,
        (TicketState::Claimed, Event::Held { reason }) => {
            return held(ticket, reason.clone(), TicketState::Claimed);
        }
        (TicketState::Claimed, Event::Failed { class, detail }) => TicketState::Failed {
            class: class.clone(),
            detail: detail.clone(),
        },

        (TicketState::Working, Event::ChecksStarted) if ticket.kind == TicketKind::Work => {
            TicketState::Checking
        }
        // A finished plan has nothing to check, review or merge.
        (TicketState::Working, Event::Landed) if ticket.kind == TicketKind::Plan => {
            TicketState::Landed
        }
        (TicketState::Working, Event::AskOpened { ask }) => {
            TicketState::Asking { ask: ask.clone() }
        }
        // A review gate asks the owner and then picks up where it left off.
        (TicketState::Reviewing, Event::AskOpened { ask }) => {
            let mut next = ticket.clone();
            next.state = TicketState::Asking { ask: ask.clone() };
            next.held_from = Some(TicketState::Reviewing);
            return Ok(next);
        }
        (TicketState::Working, Event::Held { reason }) => {
            return held(ticket, reason.clone(), TicketState::Working);
        }
        (TicketState::Working, Event::Failed { class, detail }) => TicketState::Failed {
            class: class.clone(),
            detail: detail.clone(),
        },

        (
            TicketState::Asking { ask },
            Event::AskResolved {
                ask: resolved,
                approved,
            },
        ) => {
            if ask != resolved {
                return Err(err(ticket, event));
            }
            if *approved {
                ticket.held_from.clone().unwrap_or(TicketState::Working)
            } else {
                TicketState::Failed {
                    class: FailureClass::Denied,
                    detail: "denied".to_string(),
                }
            }
        }
        // An open ask can't survive a restart: hold the ticket and resume it as
        // working, so the agent asks again.
        (TicketState::Asking { .. }, Event::Held { reason }) => {
            let from = ticket.held_from.clone().unwrap_or(TicketState::Working);
            return held(ticket, reason.clone(), from);
        }

        (TicketState::Checking, Event::ChecksPassed) => TicketState::Reviewing,
        (TicketState::Checking, Event::ChecksFailed { detail }) => {
            return retry_or_fail(ticket, FailureClass::Checks, detail.clone());
        }
        (TicketState::Checking, Event::Held { reason }) => {
            return held(ticket, reason.clone(), TicketState::Checking);
        }
        (TicketState::Checking, Event::Failed { class, detail }) => TicketState::Failed {
            class: class.clone(),
            detail: detail.clone(),
        },

        (TicketState::Reviewing, Event::ReviewPassed) => TicketState::Ready,
        (TicketState::Reviewing, Event::ReviewFailed { detail }) => {
            return retry_or_fail(ticket, FailureClass::Review, detail.clone());
        }
        (TicketState::Reviewing, Event::Held { reason }) => {
            return held(ticket, reason.clone(), TicketState::Reviewing);
        }
        (TicketState::Reviewing, Event::Failed { class, detail }) => TicketState::Failed {
            class: class.clone(),
            detail: detail.clone(),
        },

        (TicketState::Ready, Event::Landed) => TicketState::Landed,
        (TicketState::Ready, Event::ChangesRequested { detail }) => {
            let mut next = ticket.clone();
            next.state = TicketState::Queued;
            next.feedback = Some(detail.clone());
            next.held_from = None;
            return Ok(next);
        }
        // Not the agent's failure, so it costs no attempt; the worktree tells
        // the agent what to settle, and older feedback was already acted on.
        (TicketState::Ready, Event::Conflicted { .. }) => {
            let mut next = ticket.clone();
            next.state = TicketState::Queued;
            next.feedback = None;
            next.held_from = None;
            return Ok(next);
        }

        (state, Event::Cancelled)
            if !matches!(state, TicketState::Landed | TicketState::Cancelled) =>
        {
            TicketState::Cancelled
        }

        (TicketState::Held { .. }, Event::Resumed) => match ticket.held_from.clone() {
            Some(state) => state,
            None => return Err(err(ticket, event)),
        },

        (TicketState::Failed { .. }, Event::Retried) => {
            return Ok(ticket_with(ticket, TicketState::Queued, 0));
        }

        _ => return Err(err(ticket, event)),
    };

    Ok(ticket_with(ticket, next, ticket.attempts))
}

pub fn replay(events: &[Event]) -> Result<Ticket, ReplayError> {
    let first = events.first().ok_or(ReplayError::Empty)?;
    let Event::Created {
        id,
        project,
        title,
        goal,
        hire,
        base_branch,
        created_at,
        kind,
        parent,
        detail,
        source,
    } = first
    else {
        return Err(ReplayError::MissingCreated);
    };

    let mut ticket = Ticket {
        id: id.clone(),
        project: project.clone(),
        title: title.clone(),
        goal: goal.clone(),
        hire: hire.clone(),
        state: TicketState::Queued,
        attempts: 0,
        base_branch: base_branch.clone(),
        created_at: *created_at,
        held_from: None,
        feedback: None,
        kind: *kind,
        parent: parent.clone(),
        detail: detail.clone(),
        source: source.clone(),
    };

    for event in &events[1..] {
        ticket = apply(&ticket, event)?;
    }

    Ok(ticket)
}

fn held(ticket: &Ticket, reason: HeldReason, from: TicketState) -> Result<Ticket, TransitionError> {
    let mut next = ticket.clone();
    next.state = TicketState::Held { reason };
    next.held_from = Some(from);
    Ok(next)
}

fn retry_or_fail(
    ticket: &Ticket,
    class: FailureClass,
    detail: String,
) -> Result<Ticket, TransitionError> {
    if ticket.attempts + 1 < MAX_ATTEMPTS {
        Ok(ticket_with(
            ticket,
            TicketState::Working,
            ticket.attempts + 1,
        ))
    } else {
        Ok(ticket_with(
            ticket,
            TicketState::Failed { class, detail },
            ticket.attempts,
        ))
    }
}

fn ticket_with(ticket: &Ticket, state: TicketState, attempts: u32) -> Ticket {
    let mut next = ticket.clone();
    next.state = state;
    next.attempts = attempts;
    next.held_from = None;
    next
}

fn err(ticket: &Ticket, event: &Event) -> TransitionError {
    TransitionError {
        from: Box::new(ticket.state.clone()),
        event: Box::new(event.clone()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::ids::{AskId, HireId, ProjectId, RunId, TicketId};

    fn tid() -> TicketId {
        TicketId::new("t-1")
    }

    fn pid() -> ProjectId {
        ProjectId::new("p-1")
    }

    fn hid() -> HireId {
        HireId::new("h-1")
    }

    fn rid() -> RunId {
        RunId::new("r-1")
    }

    fn aid() -> AskId {
        AskId::new("a-1")
    }

    fn created_event() -> Event {
        Event::Created {
            id: tid(),
            project: pid(),
            title: "title".into(),
            goal: "goal".into(),
            hire: hid(),
            base_branch: "main".into(),
            created_at: 1,
            kind: TicketKind::Work,
            parent: None,
            detail: String::new(),
            source: None,
        }
    }

    fn new_ticket() -> Ticket {
        replay(&[created_event()]).unwrap()
    }

    fn ticket(state: TicketState, attempts: u32, held_from: Option<TicketState>) -> Ticket {
        Ticket {
            id: tid(),
            project: pid(),
            title: "title".into(),
            goal: "goal".into(),
            hire: hid(),
            state,
            attempts,
            base_branch: "main".into(),
            created_at: 1,
            held_from,
            feedback: None,
            kind: TicketKind::Work,
            parent: None,
            detail: String::new(),
            source: None,
        }
    }

    fn assert_transition(
        from: Ticket,
        event: Event,
        expected: TicketState,
        attempts: u32,
        held_from: Option<TicketState>,
    ) {
        let next = apply(&from, &event).unwrap();
        assert_eq!(next.state, expected);
        assert_eq!(next.attempts, attempts);
        assert_eq!(next.held_from, held_from);
    }

    fn assert_forbidden(from: Ticket, event: Event) {
        assert!(apply(&from, &event).is_err());
    }

    #[test]
    fn allowed_transitions() {
        assert_transition(
            ticket(TicketState::Queued, 0, None),
            Event::Claimed,
            TicketState::Claimed,
            0,
            None,
        );
        assert_transition(
            ticket(TicketState::Claimed, 0, None),
            Event::RunStarted { run: rid() },
            TicketState::Working,
            0,
            None,
        );
        assert_transition(
            ticket(TicketState::Claimed, 2, None),
            Event::Held {
                reason: HeldReason::Budget,
            },
            TicketState::Held {
                reason: HeldReason::Budget,
            },
            2,
            Some(TicketState::Claimed),
        );
        assert_transition(
            ticket(TicketState::Claimed, 1, None),
            Event::Failed {
                class: FailureClass::Crash,
                detail: "crash".into(),
            },
            TicketState::Failed {
                class: FailureClass::Crash,
                detail: "crash".into(),
            },
            1,
            None,
        );

        assert_transition(
            ticket(TicketState::Working, 0, None),
            Event::ChecksStarted,
            TicketState::Checking,
            0,
            None,
        );
        assert_transition(
            ticket(TicketState::Working, 0, None),
            Event::AskOpened { ask: aid() },
            TicketState::Asking { ask: aid() },
            0,
            None,
        );
        assert_transition(
            ticket(TicketState::Working, 1, None),
            Event::Held {
                reason: HeldReason::Quota,
            },
            TicketState::Held {
                reason: HeldReason::Quota,
            },
            1,
            Some(TicketState::Working),
        );
        assert_transition(
            ticket(TicketState::Working, 2, None),
            Event::Failed {
                class: FailureClass::Timeout,
                detail: "timeout".into(),
            },
            TicketState::Failed {
                class: FailureClass::Timeout,
                detail: "timeout".into(),
            },
            2,
            None,
        );

        assert_transition(
            ticket(TicketState::Asking { ask: aid() }, 0, None),
            Event::AskResolved {
                ask: aid(),
                approved: true,
            },
            TicketState::Working,
            0,
            None,
        );
        assert_transition(
            ticket(TicketState::Asking { ask: aid() }, 2, None),
            Event::AskResolved {
                ask: aid(),
                approved: false,
            },
            TicketState::Failed {
                class: FailureClass::Denied,
                detail: "denied".into(),
            },
            2,
            None,
        );

        assert_transition(
            ticket(TicketState::Checking, 0, None),
            Event::ChecksPassed,
            TicketState::Reviewing,
            0,
            None,
        );
        assert_transition(
            ticket(TicketState::Checking, 0, None),
            Event::ChecksFailed {
                detail: "checks".into(),
            },
            TicketState::Working,
            1,
            None,
        );
        assert_transition(
            ticket(TicketState::Checking, 2, None),
            Event::Held {
                reason: HeldReason::Auth,
            },
            TicketState::Held {
                reason: HeldReason::Auth,
            },
            2,
            Some(TicketState::Checking),
        );
        assert_transition(
            ticket(TicketState::Checking, 1, None),
            Event::Failed {
                class: FailureClass::Tooling,
                detail: "tooling".into(),
            },
            TicketState::Failed {
                class: FailureClass::Tooling,
                detail: "tooling".into(),
            },
            1,
            None,
        );

        assert_transition(
            ticket(TicketState::Reviewing, 0, None),
            Event::ReviewPassed,
            TicketState::Ready,
            0,
            None,
        );
        assert_transition(
            ticket(TicketState::Reviewing, 1, None),
            Event::ReviewFailed {
                detail: "review".into(),
            },
            TicketState::Working,
            2,
            None,
        );
        assert_transition(
            ticket(TicketState::Reviewing, 0, None),
            Event::Held {
                reason: HeldReason::Conflict,
            },
            TicketState::Held {
                reason: HeldReason::Conflict,
            },
            0,
            Some(TicketState::Reviewing),
        );
        assert_transition(
            ticket(TicketState::Reviewing, 0, None),
            Event::Failed {
                class: FailureClass::Checks,
                detail: "checks".into(),
            },
            TicketState::Failed {
                class: FailureClass::Checks,
                detail: "checks".into(),
            },
            0,
            None,
        );

        assert_transition(
            ticket(TicketState::Ready, 0, None),
            Event::Landed,
            TicketState::Landed,
            0,
            None,
        );

        assert_transition(
            ticket(
                TicketState::Held {
                    reason: HeldReason::Paused,
                },
                0,
                Some(TicketState::Working),
            ),
            Event::Resumed,
            TicketState::Working,
            0,
            None,
        );

        assert_transition(
            ticket(
                TicketState::Failed {
                    class: FailureClass::Crash,
                    detail: "x".into(),
                },
                3,
                None,
            ),
            Event::Retried,
            TicketState::Queued,
            0,
            None,
        );
    }

    #[test]
    fn changes_requested_returns_to_queued_with_feedback() {
        let before = ticket(TicketState::Ready, 2, None);
        let next = apply(
            &before,
            &Event::ChangesRequested {
                detail: "fix typo".into(),
            },
        )
        .unwrap();
        assert_eq!(next.state, TicketState::Queued);
        assert_eq!(next.attempts, 2);
        assert_eq!(next.feedback, Some("fix typo".into()));
        assert_eq!(next.held_from, None);

        let overwritten = apply(
            &Ticket {
                feedback: Some("old".into()),
                ..before
            },
            &Event::ChangesRequested {
                detail: "new".into(),
            },
        )
        .unwrap();
        assert_eq!(overwritten.feedback, Some("new".into()));
    }

    #[test]
    fn a_conflict_returns_to_queued_without_costing_an_attempt() {
        let before = Ticket {
            feedback: Some("old".into()),
            ..ticket(TicketState::Ready, 2, None)
        };
        let conflicted = Event::Conflicted {
            branch: "studio/goal/x".into(),
            files: vec!["a.rs".into()],
        };
        let next = apply(&before, &conflicted).unwrap();
        assert_eq!(next.state, TicketState::Queued);
        assert_eq!(next.attempts, 2);
        assert_eq!(next.feedback, None);
        for state in [TicketState::Working, TicketState::Landed] {
            assert_forbidden(ticket(state, 0, None), conflicted.clone());
        }
    }

    #[test]
    fn cancelled_allowed_from_non_terminal_states() {
        let cases = [
            TicketState::Queued,
            TicketState::Claimed,
            TicketState::Working,
            TicketState::Checking,
            TicketState::Reviewing,
            TicketState::Ready,
            TicketState::Held {
                reason: HeldReason::Paused,
            },
            TicketState::Failed {
                class: FailureClass::Crash,
                detail: "x".into(),
            },
            TicketState::Asking { ask: aid() },
        ];
        for state in cases {
            let before = ticket(state, 2, None);
            let next = apply(&before, &Event::Cancelled).unwrap();
            assert_eq!(next.state, TicketState::Cancelled);
            assert_eq!(next.attempts, 2);
            assert_eq!(next.held_from, None);
        }
    }

    #[test]
    fn cancelled_forbidden_from_terminal_states() {
        assert_forbidden(ticket(TicketState::Landed, 0, None), Event::Cancelled);
        assert_forbidden(ticket(TicketState::Cancelled, 0, None), Event::Cancelled);
    }

    #[test]
    fn forbidden_transitions() {
        let queued = new_ticket();
        let claimed = ticket(TicketState::Claimed, 0, None);
        let working = ticket(TicketState::Working, 0, None);
        let asking = ticket(TicketState::Asking { ask: aid() }, 0, None);
        let checking = ticket(TicketState::Checking, 0, None);
        let reviewing = ticket(TicketState::Reviewing, 0, None);
        let ready = ticket(TicketState::Ready, 0, None);
        let landed = ticket(TicketState::Landed, 0, None);
        let failed = ticket(
            TicketState::Failed {
                class: FailureClass::Crash,
                detail: "x".into(),
            },
            0,
            None,
        );
        let held = ticket(
            TicketState::Held {
                reason: HeldReason::Budget,
            },
            0,
            Some(TicketState::Working),
        );
        let held_no_source = ticket(
            TicketState::Held {
                reason: HeldReason::Budget,
            },
            0,
            None,
        );
        let cancelled = ticket(TicketState::Cancelled, 0, None);

        assert_forbidden(queued.clone(), Event::RunStarted { run: rid() });
        assert_forbidden(queued.clone(), Event::ChecksStarted);
        assert_forbidden(queued.clone(), Event::ChecksPassed);
        assert_forbidden(queued.clone(), Event::ReviewPassed);
        assert_forbidden(queued.clone(), Event::Landed);
        assert_forbidden(
            queued.clone(),
            Event::Held {
                reason: HeldReason::Budget,
            },
        );
        assert_forbidden(queued.clone(), Event::Resumed);
        assert_forbidden(queued, Event::ChangesRequested { detail: "x".into() });

        assert_forbidden(claimed.clone(), Event::Claimed);
        assert_forbidden(claimed.clone(), Event::ChecksStarted);
        assert_forbidden(claimed.clone(), Event::AskOpened { ask: aid() });
        assert_forbidden(claimed.clone(), Event::ChecksPassed);
        assert_forbidden(claimed.clone(), Event::ReviewPassed);
        assert_forbidden(claimed.clone(), Event::Landed);
        assert_forbidden(claimed.clone(), Event::Resumed);
        assert_forbidden(claimed.clone(), Event::Retried);
        assert_forbidden(claimed, Event::ChangesRequested { detail: "x".into() });

        assert_forbidden(working.clone(), Event::Claimed);
        assert_forbidden(working.clone(), Event::RunStarted { run: rid() });
        assert_forbidden(working.clone(), Event::ChecksPassed);
        assert_forbidden(working.clone(), Event::ReviewPassed);
        assert_forbidden(working.clone(), Event::Landed);
        assert_forbidden(working.clone(), Event::Resumed);
        assert_forbidden(working.clone(), Event::Retried);
        assert_forbidden(working, Event::ChangesRequested { detail: "x".into() });

        assert_forbidden(asking.clone(), Event::Claimed);
        assert_forbidden(asking.clone(), Event::RunStarted { run: rid() });
        assert_forbidden(asking.clone(), Event::ChecksStarted);
        assert_forbidden(asking.clone(), Event::AskOpened { ask: aid() });
        assert_forbidden(asking.clone(), Event::ChecksPassed);
        assert_forbidden(asking.clone(), Event::ChecksFailed { detail: "x".into() });
        assert_forbidden(asking.clone(), Event::ReviewPassed);
        assert_forbidden(asking.clone(), Event::ReviewFailed { detail: "x".into() });
        assert_forbidden(asking.clone(), Event::Landed);
        assert_forbidden(asking.clone(), Event::Resumed);
        assert_forbidden(asking.clone(), Event::Retried);
        assert_forbidden(
            asking.clone(),
            Event::Failed {
                class: FailureClass::Crash,
                detail: "x".into(),
            },
        );
        assert_forbidden(
            asking.clone(),
            Event::AskResolved {
                ask: AskId::new("wrong"),
                approved: true,
            },
        );
        assert_forbidden(asking, Event::ChangesRequested { detail: "x".into() });

        assert_forbidden(checking.clone(), Event::Claimed);
        assert_forbidden(checking.clone(), Event::RunStarted { run: rid() });
        assert_forbidden(checking.clone(), Event::AskOpened { ask: aid() });
        assert_forbidden(checking.clone(), Event::ChecksStarted);
        assert_forbidden(checking.clone(), Event::ReviewPassed);
        assert_forbidden(checking.clone(), Event::Landed);
        assert_forbidden(checking.clone(), Event::Resumed);
        assert_forbidden(checking.clone(), Event::Retried);
        assert_forbidden(checking, Event::ChangesRequested { detail: "x".into() });

        assert_forbidden(reviewing.clone(), Event::Claimed);
        assert_forbidden(reviewing.clone(), Event::RunStarted { run: rid() });
        assert_forbidden(reviewing.clone(), Event::ChecksStarted);
        assert_forbidden(reviewing.clone(), Event::ChecksPassed);
        assert_forbidden(
            reviewing.clone(),
            Event::ChecksFailed { detail: "x".into() },
        );
        assert_forbidden(reviewing.clone(), Event::Landed);
        assert_forbidden(reviewing.clone(), Event::Resumed);
        assert_forbidden(reviewing.clone(), Event::Retried);
        assert_forbidden(reviewing, Event::ChangesRequested { detail: "x".into() });

        assert_forbidden(ready.clone(), Event::Claimed);
        assert_forbidden(ready.clone(), Event::RunStarted { run: rid() });
        assert_forbidden(ready.clone(), Event::ChecksStarted);
        assert_forbidden(ready.clone(), Event::AskOpened { ask: aid() });
        assert_forbidden(ready.clone(), Event::ChecksPassed);
        assert_forbidden(ready.clone(), Event::ReviewPassed);
        assert_forbidden(ready.clone(), Event::ReviewFailed { detail: "x".into() });
        assert_forbidden(
            ready.clone(),
            Event::Held {
                reason: HeldReason::Budget,
            },
        );
        assert_forbidden(
            ready.clone(),
            Event::Failed {
                class: FailureClass::Crash,
                detail: "x".into(),
            },
        );
        assert_forbidden(ready.clone(), Event::Resumed);
        assert_forbidden(ready, Event::Retried);

        assert_forbidden(landed.clone(), Event::Claimed);
        assert_forbidden(landed.clone(), Event::RunStarted { run: rid() });
        assert_forbidden(landed.clone(), Event::ChecksStarted);
        assert_forbidden(landed.clone(), Event::ChecksPassed);
        assert_forbidden(landed.clone(), Event::ReviewPassed);
        assert_forbidden(landed.clone(), Event::Landed);
        assert_forbidden(
            landed.clone(),
            Event::Held {
                reason: HeldReason::Budget,
            },
        );
        assert_forbidden(landed.clone(), Event::Resumed);
        assert_forbidden(landed.clone(), Event::Retried);
        assert_forbidden(landed.clone(), Event::Cancelled);
        assert_forbidden(
            landed.clone(),
            Event::Failed {
                class: FailureClass::Crash,
                detail: "x".into(),
            },
        );
        assert_forbidden(landed, Event::ChangesRequested { detail: "x".into() });

        assert_forbidden(failed.clone(), Event::Claimed);
        assert_forbidden(failed.clone(), Event::RunStarted { run: rid() });
        assert_forbidden(failed.clone(), Event::ChecksStarted);
        assert_forbidden(failed.clone(), Event::AskOpened { ask: aid() });
        assert_forbidden(failed.clone(), Event::ChecksPassed);
        assert_forbidden(failed.clone(), Event::ChecksFailed { detail: "x".into() });
        assert_forbidden(failed.clone(), Event::ReviewPassed);
        assert_forbidden(failed.clone(), Event::ReviewFailed { detail: "x".into() });
        assert_forbidden(failed.clone(), Event::Landed);
        assert_forbidden(
            failed.clone(),
            Event::Held {
                reason: HeldReason::Budget,
            },
        );
        assert_forbidden(failed.clone(), Event::Resumed);
        assert_forbidden(
            failed.clone(),
            Event::Failed {
                class: FailureClass::Crash,
                detail: "x".into(),
            },
        );
        assert_forbidden(failed, Event::ChangesRequested { detail: "x".into() });

        assert_forbidden(held_no_source, Event::Resumed);
        assert_forbidden(held.clone(), Event::Claimed);
        assert_forbidden(held.clone(), Event::RunStarted { run: rid() });
        assert_forbidden(held.clone(), Event::ChecksStarted);
        assert_forbidden(held.clone(), Event::ChecksPassed);
        assert_forbidden(held.clone(), Event::ReviewPassed);
        assert_forbidden(held.clone(), Event::Landed);
        assert_forbidden(
            held.clone(),
            Event::Failed {
                class: FailureClass::Crash,
                detail: "x".into(),
            },
        );
        assert_forbidden(held, Event::ChangesRequested { detail: "x".into() });

        assert_forbidden(cancelled.clone(), Event::Claimed);
        assert_forbidden(cancelled.clone(), Event::RunStarted { run: rid() });
        assert_forbidden(cancelled.clone(), Event::ChecksStarted);
        assert_forbidden(cancelled.clone(), Event::AskOpened { ask: aid() });
        assert_forbidden(cancelled.clone(), Event::ChecksPassed);
        assert_forbidden(
            cancelled.clone(),
            Event::ChecksFailed { detail: "x".into() },
        );
        assert_forbidden(cancelled.clone(), Event::ReviewPassed);
        assert_forbidden(
            cancelled.clone(),
            Event::ReviewFailed { detail: "x".into() },
        );
        assert_forbidden(cancelled.clone(), Event::Landed);
        assert_forbidden(
            cancelled.clone(),
            Event::Held {
                reason: HeldReason::Budget,
            },
        );
        assert_forbidden(cancelled.clone(), Event::Resumed);
        assert_forbidden(cancelled.clone(), Event::Retried);
        assert_forbidden(cancelled.clone(), Event::Cancelled);
        assert_forbidden(cancelled, Event::ChangesRequested { detail: "x".into() });
    }

    #[test]
    fn check_retry_limit() {
        for attempts in 0..MAX_ATTEMPTS - 1 {
            let next = apply(
                &ticket(TicketState::Checking, attempts, None),
                &Event::ChecksFailed {
                    detail: "fail".into(),
                },
            )
            .unwrap();
            assert_eq!(next.state, TicketState::Working);
            assert_eq!(next.attempts, attempts + 1);
        }

        let final_state = apply(
            &ticket(TicketState::Checking, MAX_ATTEMPTS - 1, None),
            &Event::ChecksFailed {
                detail: "final".into(),
            },
        )
        .unwrap();
        assert_eq!(
            final_state.state,
            TicketState::Failed {
                class: FailureClass::Checks,
                detail: "final".into(),
            }
        );
        assert_eq!(final_state.attempts, MAX_ATTEMPTS - 1);
    }

    #[test]
    fn review_retry_limit() {
        for attempts in 0..MAX_ATTEMPTS - 1 {
            let next = apply(
                &ticket(TicketState::Reviewing, attempts, None),
                &Event::ReviewFailed {
                    detail: "fail".into(),
                },
            )
            .unwrap();
            assert_eq!(next.state, TicketState::Working);
            assert_eq!(next.attempts, attempts + 1);
        }

        let final_state = apply(
            &ticket(TicketState::Reviewing, MAX_ATTEMPTS - 1, None),
            &Event::ReviewFailed {
                detail: "final".into(),
            },
        )
        .unwrap();
        assert_eq!(
            final_state.state,
            TicketState::Failed {
                class: FailureClass::Review,
                detail: "final".into(),
            }
        );
        assert_eq!(final_state.attempts, MAX_ATTEMPTS - 1);
    }

    #[test]
    fn held_resume_round_trip() {
        let cases = [
            (TicketState::Claimed, HeldReason::Budget),
            (TicketState::Working, HeldReason::Quota),
            (TicketState::Checking, HeldReason::Auth),
            (TicketState::Reviewing, HeldReason::Conflict),
        ];
        for (state, reason) in cases {
            let before = ticket(state.clone(), 2, None);
            let held = apply(
                &before,
                &Event::Held {
                    reason: reason.clone(),
                },
            )
            .unwrap();
            assert_eq!(
                held.state,
                TicketState::Held {
                    reason: reason.clone(),
                }
            );
            assert_eq!(held.held_from, Some(state.clone()));
            assert_eq!(held.attempts, 2);

            let resumed = apply(&held, &Event::Resumed).unwrap();
            assert_eq!(resumed.state, state);
            assert_eq!(resumed.held_from, None);
            assert_eq!(resumed.attempts, 2);
        }
    }

    #[test]
    fn held_ask_resumes_as_working() {
        let asking = ticket(TicketState::Asking { ask: aid() }, 1, None);
        let held = apply(
            &asking,
            &Event::Held {
                reason: HeldReason::Paused,
            },
        )
        .unwrap();
        assert_eq!(held.held_from, Some(TicketState::Working));
        let resumed = apply(&held, &Event::Resumed).unwrap();
        assert_eq!(resumed.state, TicketState::Working);
        assert_eq!(resumed.attempts, 1);
    }

    #[test]
    fn replay_equals_stepwise_apply() {
        let events = vec![
            created_event(),
            Event::Claimed,
            Event::RunStarted { run: rid() },
            Event::ChecksStarted,
            Event::ChecksPassed,
            Event::ReviewPassed,
            Event::Landed,
        ];
        let replayed = replay(&events).unwrap();

        let mut stepwise = replay(&[events[0].clone()]).unwrap();
        for event in &events[1..] {
            stepwise = apply(&stepwise, event).unwrap();
        }
        assert_eq!(replayed, stepwise);
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0
        }
    }

    fn allowed_events(state: &TicketState) -> Vec<Event> {
        let held = || {
            vec![
                Event::Held {
                    reason: HeldReason::Budget,
                },
                Event::Held {
                    reason: HeldReason::Quota,
                },
                Event::Held {
                    reason: HeldReason::Auth,
                },
                Event::Held {
                    reason: HeldReason::Conflict,
                },
                Event::Held {
                    reason: HeldReason::Paused,
                },
            ]
        };
        let cancelled = || Event::Cancelled;
        match state {
            TicketState::Queued => vec![Event::Claimed, cancelled()],
            TicketState::Claimed => {
                let mut events = vec![
                    Event::RunStarted { run: rid() },
                    Event::Failed {
                        class: FailureClass::Crash,
                        detail: "crash".into(),
                    },
                    Event::Failed {
                        class: FailureClass::Timeout,
                        detail: "timeout".into(),
                    },
                    cancelled(),
                ];
                events.extend(held());
                events
            }
            TicketState::Working => {
                let mut events = vec![
                    Event::ChecksStarted,
                    Event::AskOpened { ask: aid() },
                    Event::Failed {
                        class: FailureClass::Crash,
                        detail: "crash".into(),
                    },
                    Event::Failed {
                        class: FailureClass::Timeout,
                        detail: "timeout".into(),
                    },
                    cancelled(),
                ];
                events.extend(held());
                events
            }
            TicketState::Asking { ask } => vec![
                Event::AskResolved {
                    ask: ask.clone(),
                    approved: true,
                },
                Event::AskResolved {
                    ask: ask.clone(),
                    approved: false,
                },
                cancelled(),
                Event::Held {
                    reason: HeldReason::Paused,
                },
            ],
            TicketState::Checking => {
                let mut events = vec![
                    Event::ChecksPassed,
                    Event::ChecksFailed {
                        detail: "checks".into(),
                    },
                    Event::Failed {
                        class: FailureClass::Tooling,
                        detail: "tooling".into(),
                    },
                    cancelled(),
                ];
                events.extend(held());
                events
            }
            TicketState::Reviewing => {
                let mut events = vec![
                    Event::ReviewPassed,
                    Event::ReviewFailed {
                        detail: "review".into(),
                    },
                    Event::Failed {
                        class: FailureClass::Review,
                        detail: "review".into(),
                    },
                    cancelled(),
                ];
                events.extend(held());
                events
            }
            TicketState::Ready => vec![
                Event::Landed,
                Event::ChangesRequested {
                    detail: "feedback".into(),
                },
                cancelled(),
            ],
            TicketState::Held { .. } => vec![Event::Resumed, cancelled()],
            TicketState::Failed { .. } => vec![Event::Retried, cancelled()],
            TicketState::Landed => vec![],
            TicketState::Cancelled => vec![],
        }
    }

    fn random_events(seed: u64, limit: usize) -> Vec<Event> {
        let mut rng = Rng(seed);
        let mut events = vec![created_event()];
        let mut t = replay(&events).unwrap();
        for _ in 0..limit {
            let options = allowed_events(&t.state);
            if options.is_empty() {
                break;
            }
            let idx = (rng.next() as usize) % options.len();
            let event = options[idx].clone();
            t = apply(&t, &event).unwrap();
            events.push(event);
        }
        events
    }

    #[test]
    fn random_walk_replay_determinism() {
        let seq = random_events(0x1234_5678_9abc_def0, 250);
        assert!(!seq.is_empty());

        let replayed = replay(&seq).unwrap();
        assert_eq!(replay(&seq).unwrap(), replayed);

        let mut stepwise = replay(&[seq[0].clone()]).unwrap();
        for event in &seq[1..] {
            stepwise = apply(&stepwise, event).unwrap();
        }
        assert_eq!(replayed, stepwise);
    }

    fn plan(state: TicketState) -> Ticket {
        Ticket {
            kind: TicketKind::Plan,
            ..ticket(state, 0, None)
        }
    }

    #[test]
    fn a_finished_plan_lands_without_checks_or_review() {
        assert_transition(
            plan(TicketState::Working),
            Event::Landed,
            TicketState::Landed,
            0,
            None,
        );
        assert_forbidden(plan(TicketState::Working), Event::ChecksStarted);
    }

    #[test]
    fn work_cannot_land_straight_from_working() {
        assert_forbidden(ticket(TicketState::Working, 0, None), Event::Landed);
    }

    #[test]
    fn a_review_ask_returns_to_reviewing() {
        let asking = apply(
            &ticket(TicketState::Reviewing, 1, None),
            &Event::AskOpened { ask: aid() },
        )
        .unwrap();
        assert_eq!(asking.state, TicketState::Asking { ask: aid() });
        let resolved = |approved| {
            apply(
                &asking,
                &Event::AskResolved {
                    ask: aid(),
                    approved,
                },
            )
            .unwrap()
        };
        let back = resolved(true);
        assert_eq!(back.state, TicketState::Reviewing);
        assert_eq!((back.attempts, back.held_from), (1, None));
        assert!(matches!(resolved(false).state, TicketState::Failed { .. }));
        let held = apply(
            &asking,
            &Event::Held {
                reason: HeldReason::Paused,
            },
        )
        .unwrap();
        assert_eq!(held.held_from, Some(TicketState::Reviewing));
    }

    #[test]
    fn old_created_events_still_load() {
        let old = r#"{"type":"created","id":"t-1","project":"p","title":"t","goal":"g","hire":"h","base_branch":"main","created_at":1}"#;
        let event: Event = serde_json::from_str(old).unwrap();
        let ticket = replay(&[event]).unwrap();
        assert_eq!(ticket.kind, TicketKind::Work);
        assert_eq!(ticket.parent, None);
        assert_eq!(ticket.detail, "");
    }
}
