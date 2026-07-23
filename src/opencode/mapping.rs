//! Translation between opencode's SSE event shapes and the rest of
//! AoE.
//!
//! This module owns the **structural** conversions that don't need
//! AoE's `AcpState::Event` enum (those live in
//! `src/acp/opencode_bridge.rs` to keep the `acp` module's
//! `#[cfg(feature = "serve")]` gate clean and to keep this
//! `opencode` module free of upward deps). For now, the only
//! shape-mapper that doesn't need `AcpState` is the Todo conversion,
//! because `Todo` is a shared, simple record.

use super::types::Todo as OpencodeTodo;

/// Convert an opencode `Todo` (with `content` + `status` + `priority`)
/// into the `Todo` shape that `AcpState` (and the structured-view
/// TUI/Web renderers) already understand. The shape is intentionally
/// simple: `{ id, text, completed }`. We drop the `priority` field
/// for now; the structured view doesn't render it and adding it
/// would be a breaking change to `AcpState::Todo`.
//
// `AcpState::Todo` is defined in `src/acp/state.rs`. We duplicate
// the shape here to avoid a `cfg(feature = "serve")` dep from the
// `opencode` module. The `src/acp/opencode_bridge.rs` (added in a
// follow-up) is responsible for mapping `OpencodeTodo` into the
// real `AcpState::Todo` once both modules are wired together.
pub fn opencode_todo_to_acp_todo(t: OpencodeTodo) -> AcpShapeTodo {
    AcpShapeTodo {
        id: t.id,
        text: t.content,
        completed: matches!(t.status.as_str(), "completed"),
    }
}

/// Mirror of `acp::state::Todo` so the opencode module can produce
/// the right shape without `cfg(feature = "serve")` coupling.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AcpShapeTodo {
    pub id: String,
    pub text: String,
    pub completed: bool,
}

/// Result of [`map_event`]. Carries the structurally-mapped Todo
/// list (the only conversion that doesn't need `AcpState`); the
/// broader event-bridge to `AcpState::Event` lives in
/// `src/acp/opencode_bridge.rs` and uses this as its input.
pub struct OpencodeEventToAcp {
    pub session_id: String,
    pub todos: Option<Vec<AcpShapeTodo>>,
    /// `true` when the opencode session went idle (translates to
    /// `Event::Stopped { reason: "session.idle" }` in the bridge).
    pub went_idle: bool,
}

/// Lightweight structural inspection of an opencode `Event`. Returns
/// `None` for events the bridge doesn't need to look at (most of
/// them; the heavy lifting happens in `opencode_bridge`).
pub fn map_event(ev: &super::types::Event) -> Option<OpencodeEventToAcp> {
    use super::types::Event as E;
    match ev {
        E::SessionIdle { properties } => Some(OpencodeEventToAcp {
            session_id: properties.session_id.clone(),
            todos: None,
            went_idle: true,
        }),
        E::SessionStatus { properties } => {
            if matches!(properties.status, super::types::SessionStatus::Idle) {
                Some(OpencodeEventToAcp {
                    session_id: properties.session_id.clone(),
                    todos: None,
                    went_idle: true,
                })
            } else {
                None
            }
        }
        E::TodoUpdated { properties } => Some(OpencodeEventToAcp {
            session_id: properties.session_id.clone(),
            todos: Some(
                properties
                    .todos
                    .iter()
                    .cloned()
                    .map(opencode_todo_to_acp_todo)
                    .collect(),
            ),
            went_idle: false,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opencode::types::{Event, SessionStatus, TodoUpdatedProperties};

    fn todo(id: &str, content: &str, status: &str) -> super::super::types::Todo {
        super::super::types::Todo {
            id: id.into(),
            content: content.into(),
            status: status.into(),
            priority: "medium".into(),
        }
    }

    #[test]
    fn maps_completed_todo() {
        let t = todo("t1", "write tests", "completed");
        let mapped = opencode_todo_to_acp_todo(t);
        assert_eq!(mapped.id, "t1");
        assert_eq!(mapped.text, "write tests");
        assert!(mapped.completed);
    }

    #[test]
    fn maps_pending_todo() {
        let t = todo("t1", "ship it", "pending");
        let mapped = opencode_todo_to_acp_todo(t);
        assert!(!mapped.completed);
    }

    #[test]
    fn maps_in_progress_todo_as_not_completed() {
        // The structured view treats anything non-completed as
        // "still in flight", so in_progress is "not completed" too.
        let t = todo("t1", "wip", "in_progress");
        let mapped = opencode_todo_to_acp_todo(t);
        assert!(!mapped.completed);
    }

    #[test]
    fn map_event_extracts_todo_list() {
        let ev = Event::TodoUpdated {
            properties: TodoUpdatedProperties {
                session_id: "s1".into(),
                todos: vec![todo("a", "do x", "pending"), todo("b", "do y", "completed")],
            },
        };
        let m = map_event(&ev).expect("TodoUpdated should map");
        assert_eq!(m.session_id, "s1");
        let todos = m.todos.unwrap();
        assert_eq!(todos.len(), 2);
        assert_eq!(todos[0].text, "do x");
        assert!(!todos[0].completed);
        assert!(todos[1].completed);
        assert!(!m.went_idle);
    }

    #[test]
    fn map_event_session_idle_sets_flag() {
        use crate::opencode::types::SessionIdProperty;
        let ev = Event::SessionIdle {
            properties: SessionIdProperty {
                session_id: "s1".into(),
            },
        };
        let m = map_event(&ev).unwrap();
        assert_eq!(m.session_id, "s1");
        assert!(m.went_idle);
        assert!(m.todos.is_none());
    }

    #[test]
    fn map_event_session_status_idle_sets_flag() {
        use crate::opencode::types::SessionStatusProperties;
        let ev = Event::SessionStatus {
            properties: SessionStatusProperties {
                session_id: "s1".into(),
                status: SessionStatus::Idle,
            },
        };
        let m = map_event(&ev).unwrap();
        assert!(m.went_idle);
    }

    #[test]
    fn map_event_session_status_busy_returns_none() {
        // Busy doesn't need special handling; the tool/text events
        // themselves drive activity in the structured view.
        use crate::opencode::types::SessionStatusProperties;
        let ev = Event::SessionStatus {
            properties: SessionStatusProperties {
                session_id: "s1".into(),
                status: SessionStatus::Busy,
            },
        };
        assert!(map_event(&ev).is_none());
    }
}
