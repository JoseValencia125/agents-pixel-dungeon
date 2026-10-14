//! The contract every harness adapter honours, and the adapters: a
//! deterministic mock for tests and Kiro over ACP for real work. Guild
//! persists the event stream itself; it never relies on the provider
//! replaying it.

pub mod kiro_acp;
pub mod mock;

use crate::domain::{HarnessKind, ModelOption, PermissionOption};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support { Supported, Unsupported, Unknown }

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSwitch { BetweenTurns, NewSession, Unsupported, Unknown }

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport { Acp, Stdio, Pane, Mock }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Capabilities {
    pub adapter_version: String,
    pub runtime_version: String,
    pub protocol_version: Option<String>,
    pub transport: Transport,
    pub create_session: Support,
    pub resume_session: Support,
    pub cancel_turn: Support,
    pub structured_events: Support,
    pub permission_requests: Support,
    pub model_discovery: Support,
    pub model_switch: ModelSwitch,
    pub usage_reporting: Support,
    pub native_fork: Support,
    pub max_concurrent_sessions: Option<u32>,
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
}

#[derive(Clone, Debug)]
pub struct SessionRef {
    pub id: String,
    pub native_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SessionOptions {
    pub cwd: PathBuf,
    pub model_id: Option<String>,
    pub mode: Option<String>,
    pub role_prompt: String,
}

/// What an adapter reports while a turn runs. `Tool` carries the room
/// action it maps to (read, forge, brew, summon, plan, type, gems).
#[derive(Clone, Debug, PartialEq)]
pub enum HarnessEvent {
    /// A chunk of the agent's visible message.
    Text(String),
    /// A chunk of the agent's reasoning, kept only as telemetry.
    Thought(String),
    Tool { id: String, title: String, kind: String, status: String, action: String },
    Plan(Value),
    ModelChanged { effective: String, reason: String },
    PermissionRequested { request_id: String, title: String, detail: String, options: Vec<PermissionOption> },
    /// Anything else the provider sent, verbatim.
    Raw(Value),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TurnOutcome { Completed { stop_reason: String }, Failed { reason: String }, Cancelled }

pub struct TurnHandle {
    pub turn_id: String,
    pub events: Receiver<HarnessEvent>,
    pub result: Receiver<TurnOutcome>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HarnessError {
    AuthRequired(String),
    RateLimited { retry_after_seconds: Option<u64> },
    ModelUnavailable(String),
    PermissionDenied(String),
    TransportLost(String),
    Timeout,
    UnsupportedCapability(&'static str),
    InvalidOutput(String),
    RuntimeFailed(String),
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            HarnessError::AuthRequired(s) => write!(f, "AUTH_REQUIRED: {s}"),
            HarnessError::RateLimited { retry_after_seconds } => write!(f, "RATE_LIMITED (retry after {retry_after_seconds:?} s)"),
            HarnessError::ModelUnavailable(s) => write!(f, "MODEL_UNAVAILABLE: {s}"),
            HarnessError::PermissionDenied(s) => write!(f, "PERMISSION_DENIED: {s}"),
            HarnessError::TransportLost(s) => write!(f, "TRANSPORT_LOST: {s}"),
            HarnessError::Timeout => write!(f, "TIMEOUT"),
            HarnessError::UnsupportedCapability(s) => write!(f, "UNSUPPORTED_CAPABILITY: {s}"),
            HarnessError::InvalidOutput(s) => write!(f, "INVALID_OUTPUT: {s}"),
            HarnessError::RuntimeFailed(s) => write!(f, "RUNTIME_FAILED: {s}"),
        }
    }
}

pub type HarnessResult<T> = Result<T, HarnessError>;

pub trait HarnessAdapter: Send + Sync {
    fn kind(&self) -> HarnessKind;
    /// Find the runtime and what it supports, without creating a session.
    fn probe(&self) -> HarnessResult<Capabilities>;
    fn create_session(&self, options: &SessionOptions) -> HarnessResult<SessionRef>;
    fn prompt(&self, session: &SessionRef, request_id: &str, text: &str) -> HarnessResult<TurnHandle>;
    fn interrupt(&self, session: &SessionRef) -> HarnessResult<()>;
    /// Close whatever the adapter started for this session. True when
    /// the runtime confirmed it ended.
    fn stop(&self, session: &SessionRef) -> HarnessResult<bool>;
    /// Models the harness announces; needs a live session on some.
    fn list_models(&self, session: Option<&SessionRef>) -> HarnessResult<Vec<ModelOption>>;
    /// Ask for a model between turns; returns the effective id.
    fn set_model(&self, session: &SessionRef, model_id: &str) -> HarnessResult<String>;
    fn respond_permission(&self, session: &SessionRef, request_id: &str, option_id: Option<&str>) -> HarnessResult<()>;
}

/// The room action a tool kind maps to, shared by every adapter.
pub fn action_for_tool(kind: &str, title: &str) -> &'static str {
    let t = title.to_ascii_lowercase();
    match kind {
        "read" | "search" | "fetch" => "read",
        "edit" | "delete" | "move" => "forge",
        "execute" => "brew",
        "think" => "plan",
        _ if t.contains("subagent") || t.contains("delegate") || t.contains("agent") => "summon",
        _ if t.contains("read") || t.contains("grep") || t.contains("search") || t.contains("glob") || t.contains("list") => "read",
        _ if t.contains("write") || t.contains("edit") || t.contains("patch") => "forge",
        _ if t.contains("shell") || t.contains("bash") || t.contains("command") || t.contains("execute") => "brew",
        _ if t.contains("plan") || t.contains("todo") => "plan",
        _ => "type",
    }
}
