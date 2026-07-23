//! Stateful translation from OpenCode's global SSE events into AoE's
//! structured-view event model.

use std::collections::{HashMap, HashSet};

use chrono::Utc;

use crate::opencode::types::{Event as OpenCodeEvent, GlobalEvent, Part, SessionStatus, ToolState};

use super::approvals::{ApprovalDecision, Nonce};
use super::permissions::build_approval;
use super::state::{Event, Todo, ToolCall};

/// Per-OpenCode-session mapping state. OpenCode part updates are full
/// snapshots, while AoE transcript events are append/lifecycle events,
/// so the bridge remembers prior text and completed tools.
pub struct OpenCodeEventMapper {
    session_id: String,
    assistant_messages: HashSet<String>,
    text_snapshots: HashMap<String, String>,
    started_tools: HashSet<String>,
    completed_tools: HashSet<String>,
    nonce_to_permission: HashMap<Nonce, String>,
    permission_to_nonce: HashMap<String, Nonce>,
    idle_emitted: bool,
    thinking: bool,
}

impl OpenCodeEventMapper {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            assistant_messages: HashSet::new(),
            text_snapshots: HashMap::new(),
            started_tools: HashSet::new(),
            completed_tools: HashSet::new(),
            nonce_to_permission: HashMap::new(),
            permission_to_nonce: HashMap::new(),
            idle_emitted: false,
            thinking: false,
        }
    }

    /// Translate one global event. Events for other sessions are ignored.
    pub fn map(&mut self, event: GlobalEvent) -> Vec<Event> {
        if event
            .payload
            .session_id()
            .is_some_and(|id| id != self.session_id)
        {
            return Vec::new();
        }

        match event.payload {
            OpenCodeEvent::MessageUpdated { properties } => {
                if properties.info.role == "assistant" {
                    self.assistant_messages.insert(properties.info.id.clone());
                    self.idle_emitted = false;
                    if properties.info.error.is_some() {
                        return vec![
                            Event::PromptRuntimeError {
                                message: "OpenCode assistant turn failed".into(),
                            },
                            Event::Stopped {
                                reason: "error".into(),
                            },
                        ];
                    }
                }
                Vec::new()
            }
            OpenCodeEvent::MessageRemoved { properties } => {
                self.assistant_messages.remove(&properties.message_id);
                Vec::new()
            }
            OpenCodeEvent::MessagePartUpdated { properties } => self.map_part(properties.part),
            OpenCodeEvent::TodoUpdated { properties } => vec![Event::TodoListUpdated {
                todos: properties
                    .todos
                    .into_iter()
                    .enumerate()
                    .map(|(index, todo)| Todo {
                        id: if todo.id.is_empty() {
                            format!("opencode-todo-{index}")
                        } else {
                            todo.id
                        },
                        text: todo.content,
                        completed: todo.status == "completed",
                    })
                    .collect(),
            }],
            OpenCodeEvent::PermissionAsked { properties } => {
                if self.permission_to_nonce.contains_key(&properties.id) {
                    return Vec::new();
                }
                let call_id = properties
                    .tool
                    .as_ref()
                    .map(|tool| tool.call_id.clone())
                    .unwrap_or_else(|| properties.id.clone());
                let tool_call = ToolCall {
                    id: call_id.clone(),
                    name: properties.permission.clone(),
                    kind: tool_kind(&properties.permission).into(),
                    args_preview: preview_json(&properties.metadata),
                    started_at: Utc::now(),
                    parent_tool_call_id: None,
                    memory_recall: None,
                    diffs: Vec::new(),
                };
                let approval = build_approval(tool_call.clone());
                self.nonce_to_permission
                    .insert(approval.nonce.clone(), properties.id.clone());
                self.permission_to_nonce
                    .insert(properties.id, approval.nonce.clone());

                let mut events = Vec::new();
                if self.started_tools.insert(call_id) {
                    events.push(Event::ToolCallStarted { tool_call });
                }
                events.push(Event::ApprovalRequested { approval });
                events
            }
            OpenCodeEvent::PermissionReplied { properties } => {
                let Some(nonce) = self.permission_to_nonce.remove(&properties.request_id) else {
                    return Vec::new();
                };
                self.nonce_to_permission.remove(&nonce);
                vec![Event::ApprovalResolved {
                    nonce,
                    decision: response_to_decision(&properties.reply),
                }]
            }
            OpenCodeEvent::SessionIdle { .. } => self.stop_once("prompt_complete"),
            OpenCodeEvent::SessionStatus { properties } => match properties.status {
                SessionStatus::Idle => self.stop_once("prompt_complete"),
                SessionStatus::Busy => {
                    self.idle_emitted = false;
                    Vec::new()
                }
                SessionStatus::Retry { message, .. } => {
                    self.stop_once(&format!("retry: {message}"))
                }
            },
            OpenCodeEvent::SessionCompacted { .. } => vec![Event::ConversationCompacted],
            OpenCodeEvent::SessionError { properties } => vec![
                Event::PromptRuntimeError {
                    message: properties.to_string(),
                },
                Event::Stopped {
                    reason: "error".into(),
                },
            ],
            _ => Vec::new(),
        }
    }

    /// Resolve an AoE approval nonce to the OpenCode permission id.
    /// Removal makes the nonce single-use.
    pub fn take_permission_id(&mut self, nonce: &Nonce) -> Option<String> {
        let permission_id = self.nonce_to_permission.remove(nonce)?;
        self.permission_to_nonce.remove(&permission_id);
        Some(permission_id)
    }

    fn map_part(&mut self, part: Part) -> Vec<Event> {
        match part {
            Part::Text {
                id,
                message_id,
                text,
                ..
            } => {
                if !self.assistant_messages.contains(&message_id) {
                    return Vec::new();
                }
                let previous = self
                    .text_snapshots
                    .insert(id, text.clone())
                    .unwrap_or_default();
                let delta = text.strip_prefix(&previous).unwrap_or(&text);
                if delta.is_empty() {
                    Vec::new()
                } else {
                    vec![Event::AgentMessageChunk { text: delta.into() }]
                }
            }
            Part::Reasoning { id, text, .. } => {
                self.text_snapshots.insert(id, text);
                if self.thinking {
                    Vec::new()
                } else {
                    self.thinking = true;
                    vec![Event::ThinkingStarted]
                }
            }
            Part::StepFinish { .. } => {
                if self.thinking {
                    self.thinking = false;
                    vec![Event::ThinkingEnded]
                } else {
                    Vec::new()
                }
            }
            Part::Tool {
                call_id,
                tool,
                state,
                ..
            } => self.map_tool(call_id, tool, state),
            _ => Vec::new(),
        }
    }

    fn map_tool(&mut self, call_id: String, tool: String, state: ToolState) -> Vec<Event> {
        let (input, title, completion) = match state {
            ToolState::Pending { input } => (input, None, None),
            ToolState::Running { input, title } => (input, title, None),
            ToolState::Completed {
                input,
                output,
                title,
                ..
            } => (input, Some(title), Some((false, output))),
            ToolState::Error { input, error } => (input, None, Some((true, error))),
        };
        let tool_call = ToolCall {
            id: call_id.clone(),
            name: title.unwrap_or_else(|| tool.clone()),
            kind: tool_kind(&tool).into(),
            args_preview: preview_json(&input),
            started_at: Utc::now(),
            parent_tool_call_id: None,
            memory_recall: None,
            diffs: Vec::new(),
        };
        let mut events = Vec::new();
        if self.started_tools.insert(call_id.clone()) {
            events.push(Event::ToolCallStarted { tool_call });
        } else {
            events.push(Event::ToolCallUpdated {
                tool_call_id: call_id.clone(),
                title: Some(tool_call.name),
                args_preview: Some(tool_call.args_preview),
                started_at: None,
                diffs: None,
            });
        }
        if let Some((is_error, content)) = completion {
            if self.completed_tools.insert(call_id.clone()) {
                events.push(Event::ToolCallCompleted {
                    tool_call_id: call_id,
                    is_error,
                    content,
                    output: Vec::new(),
                    completed_at: Utc::now(),
                    async_subagent: false,
                });
            }
        }
        events
    }

    fn stop_once(&mut self, reason: &str) -> Vec<Event> {
        if self.idle_emitted {
            return Vec::new();
        }
        self.idle_emitted = true;
        let mut events = Vec::new();
        if self.thinking {
            self.thinking = false;
            events.push(Event::ThinkingEnded);
        }
        events.push(Event::Stopped {
            reason: reason.into(),
        });
        events
    }
}

fn preview_json(value: &impl serde::Serialize) -> String {
    let mut preview = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    if preview.len() > 16 * 1024 {
        preview.truncate(16 * 1024);
    }
    preview
}

fn tool_kind(tool: &str) -> &'static str {
    match tool.to_ascii_lowercase().as_str() {
        "bash" | "shell" | "terminal" => "execute",
        "read" | "readfile" => "read",
        "write" | "edit" | "apply_patch" => "edit",
        "grep" | "glob" | "search" => "search",
        "webfetch" | "fetch" => "fetch",
        "task" => "think",
        _ => "other",
    }
}

fn response_to_decision(response: &str) -> ApprovalDecision {
    match response {
        "once" => ApprovalDecision::Allow,
        "always" => ApprovalDecision::AllowAlways,
        _ => ApprovalDecision::Deny,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opencode::types::{
        Message, MessagePartUpdatedProperties, MessageUpdatedProperties, Permission,
        PermissionRepliedProperties, PermissionTool, SessionIdProperty,
    };

    fn global(payload: OpenCodeEvent) -> GlobalEvent {
        GlobalEvent {
            directory: Some("/tmp/project".into()),
            payload,
        }
    }

    fn assistant_message() -> OpenCodeEvent {
        OpenCodeEvent::MessageUpdated {
            properties: MessageUpdatedProperties {
                info: Message {
                    id: "msg-1".into(),
                    session_id: "ses-1".into(),
                    role: "assistant".into(),
                    tokens: None,
                    cost: None,
                    time: None,
                    error: None,
                },
            },
        }
    }

    #[test]
    fn text_snapshots_emit_only_new_suffix() {
        let mut mapper = OpenCodeEventMapper::new("ses-1");
        assert!(mapper.map(global(assistant_message())).is_empty());

        let text = |value: &str| {
            global(OpenCodeEvent::MessagePartUpdated {
                properties: MessagePartUpdatedProperties {
                    part: Part::Text {
                        id: "part-1".into(),
                        session_id: "ses-1".into(),
                        message_id: "msg-1".into(),
                        text: value.into(),
                    },
                },
            })
        };
        assert!(matches!(
            mapper.map(text("hello")).as_slice(),
            [Event::AgentMessageChunk { text }] if text == "hello"
        ));
        assert!(matches!(
            mapper.map(text("hello world")).as_slice(),
            [Event::AgentMessageChunk { text }] if text == " world"
        ));
        assert!(mapper.map(text("hello world")).is_empty());
    }

    #[test]
    fn tool_snapshots_start_once_and_complete_once() {
        let mut mapper = OpenCodeEventMapper::new("ses-1");
        let tool = |state| {
            global(OpenCodeEvent::MessagePartUpdated {
                properties: MessagePartUpdatedProperties {
                    part: Part::Tool {
                        id: "part-tool".into(),
                        session_id: "ses-1".into(),
                        message_id: "msg-1".into(),
                        call_id: "call-1".into(),
                        tool: "bash".into(),
                        state,
                    },
                },
            })
        };
        let started = mapper.map(tool(ToolState::Running {
            input: serde_json::json!({"command":"ls"}),
            title: Some("List files".into()),
        }));
        assert!(matches!(
            started.as_slice(),
            [Event::ToolCallStarted { .. }]
        ));

        let completed = mapper.map(tool(ToolState::Completed {
            input: serde_json::json!({"command":"ls"}),
            output: "a.rs".into(),
            title: "List files".into(),
            metadata: HashMap::new(),
        }));
        assert!(matches!(
            completed.as_slice(),
            [
                Event::ToolCallUpdated { .. },
                Event::ToolCallCompleted { .. }
            ]
        ));
    }

    #[test]
    fn duplicate_idle_status_is_suppressed() {
        let mut mapper = OpenCodeEventMapper::new("ses-1");
        let idle = global(OpenCodeEvent::SessionIdle {
            properties: SessionIdProperty {
                session_id: "ses-1".into(),
            },
        });
        assert!(matches!(
            mapper.map(idle.clone()).as_slice(),
            [Event::Stopped { .. }]
        ));
        assert!(mapper.map(idle).is_empty());
    }

    #[test]
    fn permission_roundtrip_tracks_nonce() {
        let mut mapper = OpenCodeEventMapper::new("ses-1");
        let events = mapper.map(global(OpenCodeEvent::PermissionAsked {
            properties: Permission {
                id: "perm-1".into(),
                session_id: "ses-1".into(),
                permission: "bash".into(),
                patterns: vec!["ls".into()],
                always: Vec::new(),
                metadata: HashMap::new(),
                tool: Some(PermissionTool {
                    message_id: "msg-1".into(),
                    call_id: "call-1".into(),
                }),
            },
        }));
        let nonce = match events.last().unwrap() {
            Event::ApprovalRequested { approval } => approval.nonce.clone(),
            other => panic!("expected approval, got {other:?}"),
        };
        assert_eq!(mapper.take_permission_id(&nonce).as_deref(), Some("perm-1"));

        // The local take removed both directions, so the echoed SSE
        // reply does not emit a duplicate ApprovalResolved.
        assert!(mapper
            .map(global(OpenCodeEvent::PermissionReplied {
                properties: PermissionRepliedProperties {
                    session_id: "ses-1".into(),
                    request_id: "perm-1".into(),
                    reply: "once".into(),
                },
            }))
            .is_empty());
    }

    #[test]
    fn ignores_other_sessions() {
        let mut mapper = OpenCodeEventMapper::new("ses-1");
        assert!(mapper
            .map(global(OpenCodeEvent::SessionIdle {
                properties: SessionIdProperty {
                    session_id: "ses-other".into(),
                },
            }))
            .is_empty());
    }
}
