//! Types from opencode's HTTP server, hand-translated from
//! `packages/sdk/js/src/gen/types.gen.ts` in the sst/opencode repo.
//!
//! We only model the surface aoe actually uses: sessions, messages,
//! parts, permissions, todos, tool state, and the agent list. Anything
//! else is intentionally absent; it gets a `serde_json::Value`
//! passthrough in [`super::events`].

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// An opencode session. The id (`ses_*`) is the AoE-side handle; pass
/// it back in every URL path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    #[serde(default, rename = "projectID")]
    pub project_id: Option<String>,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default, rename = "parentID")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub summary: Option<SessionSummary>,
    /// Unix epoch milliseconds for both created and updated.
    #[serde(default)]
    pub time: Option<Time>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    #[serde(default)]
    pub additions: i64,
    #[serde(default)]
    pub deletions: i64,
    #[serde(default)]
    pub files: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Time {
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub updated: i64,
}

/// A bus event from `/global/event`. AoE subscribes once and routes
/// each frame by directory plus payload session id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalEvent {
    #[serde(default)]
    pub directory: Option<String>,
    /// The actual event, a `type`-tagged enum.
    pub payload: Event,
}

/// `Event` is the opencode server's `type: "..."` tagged enum. We
/// model only the variants aoe cares about. Unknown variants become
/// `Unknown` so a future opencode release does not break the stream.
///
/// Note: opencode uses dotted `type` strings (`session.created`),
/// which serde's `rename_all` doesn't support, so each variant
/// has an explicit `#[serde(rename = ...)]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    #[serde(rename = "server.connected")]
    ServerConnected { properties: serde_json::Value },
    #[serde(rename = "session.created")]
    SessionCreated { properties: SessionProperties },
    #[serde(rename = "session.updated")]
    SessionUpdated { properties: SessionProperties },
    #[serde(rename = "session.deleted")]
    SessionDeleted { properties: SessionProperties },
    #[serde(rename = "session.idle")]
    SessionIdle { properties: SessionIdProperty },
    #[serde(rename = "session.status")]
    SessionStatus { properties: SessionStatusProperties },
    #[serde(rename = "session.compacted")]
    SessionCompacted { properties: SessionIdProperty },
    #[serde(rename = "session.error")]
    SessionError {
        #[serde(default)]
        properties: serde_json::Value,
    },
    #[serde(rename = "message.updated")]
    MessageUpdated {
        properties: MessageUpdatedProperties,
    },
    #[serde(rename = "message.removed")]
    MessageRemoved {
        properties: MessageRemovedProperties,
    },
    #[serde(rename = "message.part.updated")]
    MessagePartUpdated {
        properties: MessagePartUpdatedProperties,
    },
    #[serde(rename = "message.part.removed")]
    MessagePartRemoved {
        properties: MessagePartRemovedProperties,
    },
    #[serde(rename = "permission.asked")]
    PermissionAsked { properties: Permission },
    #[serde(rename = "permission.replied")]
    PermissionReplied {
        properties: PermissionRepliedProperties,
    },
    #[serde(rename = "todo.updated")]
    TodoUpdated { properties: TodoUpdatedProperties },
    #[serde(rename = "file.edited")]
    FileEdited { properties: FileEditedProperties },
    #[serde(rename = "lsp.updated")]
    LspUpdated { properties: serde_json::Value },
    /// Catchall for any event type we don't (yet) model. Tagged
    /// "other" so unknown frames still parse instead of failing
    /// the whole stream.
    #[serde(other)]
    Unknown,
}

impl Event {
    /// Session id carried by session-scoped events.
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Event::SessionIdle { properties } | Event::SessionCompacted { properties } => {
                Some(&properties.session_id)
            }
            Event::SessionStatus { properties } => Some(&properties.session_id),
            Event::MessageUpdated { properties } => Some(&properties.info.session_id),
            Event::MessageRemoved { properties } => Some(&properties.session_id),
            Event::MessagePartUpdated { properties } => properties.part.session_id(),
            Event::MessagePartRemoved { properties } => Some(&properties.session_id),
            Event::PermissionAsked { properties } => Some(&properties.session_id),
            Event::PermissionReplied { properties } => Some(&properties.session_id),
            Event::TodoUpdated { properties } => Some(&properties.session_id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionProperties {
    pub info: Session,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIdProperty {
    #[serde(rename = "sessionID")]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatusProperties {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub status: SessionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SessionStatus {
    Idle,
    Busy,
    Retry {
        attempt: i64,
        message: String,
        next: i64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageUpdatedProperties {
    pub info: Message,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRemovedProperties {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID")]
    pub message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePartUpdatedProperties {
    pub part: Part,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePartRemovedProperties {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(rename = "partID")]
    pub part_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRepliedProperties {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "requestID")]
    pub request_id: String,
    pub reply: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoUpdatedProperties {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub todos: Vec<Todo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEditedProperties {
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub permission: String,
    pub patterns: Vec<String>,
    #[serde(default)]
    pub always: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub tool: Option<PermissionTool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionTool {
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(rename = "callID")]
    pub call_id: String,
}

/// An opencode `Todo` from `EventTodoUpdated`. We translate this into
/// the ACP `Todo` shape in [`super::mapping`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    #[serde(default)]
    pub id: String,
    pub content: String,
    /// `"pending" | "in_progress" | "completed" | "cancelled"`.
    pub status: String,
    /// `"high" | "medium" | "low"`. AoE's Todo type has no priority
    /// field; we drop it.
    #[serde(default)]
    pub priority: String,
}

/// A user or assistant message. Used as the `info` payload of
/// `message.updated`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub role: String,
    /// Tokens are present on assistant messages only; we keep the
    /// full struct so the mapper can read `cost` for the UsageUpdated
    /// bridge without a second API call.
    #[serde(default)]
    pub tokens: Option<TokenCounts>,
    #[serde(default)]
    pub cost: Option<f64>,
    #[serde(default)]
    pub time: Option<Time>,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCounts {
    #[serde(default)]
    pub input: i64,
    #[serde(default)]
    pub output: i64,
    #[serde(default)]
    pub reasoning: i64,
    #[serde(default)]
    pub cache: Option<CacheCounts>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheCounts {
    #[serde(default)]
    pub read: i64,
    #[serde(default)]
    pub write: i64,
}

/// A `Part` is a unit of a message (text, tool, file, ...). Tool
/// parts carry a `state` with one of the [`ToolState`] shapes that
/// drive the structured-view tool card.
///
/// We model the variants the structured view renders and stash
/// unknown shapes in `Other { raw }`. `Other` is a struct variant
/// so the full original JSON survives the round-trip, which is
/// what the structured view's `Event::RawAgentUpdate` escape hatch
/// needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Part {
    Text {
        id: String,
        #[serde(rename = "sessionID")]
        session_id: String,
        #[serde(rename = "messageID")]
        message_id: String,
        text: String,
    },
    Tool {
        id: String,
        #[serde(rename = "sessionID")]
        session_id: String,
        #[serde(rename = "messageID")]
        message_id: String,
        #[serde(rename = "callID")]
        call_id: String,
        tool: String,
        state: ToolState,
    },
    Reasoning {
        id: String,
        #[serde(rename = "sessionID")]
        session_id: String,
        #[serde(rename = "messageID")]
        message_id: String,
        text: String,
    },
    File {
        id: String,
        #[serde(rename = "sessionID")]
        session_id: String,
        #[serde(rename = "messageID")]
        message_id: String,
        mime: String,
        url: String,
    },
    StepStart {
        id: String,
        #[serde(rename = "sessionID")]
        session_id: String,
        #[serde(rename = "messageID")]
        message_id: String,
    },
    StepFinish {
        id: String,
        #[serde(rename = "sessionID")]
        session_id: String,
        #[serde(rename = "messageID")]
        message_id: String,
        #[serde(default)]
        cost: Option<f64>,
        #[serde(default)]
        tokens: Option<TokenCounts>,
    },
    /// Anything else: step-snapshot, patch, agent, retry, compaction,
    /// or subtask. Unknown parts are ignored until the ACP bridge
    /// learns their shape.
    #[serde(other)]
    Other,
}

impl Part {
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Part::Text { session_id, .. }
            | Part::Tool { session_id, .. }
            | Part::Reasoning { session_id, .. }
            | Part::File { session_id, .. }
            | Part::StepStart { session_id, .. }
            | Part::StepFinish { session_id, .. } => Some(session_id),
            Part::Other => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ToolState {
    Pending {
        input: serde_json::Value,
    },
    Running {
        input: serde_json::Value,
        #[serde(default)]
        title: Option<String>,
    },
    Completed {
        input: serde_json::Value,
        output: String,
        title: String,
        #[serde(default)]
        metadata: HashMap<String, serde_json::Value>,
    },
    Error {
        input: serde_json::Value,
        error: String,
    },
}

/// A row from `GET /agent`, describing an agent opencode can dispatch to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub mode: String,
    #[serde(default)]
    pub model: Option<AgentModelRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentModelRef {
    #[serde(rename = "modelID")]
    pub model_id: String,
    #[serde(rename = "providerID")]
    pub provider_id: String,
}

/// `GET /global/health` response: `{"healthy": true, "version": "..."}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub healthy: bool,
    pub version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_created_event_from_canonical_payload() {
        // Real shape: opencode sends `{"type":"session.created","properties":{"info":{...}}}`.
        let raw = r#"{
            "directory": "/tmp/proj",
            "payload": {
                "type": "session.created",
                "properties": {
                    "info": {
                        "id": "ses_abc",
                        "title": "hello",
                        "version": "1.17.20",
                        "time": {"created": 1, "updated": 2}
                    }
                }
            }
        }"#;
        let ev: GlobalEvent = serde_json::from_str(raw).unwrap();
        match ev.payload {
            Event::SessionCreated { properties } => {
                assert_eq!(properties.info.id, "ses_abc");
                assert_eq!(properties.info.title.as_deref(), Some("hello"));
            }
            other => panic!("expected SessionCreated, got {other:?}"),
        }
    }

    #[test]
    fn parses_session_status_idle_and_busy_and_retry() {
        for (raw, expected) in [
            (
                r#"{"type":"session.status","properties":{"sessionID":"s1","status":{"type":"idle"}}}"#,
                SessionStatus::Idle,
            ),
            (
                r#"{"type":"session.status","properties":{"sessionID":"s1","status":{"type":"busy"}}}"#,
                SessionStatus::Busy,
            ),
            (
                r#"{"type":"session.status","properties":{"sessionID":"s1","status":{"type":"retry","attempt":3,"message":"rate","next":4}}}"#,
                SessionStatus::Retry {
                    attempt: 3,
                    message: "rate".into(),
                    next: 4,
                },
            ),
        ] {
            let ev: Event = serde_json::from_str(raw).unwrap();
            let got = match ev {
                Event::SessionStatus { properties } => properties.status,
                _ => panic!("not a status event"),
            };
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn parses_todo_updated_with_priority_and_status() {
        let raw = r#"{
            "type": "todo.updated",
            "properties": {
                "sessionID": "s1",
                "todos": [
                    {"id":"t1","content":"do thing","status":"in_progress","priority":"high"},
                    {"id":"t2","content":"other","status":"completed","priority":"low"}
                ]
            }
        }"#;
        let ev: Event = serde_json::from_str(raw).unwrap();
        let todos = match ev {
            Event::TodoUpdated { properties } => properties.todos,
            _ => panic!(),
        };
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].status, "in_progress");
        assert_eq!(todos[0].priority, "high");
        assert_eq!(todos[1].status, "completed");
    }

    #[test]
    fn parses_permission_with_minimal_fields() {
        let raw = r#"{
            "id": "perm_1",
            "sessionID": "ses_x",
            "permission": "bash",
            "patterns": ["rm -rf"],
            "metadata": {},
            "always": [],
            "tool": {"messageID": "msg_y", "callID": "call_z"}
        }"#;
        let p: Permission = serde_json::from_str(raw).unwrap();
        assert_eq!(p.id, "perm_1");
        assert_eq!(p.permission, "bash");
        assert_eq!(p.tool.unwrap().call_id, "call_z");
    }

    #[test]
    fn unknown_event_type_does_not_break_the_stream() {
        // Future opencode adds a new event type; we must still
        // deserialise the frame (as `Unknown`) and keep going.
        let raw = r#"{"payload":{"type":"some.future.event","properties":{}}}"#;
        let ev: GlobalEvent = serde_json::from_str(raw).unwrap();
        assert!(matches!(ev.payload, Event::Unknown));
    }

    #[test]
    fn tool_state_all_variants_roundtrip() {
        for raw in [
            r#"{"status":"pending","input":{"cmd":"ls"}}"#,
            r#"{"status":"running","input":{"cmd":"ls"},"title":"ls"}"#,
            r#"{"status":"completed","input":{"cmd":"ls"},"output":"a\nb","title":"ls"}"#,
            r#"{"status":"error","input":{"cmd":"ls"},"error":"boom"}"#,
        ] {
            let _: ToolState =
                serde_json::from_str(raw).unwrap_or_else(|e| panic!("parse fail for {raw}: {e}"));
        }
    }
}
