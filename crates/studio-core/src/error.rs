use crate::event::Event;
use crate::ticket::TicketState;
use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
#[error("transition from {from:?} with {event:?} is not allowed")]
pub struct TransitionError {
    pub from: Box<TicketState>,
    pub event: Box<Event>,
}

#[derive(Error, Debug, PartialEq, Eq)]
pub enum ReplayError {
    #[error("no events to replay")]
    Empty,
    #[error("first event must be Created")]
    MissingCreated,
    #[error(transparent)]
    Transition(#[from] TransitionError),
}
