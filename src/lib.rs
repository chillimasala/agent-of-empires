//! Agent of Empires library - Core functionality for the terminal session manager

#[cfg(feature = "serve")]
pub mod acp;
pub mod agents;
pub mod build_info;
pub mod claude_settings;
pub mod cli;
pub mod containers;
/// Protocol-agnostic durable event log, the storage substrate behind the
/// ACP transcript store and (later) the plugin host's event bus. Serve-gated
/// because its only consumer today is the serve-gated acp module.
#[cfg(feature = "serve")]
pub mod events;
pub mod file_watch;
pub mod git;
pub mod github;
pub mod hooks;
pub mod logging;
pub mod migrations;
/// Pure-Rust client for the long-lived `opencode serve` HTTP
/// transport. The types, client, SSE consumer, and Todo mapping
/// live here; the bridge into `AcpState::Event` is a follow-up in
/// `src/acp/opencode_bridge.rs`. Ungate stays — there's no JS or
/// heavy dep, and keeping the module reachable from any build
/// makes incremental rollout cheaper.
pub mod opencode;
pub mod plugin;
pub mod process;
#[cfg(feature = "serve")]
pub mod server;
pub mod session;
pub mod sound;
mod status_hooks;
pub mod task_util;
pub mod telemetry;
pub mod terminal;
pub mod tips;
pub mod tmux;
pub mod tui;
pub mod update;
