//! Version 1 backend-neutral proxy contract. IDs are opaque and machine scoped.
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangesRequest {
    pub version: u32,
    pub token: String,
    pub machine: String,
    pub after: u64,
    pub timeout_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Changes {
    pub revision: u64,
    pub event_updates: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    #[default]
    Tmux,
    Herdr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub backend: Backend,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<AgentSummary>,
}

/// Read-only cross-machine discovery, independent of the selected terminal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MachineSessions {
    pub machine: String,
    pub sessions: Vec<Session>,
    #[serde(default)]
    pub tabs: Vec<Tab>,
    #[serde(default)]
    pub panes: Vec<Pane>,
    #[serde(default)]
    pub agent_panes: Vec<Pane>,
    pub loading: bool,
    pub unavailable: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    pub id: String,
    pub session: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<AgentSummary>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pane {
    pub id: String,
    pub tab: String,
    pub session: String,
    pub command: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Agent>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Working,
    Blocked,
    Done,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub name: String,
    pub state: AgentState,
    /// "herdr" is backend-reported; "command" is identity-only detection.
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    pub state: AgentState,
    pub count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Selection {
    pub session: String,
    pub tab: String,
    pub pane: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub machine: String,
    pub machines: Vec<Machine>,
    pub sessions: Vec<Session>,
    pub tabs: Vec<Tab>,
    pub panes: Vec<Pane>,
    pub selection: Selection,
    pub terminal: crate::status::Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Action {
    Snapshot,
    Select {
        #[serde(default)]
        session: String,
        #[serde(default)]
        tab: String,
        #[serde(default)]
        pane: String,
    },
    Text {
        text: String,
        /// When true, wrap the payload in CSI 200~/201~ so the pane treats it
        /// as a paste (fish/bash skip quote and bracket autopair).
        #[serde(default)]
        paste: bool,
    },
    Key {
        key: String,
    },
    CreateSession {
        name: String,
    },
    CreateTab {
        cwd: Option<String>,
    },
    CloseTab,
    NextTab {
        previous: bool,
    },
    Detach,
    CopyEnter,
    CopyKey {
        key: String,
    },
    CopySearch {
        query: String,
        backwards: bool,
    },
    ListFiles {
        path: Option<String>,
    },
    /// Read-only health of the proxy and every configured machine.
    Diagnose,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagnoseCheck {
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostDiagnose {
    pub id: String,
    pub name: String,
    pub backend: Backend,
    pub ok: bool,
    pub ssh: DiagnoseCheck,
    pub service: DiagnoseCheck,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagnoseReport {
    /// True while the proxy is still probing hosts.
    #[serde(default)]
    pub running: bool,
    pub proxy: DiagnoseCheck,
    #[serde(default)]
    pub hosts: Vec<HostDiagnose>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub token: String,
    #[serde(default)]
    pub machine: String,
    /// The expected pane prevents delayed input reaching a newly selected tab.
    #[serde(default)]
    pub expected_pane: Option<String>,
    pub action: Action,
}
