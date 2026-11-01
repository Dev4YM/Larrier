//! Larrier agent runtime: launches agents (CLI stream protocols, ACP, HTTP
//! chat), normalizes their output into [`AgentEvent`]s and records them into
//! LSF sessions.

pub mod browser;
pub mod detect;
pub mod drivers;
pub mod engine;
pub mod events;
pub mod git;
pub mod models;
pub mod harness;
pub mod request;
pub mod updates;

pub use detect::{detect_all, Detected};
pub use engine::TurnRecorder;
pub use events::{AgentEvent, AskOption, AskQuestion, PermissionOption, PermissionRequest, Reply, RequestKind, TurnControl, TurnHandle};
pub use request::{build_request, can_steer, TurnRequest};

/// Start one turn on the current tokio runtime.
pub fn start_turn(req: TurnRequest) -> TurnHandle {
    let (sink, control_rx, handle) = events::turn_channels();
    tokio::spawn(drivers::run(req, sink, control_rx));
    handle
}
