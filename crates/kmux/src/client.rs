//! Conversion between existing WAF commands/status and the unified proxy API.
use crate::{
    api::*,
    status::{HostInfo, Status},
};
use serde_json::Value;

pub fn action(op: &Value) -> Result<Action, String> {
    let text = |key: &str| op[key].as_str().unwrap_or("").to_string();
    let path = |key: &str| op[key].as_str().map(String::from);
    Ok(match text("op").as_str() {
        "watch_start" | "list" => Action::Snapshot,
        "text" => Action::Text {
            text: text("text"),
            paste: op["paste"].as_bool().unwrap_or(false),
        },
        "key" => Action::Key { key: text("key") },
        "window" => Action::Select {
            session: text("session"),
            tab: text("index"),
            pane: String::new(),
        },
        "pane" => Action::Select {
            session: text("session"),
            tab: text("win"),
            pane: text("id"),
        },
        "session" => Action::Select {
            session: text("name"),
            tab: String::new(),
            pane: String::new(),
        },
        "host_session" => Action::Select {
            session: text("session"),
            tab: String::new(),
            pane: String::new(),
        },
        "host_pane" => {
            if ["id", "session", "tab", "pane"]
                .iter()
                .any(|key| text(key).is_empty())
            {
                return Err("machine, session, tab and pane are required".into());
            }
            Action::Select {
                session: text("session"),
                tab: text("tab"),
                pane: text("pane"),
            }
        }
        "new_session" => Action::CreateSession { name: text("name") },
        "new_window" => Action::CreateTab { cwd: path("cwd") },
        "kill_window" => Action::CloseTab,
        "next_window" => Action::NextTab { previous: false },
        "prev_window" => Action::NextTab { previous: true },
        "copy_mode" => Action::CopyEnter,
        "copy_search" => Action::CopySearch {
            query: text("query"),
            backwards: op["backwards"].as_bool().unwrap_or(false),
        },
        "list_files" => Action::ListFiles { path: path("path") },
        "diagnose" => Action::Diagnose,
        "detach" | "watch_stop" => Action::Detach,
        other => return Err(format!("unsupported command: {other}")),
    })
}

/// The HTTP API has typed inventories. Retain the WAF's existing row format
/// locally during its migration; backend commands never cross this boundary.
pub fn status(snapshot: Snapshot, previous: &Status) -> Status {
    let mut s = snapshot.terminal;
    s.scroll_revision = previous.scroll_revision;
    s.proxy_url = previous.proxy_url.clone();
    s.socks5 = previous.socks5.clone();
    s.diagnose = s.diagnose.clone().or(previous.diagnose.clone());
    s.clipboard_history = previous.clipboard_history.clone();
    if let Some(text) = s.clipboard.as_deref() {
        if previous.clipboard.as_deref() != Some(text) {
            if let Some(text) = crate::normalize_clipboard_text(text) {
                crate::remember_clipboard(&mut s.clipboard_history, text);
            }
        }
    } else {
        s.clipboard = previous.clipboard.clone();
    }
    if s.cwd.is_none() && previous.active_host.as_deref() == Some(&snapshot.machine) {
        s.cwd = previous.cwd.clone();
        s.files = previous.files.clone();
        s.files_truncated = previous.files_truncated;
    }
    s.scroll_offset = if s.copy_mode
        || Some(snapshot.selection.session.as_str()) != previous.session.as_deref()
    {
        0
    } else {
        previous.scroll_offset.min(s.scrollback.len())
    };
    s.session = Some(snapshot.selection.session.clone());
    s.session_names = snapshot
        .sessions
        .iter()
        .map(|v| (v.id.clone(), v.name.clone()))
        .collect();
    s.tab_names = snapshot
        .tabs
        .iter()
        .map(|v| (v.id.clone(), v.name.clone()))
        .collect();
    s.active_host = Some(snapshot.machine);
    s.tab_agents = snapshot
        .tabs
        .iter()
        .filter_map(|tab| tab.agents.clone().map(|agents| (tab.id.clone(), agents)))
        .collect();
    s.agent_panes = snapshot
        .panes
        .iter()
        .filter(|pane| pane.agent.is_some())
        .cloned()
        .collect();
    s.hosts = snapshot
        .machines
        .into_iter()
        .map(|m| HostInfo {
            id: m.id,
            name: m.name,
        })
        .collect();
    s.sessions = snapshot
        .sessions
        .iter()
        .map(|v| {
            format!(
                "S {} {}",
                v.id,
                if v.id == snapshot.selection.session {
                    1
                } else {
                    0
                }
            )
        })
        .collect();
    s.windows = snapshot
        .tabs
        .iter()
        .map(|v| {
            format!(
                "W {} {} {} {} [80x24]",
                v.session,
                v.id,
                v.name.split_whitespace().collect::<Vec<_>>().join("_"),
                if v.id == snapshot.selection.tab && v.session == snapshot.selection.session {
                    "*"
                } else {
                    "-"
                }
            )
        })
        .collect();
    s.panes = snapshot
        .panes
        .iter()
        .map(|v| {
            format!(
                "P {} {} {} 0 {} [80x24] {}",
                v.session,
                v.tab,
                v.id,
                if v.command.is_empty() {
                    "shell"
                } else {
                    &v.command
                },
                if v.id == snapshot.selection.pane {
                    1
                } else {
                    0
                }
            )
        })
        .collect();
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn routes_opaque_herdr_ids_without_tmux_commands() {
        assert!(
            matches!(action(&json!({"op":"window","session":"w1","index":"w1:t2"})).unwrap(),Action::Select{session,tab,..} if session=="w1"&&tab=="w1:t2")
        );
    }

    #[test]
    fn agent_switch_routes_exact_pane_without_input() {
        assert!(
            matches!(action(&json!({"op":"pane","session":"w1","win":"w1:t2","id":"w1:p3"})).unwrap(),
            Action::Select {session,tab,pane} if session == "w1" && tab == "w1:t2" && pane == "w1:p3")
        );
    }

    #[test]
    fn cross_host_agent_switch_requires_complete_target() {
        let op =
            json!({"op":"host_pane","id":"devbox","session":"w1","tab":"w1:t2","pane":"w1:p3"});
        assert!(
            matches!(action(&op).unwrap(), Action::Select {session,tab,pane}
            if session == "w1" && tab == "w1:t2" && pane == "w1:p3")
        );
        for field in ["id", "session", "tab", "pane"] {
            let mut missing = op.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(action(&missing).is_err());
        }
    }

    #[test]
    fn agent_metadata_reaches_waf_and_clears_on_exit() {
        let mut snapshot = Snapshot {
            version: VERSION,
            machine: "herdr".into(),
            machines: vec![],
            sessions: vec![],
            tabs: vec![Tab {
                id: "t1".into(),
                session: "s1".into(),
                name: "tab".into(),
                agents: Some(AgentSummary {
                    state: AgentState::Blocked,
                    count: 1,
                }),
            }],
            panes: vec![Pane {
                id: "p1".into(),
                tab: "t1".into(),
                session: "s1".into(),
                command: "codex".into(),
                cwd: "/tmp".into(),
                agent: Some(Agent {
                    name: "codex".into(),
                    state: AgentState::Blocked,
                    source: "herdr".into(),
                }),
            }],
            selection: Selection::default(),
            terminal: Status::default(),
        };
        let status = super::status(snapshot.clone(), &Status::default());
        assert_eq!(status.agent_panes[0].id, "p1");
        assert_eq!(status.tab_agents["t1"].state, AgentState::Blocked);
        snapshot.tabs[0].agents = None;
        snapshot.panes[0].agent = None;
        let cleared = super::status(snapshot, &status);
        assert!(cleared.agent_panes.is_empty() && cleared.tab_agents.is_empty());
    }

    #[test]
    fn selects_cross_host_session_without_a_default_tab_race() {
        assert!(
            matches!(action(&json!({"op":"host_session","id":"devbox","session":"w1"})).unwrap(),
            Action::Select {session, tab, pane} if session == "w1" && tab.is_empty() && pane.is_empty())
        );
    }

    #[test]
    fn snapshots_preserve_authoritative_local_scroll_revision() {
        let previous = Status {
            session: Some("s".into()),
            scroll_offset: 24,
            scroll_revision: 1234,
            proxy_url: "https://kmux.qingshan.dev".into(),
            ..Status::default()
        };
        let snapshot = Snapshot {
            version: VERSION,
            machine: "host".into(),
            machines: vec![],
            sessions: vec![],
            tabs: vec![],
            panes: vec![],
            selection: Selection {
                session: "s".into(),
                ..Selection::default()
            },
            terminal: Status {
                scrollback: vec![String::new(); 80],
                ..Status::default()
            },
        };
        let result = status(snapshot, &previous);
        assert_eq!((result.scroll_offset, result.scroll_revision), (24, 1234));
        assert_eq!(result.proxy_url, "https://kmux.qingshan.dev");
    }

    #[test]
    fn diagnose_command_is_read_only() {
        assert!(matches!(
            action(&json!({"op": "diagnose"})).unwrap(),
            Action::Diagnose
        ));
    }

    #[test]
    fn text_paste_flag_defaults_false() {
        assert!(matches!(
            action(&json!({"op": "text", "text": "hi"})).unwrap(),
            Action::Text { text, paste: false } if text == "hi"
        ));
        assert!(matches!(
            action(&json!({"op": "text", "text": "hi", "paste": true})).unwrap(),
            Action::Text { paste: true, .. }
        ));
    }

    #[test]
    fn polls_keep_the_last_diagnose_report() {
        let previous = Status {
            diagnose: Some(DiagnoseReport {
                running: false,
                proxy: DiagnoseCheck {
                    ok: true,
                    detail: "API v1, 1 machine".into(),
                },
                hosts: vec![],
            }),
            ..Status::default()
        };
        let snapshot = Snapshot {
            version: VERSION,
            machine: "host".into(),
            machines: vec![],
            sessions: vec![],
            tabs: vec![],
            panes: vec![],
            selection: Selection::default(),
            terminal: Status::default(),
        };
        let result = status(snapshot, &previous);
        assert_eq!(
            result.diagnose.as_ref().unwrap().proxy.detail,
            "API v1, 1 machine"
        );
    }
}
