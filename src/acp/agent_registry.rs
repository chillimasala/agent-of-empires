//! Named agent registry: maps an agent name (e.g. `claude-code`,
//! `aoe-agent`, `gemini`) to a spawn command + args. Users add agents via
//! the settings TUI; this module is the in-memory model.

use super::install_hints::install_hint_for;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Transport the agent uses to talk to its backend. Most agents are
/// `AcpStdio` (a fresh subprocess per session, JSON-RPC over stdin/stdout).
/// `OpencodeServer` is a single long-lived `opencode serve` process; each
/// session is a server-side `Session` addressed by id over HTTP+SSE. The
/// HTTP path trades one 300-400 MB subprocess per task for one shared
/// process with N sessions inside, which is the only way to scale to
/// many parallel tasks without OOM-ing the host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    /// Spawn `command args` per session and speak JSON-RPC over stdio.
    /// This is the historical default and what every non-opencode agent
    /// uses.
    #[default]
    AcpStdio,
    /// Connect to a long-lived `opencode serve` HTTP server. Sessions
    /// are created via `POST /session`, prompts sent via
    /// `POST /session/:id/prompt_async`, status streamed via the
    /// `/event` SSE endpoint. `opencode_url` and optionally
    /// `opencode_auth` must be set on the spec.
    OpencodeServer,
}

/// HTTP basic-auth credentials for an `OpencodeServer` transport.
/// Matches opencode's `OPENCODE_SERVER_USERNAME` / `OPENCODE_SERVER_PASSWORD`
/// env vars; the user is responsible for surfacing both ends in their
/// settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpAuth {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Executable to run, e.g. `npx` or `/usr/local/bin/aoe-agent`.
    /// Ignored when `kind = OpencodeServer`.
    pub command: String,
    pub args: Vec<String>,
    /// Human-readable description shown in the settings TUI and
    /// `aoe acp agents`.
    pub description: String,
    /// Optional: which env vars from aoe to forward to this agent. If
    /// `None`, only `PATH`, `HOME`, `LANG`, `TERM`, and provider auth env
    /// (e.g. `ANTHROPIC_API_KEY`) are forwarded.
    /// Ignored when `kind = OpencodeServer`.
    pub env_allowlist: Option<Vec<String>>,
    /// Worker transport. Defaults to `AcpStdio` so persisted configs
    /// from before the `OpencodeServer` variant existed keep
    /// deserializing unchanged.
    #[serde(default)]
    pub kind: AgentKind,
    /// Base URL of the `opencode serve` instance, e.g.
    /// `http://127.0.0.1:4096`. Required when `kind = OpencodeServer`;
    /// ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode_url: Option<String>,
    /// Optional HTTP basic-auth for the opencode server. Ignored when
    /// `kind != OpencodeServer`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode_auth: Option<HttpAuth>,
}

impl AgentSpec {
    /// Build an ACP `AgentSpec` from a custom agent's `agent_acp_cmd`
    /// string. The string is split with shell-word rules into argv and run
    /// directly (no shell). Returns a user-facing error message when the
    /// command is empty or has malformed quoting.
    pub fn from_acp_cmd(name: &str, cmd: &str) -> Result<AgentSpec, String> {
        let argv = shell_words::split(cmd).map_err(|e| {
            format!("custom agent `{name}` has a malformed structured view command ({e})")
        })?;
        let mut argv = argv.into_iter();
        let command = argv
            .next()
            .filter(|c| !c.trim().is_empty())
            .ok_or_else(|| format!("custom agent `{name}` has an empty structured view command"))?;
        Ok(AgentSpec {
            command,
            args: argv.collect(),
            description: format!("Custom ACP agent `{name}`"),
            env_allowlist: None,
            kind: AgentKind::AcpStdio,
            opencode_url: None,
            opencode_auth: None,
        })
    }

    /// True when this spec is the long-lived `opencode serve` transport
    /// rather than the per-session stdio spawn. Callers branch on this
    /// to pick the right worker substrate (`HttpServer` vs stdio).
    pub fn is_opencode_server(&self) -> bool {
        matches!(self.kind, AgentKind::OpencodeServer)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRegistry {
    pub agents: HashMap<String, AgentSpec>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a registry seeded with one entry per aoe tool that has
    /// a published ACP server, plus our own `aoe-agent` as a generic
    /// multi-provider fallback. Each entry is keyed on the same name
    /// the tmux view uses (claude / opencode / gemini / codex /
    /// vibe / pi) so the spawn path can map `instance.tool` directly
    /// to a registry key.
    ///
    /// Sources verified against
    /// <https://agentclientprotocol.com/get-started/agents.md>
    /// (Jan 2026):
    ///
    ///   claude   → claude-agent-acp     (Zed adapter for Claude SDK)
    ///   opencode → `opencode acp`       (native, SST)
    ///   gemini   → `gemini --acp`       (native, Google)
    ///   codex    → codex-acp            (ACP adapter, OpenAI Codex CLI)
    ///   vibe     → vibe-acp             (native, Mistral)
    ///   pi       → pi-acp               (adapter, Pi coding agent)
    ///
    /// We deliberately don't use `npx -y` for these. First-run
    /// downloads can hang for tens of seconds with no output, which
    /// used to leave the structured view worker silently wedged before the
    /// handshake. `aoe acp doctor --fix` can install missing
    /// binaries on demand.
    pub fn with_defaults() -> Self {
        let mut reg = Self::new();

        let claude_install = install_hint_for("claude-agent-acp").unwrap_or("(see project docs)");
        reg.agents.insert(
            "claude".into(),
            AgentSpec {
                command: "claude-agent-acp".into(),
                args: vec![],
                description: format!(
                    "Anthropic Claude via the official ACP adapter ({claude_install})"
                ),
                env_allowlist: None,
                kind: AgentKind::AcpStdio,
                opencode_url: None,
                opencode_auth: None,
            },
        );
        // Legacy alias used by older session records before the
        // tool-keyed naming. Kept so persisted sessions with
        // agent_name="claude-code" still resolve.
        reg.agents.insert(
            "claude-code".into(),
            AgentSpec {
                command: "claude-agent-acp".into(),
                args: vec![],
                description: "Alias for `claude` (legacy name)".into(),
                env_allowlist: None,
                kind: AgentKind::AcpStdio,
                opencode_url: None,
                opencode_auth: None,
            },
        );
        reg.agents.insert(
            "opencode".into(),
            AgentSpec {
                command: "opencode".into(),
                args: vec!["acp".into()],
                description: "OpenCode (SST) — native ACP via `opencode acp`".into(),
                env_allowlist: None,
                kind: AgentKind::AcpStdio,
                opencode_url: None,
                opencode_auth: None,
            },
        );
        // Memory-efficient opencode transport: one long-lived
        // `opencode serve` process, N sessions inside it. Each
        // session costs near-zero extra memory instead of the ~300
        // MB a fresh `opencode acp` subprocess would. URL is filled
        // in at spawn time from the `[acp.opencode_server]` settings
        // block; the placeholder here only documents the entry.
        reg.agents.insert(
            "opencode-server".into(),
            AgentSpec {
                command: String::new(),
                args: vec![],
                description: "OpenCode via a long-lived `opencode serve` HTTP server (low-memory, multi-session)"
                    .into(),
                env_allowlist: None,
                kind: AgentKind::OpencodeServer,
                opencode_url: Some("http://127.0.0.1:4096".into()),
                opencode_auth: None,
            },
        );
        reg.agents.insert(
            "gemini".into(),
            AgentSpec {
                command: "gemini".into(),
                args: vec!["--acp".into()],
                description: "Google Gemini CLI — native ACP via `gemini --acp`".into(),
                env_allowlist: None,
                kind: AgentKind::AcpStdio,
                opencode_url: None,
                opencode_auth: None,
            },
        );
        reg.agents.insert(
            "codex".into(),
            AgentSpec {
                command: "codex-acp".into(),
                args: vec![],
                description:
                    "OpenAI Codex CLI via ACP adapter (npm i -g @agentclientprotocol/codex-acp@latest)".into(),
                env_allowlist: None,
                kind: AgentKind::AcpStdio,
                opencode_url: None,
                opencode_auth: None,
            },
        );
        reg.agents.insert(
            "vibe".into(),
            AgentSpec {
                command: "vibe-acp".into(),
                args: vec![],
                description: "Mistral Vibe — native ACP via the bundled `vibe-acp` binary".into(),
                env_allowlist: None,
                kind: AgentKind::AcpStdio,
                opencode_url: None,
                opencode_auth: None,
            },
        );
        reg.agents.insert(
            "pi".into(),
            AgentSpec {
                command: "pi-acp".into(),
                args: vec![],
                description: "Pi coding agent (`pi`) via the pi-acp adapter (npm i -g pi-acp)"
                    .into(),
                env_allowlist: None,
                kind: AgentKind::AcpStdio,
                opencode_url: None,
                opencode_auth: None,
            },
        );
        reg.agents.insert(
            "aoe-agent".into(),
            AgentSpec {
                command: "${aoe_data_dir}/acp-worker/dist/aoe-agent".into(),
                args: vec![],
                description: "aoe's bundled multi-provider agent (Vercel AI SDK 6)".into(),
                env_allowlist: None,
                kind: AgentKind::AcpStdio,
                opencode_url: None,
                opencode_auth: None,
            },
        );
        reg
    }

    pub fn get(&self, name: &str) -> Option<&AgentSpec> {
        self.agents.get(name)
    }

    pub fn upsert(&mut self, name: String, spec: AgentSpec) {
        self.agents.insert(name, spec);
    }

    pub fn remove(&mut self, name: &str) -> Option<AgentSpec> {
        self.agents.remove(name)
    }

    pub fn list(&self) -> Vec<(&String, &AgentSpec)> {
        let mut entries: Vec<_> = self.agents.iter().collect();
        entries.sort_by_key(|(n, _)| n.as_str());
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_include_claude_code_and_aoe_agent() {
        let reg = AgentRegistry::with_defaults();
        assert!(reg.get("claude-code").is_some());
        assert!(reg.get("aoe-agent").is_some());
    }

    #[test]
    fn from_acp_cmd_splits_argv() {
        let spec = AgentSpec::from_acp_cmd("oc-sp", "ocp run sp acp").unwrap();
        assert_eq!(spec.command, "ocp");
        assert_eq!(spec.args, vec!["run", "sp", "acp"]);
        assert_eq!(spec.description, "Custom ACP agent `oc-sp`");
        assert!(spec.env_allowlist.is_none());
    }

    #[test]
    fn from_acp_cmd_honors_quoting() {
        let spec = AgentSpec::from_acp_cmd("wrap", "sh -lc 'ocp run sp acp'").unwrap();
        assert_eq!(spec.command, "sh");
        assert_eq!(spec.args, vec!["-lc", "ocp run sp acp"]);
    }

    #[test]
    fn from_acp_cmd_rejects_empty() {
        assert!(AgentSpec::from_acp_cmd("x", "").is_err());
        assert!(AgentSpec::from_acp_cmd("x", "   ").is_err());
    }

    #[test]
    fn from_acp_cmd_rejects_unbalanced_quotes() {
        assert!(AgentSpec::from_acp_cmd("x", "ocp run \"unterminated").is_err());
    }

    #[test]
    fn defaults_seed_opencode_server_entry() {
        let reg = AgentRegistry::with_defaults();
        let spec = reg
            .get("opencode-server")
            .expect("default registry should seed `opencode-server`");
        assert!(
            spec.is_opencode_server(),
            "opencode-server spec should advertise OpencodeServer transport"
        );
        assert_eq!(spec.kind, AgentKind::OpencodeServer);
        assert_eq!(spec.opencode_url.as_deref(), Some("http://127.0.0.1:4096"));
        assert!(
            spec.opencode_auth.is_none(),
            "no auth by default; user opts in via [acp.opencode_server] settings"
        );
        // The stdio command/args are vestigial for the HTTP path but must
        // be present and empty so struct construction stays total.
        assert!(spec.command.is_empty());
        assert!(spec.args.is_empty());
    }

    #[test]
    fn from_acp_cmd_keeps_legacy_acp_stdio_kind() {
        // Custom agents added via `aoe acp agents` keep the stdio transport
        // even after AgentKind::OpencodeServer lands; this is the load-bearing
        // assumption for backward compat.
        let spec = AgentSpec::from_acp_cmd("legacy", "legacy-acp").unwrap();
        assert_eq!(spec.kind, AgentKind::AcpStdio);
        assert!(!spec.is_opencode_server());
    }

    #[test]
    fn agent_spec_roundtrips_without_opencode_fields() {
        // Pre-AgentKind JSON in user config files must keep deserialising.
        // We strip the new optional fields and confirm the struct survives.
        let legacy = r#"{
            "command": "claude-agent-acp",
            "args": [],
            "description": "legacy",
            "env_allowlist": null
        }"#;
        let spec: AgentSpec = serde_json::from_str(legacy).unwrap();
        assert_eq!(spec.command, "claude-agent-acp");
        assert_eq!(spec.kind, AgentKind::AcpStdio);
        assert!(spec.opencode_url.is_none());
        assert!(spec.opencode_auth.is_none());
    }

    #[test]
    fn agent_spec_serializes_opencode_server_compactly() {
        // opencode_url and opencode_auth are skip_serializing_if None so
        // stdio agents don't bloat their config representation.
        let spec = AgentSpec {
            command: "opencode".into(),
            args: vec!["acp".into()],
            description: "stdio".into(),
            env_allowlist: None,
            kind: AgentKind::AcpStdio,
            opencode_url: None,
            opencode_auth: None,
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(
            !json.contains("opencode_url"),
            "opencode_url must be skipped when None, got: {json}"
        );
        assert!(
            !json.contains("opencode_auth"),
            "opencode_auth must be skipped when None, got: {json}"
        );
        // ...and the OpencodeServer variant round-trips with the field present.
        let server_spec = AgentSpec {
            command: String::new(),
            args: vec![],
            description: "server".into(),
            env_allowlist: None,
            kind: AgentKind::OpencodeServer,
            opencode_url: Some("http://127.0.0.1:4096".into()),
            opencode_auth: Some(HttpAuth {
                username: "opencode".into(),
                password: "secret".into(),
            }),
        };
        let json = serde_json::to_string(&server_spec).unwrap();
        assert!(json.contains("opencode_url"));
        assert!(json.contains("opencode_auth"));
        let roundtrip: AgentSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.kind, AgentKind::OpencodeServer);
        assert_eq!(
            roundtrip.opencode_auth,
            Some(HttpAuth {
                username: "opencode".into(),
                password: "secret".into()
            })
        );
    }

    #[test]
    fn list_is_sorted() {
        let mut reg = AgentRegistry::new();
        reg.upsert(
            "zeta".into(),
            AgentSpec {
                command: "z".into(),
                args: vec![],
                description: "z".into(),
                env_allowlist: None,
                kind: AgentKind::AcpStdio,
                opencode_url: None,
                opencode_auth: None,
            },
        );
        reg.upsert(
            "alpha".into(),
            AgentSpec {
                command: "a".into(),
                args: vec![],
                description: "a".into(),
                env_allowlist: None,
                kind: AgentKind::AcpStdio,
                opencode_url: None,
                opencode_auth: None,
            },
        );
        let names: Vec<&str> = reg.list().iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }
}
