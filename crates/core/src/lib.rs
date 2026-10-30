//! Larrier core: the unified session format (LSF), the agent catalog,
//! configuration (accounts, providers, workspaces) and the on-disk store.

pub mod agent;
pub mod automation;
pub mod config;
pub mod lsf;
pub mod models;
pub mod secrets;
pub mod store;

pub use agent::{AgentKind, AgentSpec, ModeGroup, ModeOption, ModeSelection, Protocol};
pub use config::{Account, AccountAuth, AgentEndpoint, BusySend, ButtonCorners, Config, CustomModel, Density, EndpointKind, Provider, ProviderKind, RunMode, Settings, SidebarGroup, SidebarSort, TextSizes, Theme, UpdateChannel, Workspace};
pub use lsf::Environment;
pub use lsf::{
    FileDiff, Message, NativeLink, Origin, Part, PlanEntry, PlanStatus, Role, Session, SessionStatus, SessionSummary,
    ToolCall, ToolKind, ToolStatus, Usage, WorkspaceRef,
};
pub use store::Store;

pub const APP_NAME: &str = "Larrier";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
