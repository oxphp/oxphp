mod dispatcher;
mod types;

pub use dispatcher::EventDispatcher;
pub use types::*;

use std::any::Any;

/// Marker trait for event types. All events must be `Any + Send + Sync + 'static`.
pub trait Event: Any + Send + Sync + 'static {
    /// Human-readable event name for debugging/logging.
    fn name(&self) -> &'static str;
}

/// Handler execution priority. Lower values run first.
pub type Priority = i32;

/// Controls whether subsequent handlers are invoked after this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Propagation {
    /// Continue dispatching to the next handler.
    Continue,
    /// Stop dispatching — no subsequent handlers will run.
    Stop,
}

/// Synchronous event handler for a specific event type.
pub trait EventHandler<E: Event>: Send + Sync {
    /// Handle the event, possibly mutating it. Return `Propagation::Stop` to
    /// prevent subsequent handlers from running.
    fn handle(&self, event: &mut E) -> Propagation;

    /// Execution priority. Lower values run first. Default is 0.
    fn priority(&self) -> Priority {
        0
    }
}
