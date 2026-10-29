//! Larrier agent runtime: launches agents (CLI stream protocols, ACP, HTTP
//! chat), normalizes their output into [`AgentEvent`]s and records them into
//! LSF sessions.

pub mod detect;
pub mod drivers;
pub mod engine;
pub mod events;
pub mod git;
pub mod harness;
pub mod request;

pub use detect::{detect_all, Detected};
pub use engine::TurnRecorder;
pub use events::{AgentEvent, PermissionOption, PermissionRequest, TurnControl, TurnHandle};
pub use request::{build_request, TurnRequest};

/// Start one turn on the current tokio runtime.
pub fn start_turn(req: TurnRequest) -> TurnHandle {
    let (sink, control_rx, handle) = events::turn_channels();
    tokio::spawn(drivers::run(req, sink, control_rx));
    handle
}
