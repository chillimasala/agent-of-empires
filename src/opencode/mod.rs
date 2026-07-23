//! Pure-Rust client for a long-lived `opencode serve` HTTP server.
//!
//! Lets aoe own one opencode process per project and address N
//! sessions inside it by id, instead of spawning a fresh `opencode
//! acp` subprocess per task (which costs ~300-400 MB each). Together
//! with `src/acp/agent_registry::AgentKind::OpencodeServer`, this is
//! the transport half of the memory-efficient multi-session
//! transport. The other half, mapping opencode's SSE events into
//! `AcpState::Event`, lives in [`mapping`].

#![warn(clippy::all)]

mod client;
mod events;
mod mapping;
pub mod types;

pub use client::{Client, ClientError, Health};
pub use events::{EventStream, StreamError};
pub use mapping::{map_event, opencode_todo_to_acp_todo, OpencodeEventToAcp};
pub use types::{
    Agent, Message, Part, Permission, Session, SessionStatus, Status as OpencodeStatus, Todo,
    ToolState,
};
