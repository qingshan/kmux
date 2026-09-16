//! Serializable WAF status snapshot shared by the daemon and host-side tests.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub connected: bool,
    #[serde(default)]
    pub event_updates: bool,
    #[serde(default)]
    pub event_revision: u64,
    pub cols: u16,
    pub rows: u16,
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub screen: Vec<String>,
    pub attrs: Vec<String>,
    pub scrollback: Vec<String>,
    pub scroll_offset: usize,
    #[serde(default)]
    pub scroll_revision: u64,
    pub windows: Vec<String>,
    pub panes: Vec<String>,
    pub sessions: Vec<String>,
    #[serde(default)]
    pub session_catalog: Vec<crate::api::MachineSessions>,
    #[serde(default)]
    pub session_names: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub tab_names: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub tab_agents: std::collections::BTreeMap<String, crate::api::AgentSummary>,
    #[serde(default)]
    pub agent_panes: Vec<crate::api::Pane>,
    pub session: Option<String>,
    #[serde(default)]
    pub hosts: Vec<HostInfo>,
    #[serde(default)]
    pub active_host: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub files: Vec<FileInfo>,
    #[serde(default)]
    pub files_truncated: bool,
    #[serde(default)]
    pub copy_mode: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clipboard: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clipboard_history: Vec<String>,
    pub alt_screen: bool,
    pub updated_at: u64,
    pub last_error: Option<String>,
    pub configured: bool,
    /// Non-secret connection preferences shown in the WAF Settings dialog.
    #[serde(default)]
    pub proxy_url: String,
    #[serde(default)]
    pub socks5: String,
    /// Last Diagnose report from Settings; polls must not wipe it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnose: Option<crate::api::DiagnoseReport>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub name: String,
    pub dir: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub id: String,
    pub name: String,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            connected: false,
            event_updates: false,
            event_revision: 0,
            cols: 80,
            rows: 24,
            cursor_x: 0,
            cursor_y: 0,
            screen: Vec::new(),
            attrs: Vec::new(),
            scrollback: Vec::new(),
            scroll_offset: 0,
            scroll_revision: 0,
            windows: Vec::new(),
            panes: Vec::new(),
            sessions: Vec::new(),
            session_catalog: Vec::new(),
            session_names: Default::default(),
            tab_names: Default::default(),
            tab_agents: Default::default(),
            agent_panes: Vec::new(),
            session: None,
            hosts: Vec::new(),
            active_host: None,
            cwd: None,
            files: Vec::new(),
            files_truncated: false,
            copy_mode: false,
            clipboard: None,
            clipboard_history: Vec::new(),
            alt_screen: false,
            updated_at: 0,
            last_error: None,
            configured: false,
            proxy_url: String::new(),
            socks5: String::new(),
            diagnose: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_status_matches_the_waf_terminal_geometry() {
        let status = Status::default();
        assert_eq!((status.cols, status.rows), (80, 24));
        assert!(status.clipboard_history.is_empty());
    }
}
