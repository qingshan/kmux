//! Serialized WAF commands, configuration updates, and pane selection.
use super::{state::*, Op, CONFIG_PATH};
use kmuxd::api::Action;
use kmuxd::config;
use std::path::Path;
use std::sync::{atomic::Ordering, mpsc};

pub(crate) fn dispatch(rx: mpsc::Receiver<Op>) {
    let proxy = kmuxd::proxy::Client::new(Path::new(CONFIG_PATH));
    let mut selected_pane = None;
    for op in rx {
        let name = op["op"].as_str().unwrap_or("");
        let mut cfg = config::load(Path::new(CONFIG_PATH));
        match name {
            "clipboard_remember" => {
                if let Some(t) = op["text"]
                    .as_str()
                    .and_then(kmuxd::normalize_clipboard_text)
                {
                    set_status(|s| kmuxd::remember_clipboard(&mut s.clipboard_history, t));
                }
                continue;
            }
            "config_set" => {
                if let Some(value) = op["proxy_url"].as_str() {
                    cfg.proxy_url = value.into();
                }
                // An empty token means “keep the saved token” in the WAF.
                if let Some(value) = op["token"].as_str().filter(|v| !v.is_empty()) {
                    cfg.token = value.into();
                }
                if let Some(value) = op["socks5"].as_str() {
                    cfg.socks5 = value.into();
                }
                if let Err(e) = config::save(Path::new(CONFIG_PATH), &cfg) {
                    set_status(|s| s.last_error = Some(e));
                }
                set_status(|s| {
                    s.configured = !cfg.proxy_url.is_empty() && !cfg.token.is_empty();
                    s.proxy_url = cfg.proxy_url.clone();
                    s.socks5 = cfg.socks5.clone();
                });
                selected_pane = None;
                continue;
            }
            "host" | "host_session" | "host_pane" => {
                if name == "host_pane" {
                    if let Err(error) = kmuxd::client::action(&op) {
                        set_status(|s| s.last_error = Some(error));
                        continue;
                    }
                }
                cfg.active_host = op["id"].as_str().unwrap_or("").into();
                if let Err(e) = config::save(Path::new(CONFIG_PATH), &cfg) {
                    set_status(|s| s.last_error = Some(e));
                    continue;
                }
                selected_pane = None;
                set_status(|s| {
                    s.connected = false;
                    s.event_updates = false;
                    s.active_host = Some(cfg.active_host.clone());
                    s.screen.clear();
                    s.windows.clear();
                    s.panes.clear();
                    s.agent_panes.clear();
                    s.tab_agents.clear();
                    s.sessions.clear();
                    s.scrollback.clear();
                    s.copy_mode = false;
                    s.cwd = None;
                    s.files.clear();
                });
                WATCHING.store(true, Ordering::SeqCst);
            }
            "watch_start" => {
                WATCHING.store(true, Ordering::SeqCst);
            }
            "watch_stop" | "detach" => {
                WATCHING.store(false, Ordering::SeqCst);
                let _ = proxy.request(&cfg, Action::Detach, None);
                set_status(|s| {
                    s.connected = false;
                    s.copy_mode = false;
                });
                continue;
            }
            "scroll_to" => {
                let offset = op["offset"].as_u64().unwrap_or(0) as usize;
                set_status(|s| s.scroll_offset = offset.min(s.scrollback.len()));
                continue;
            }
            "scroll_page" => {
                let rows = (op["rows"].as_u64().unwrap_or(24) as usize).clamp(1, 72);
                set_status(|s| s.scroll_page = rows);
                continue;
            }
            "diagnose" => {
                if cfg.proxy_url.is_empty() || cfg.token.is_empty() {
                    set_status(|s| {
                        s.diagnose = Some(kmuxd::api::DiagnoseReport {
                            running: false,
                            proxy: kmuxd::api::DiagnoseCheck {
                                ok: false,
                                detail: "Configure the proxy URL and token first".into(),
                            },
                            hosts: Vec::new(),
                        });
                    });
                    continue;
                }
                set_status(|s| {
                    s.diagnose = Some(kmuxd::api::DiagnoseReport {
                        running: true,
                        proxy: kmuxd::api::DiagnoseCheck {
                            ok: true,
                            detail: "checking…".into(),
                        },
                        hosts: Vec::new(),
                    });
                });
                match proxy.request_timed(&cfg, Action::Diagnose, None, "25") {
                    Ok(snapshot) => set_status(|s| {
                        s.diagnose = snapshot.terminal.diagnose;
                    }),
                    Err(error) => set_status(|s| {
                        s.diagnose = Some(kmuxd::api::DiagnoseReport {
                            running: false,
                            proxy: kmuxd::api::DiagnoseCheck {
                                ok: false,
                                detail: error,
                            },
                            hosts: Vec::new(),
                        });
                    }),
                }
                continue;
            }
            "key" if !copy_mode() => match op["key"].as_str().unwrap_or("") {
                "ScrollUp" => {
                    scroll_by(page_rows());
                    continue;
                }
                "ScrollDown" => {
                    scroll_down(page_rows());
                    continue;
                }
                "ScrollBottom" => {
                    scroll_down(usize::MAX);
                    continue;
                }
                _ => {}
            },
            "poll" => {
                POLL_PENDING.store(false, Ordering::SeqCst);
                if !WATCHING.load(Ordering::SeqCst) {
                    continue;
                }
            }
            _ => {}
        }
        if cfg.proxy_url.is_empty() || cfg.token.is_empty() {
            set_status(|s| {
                s.configured = false;
                s.last_error = Some("Configure the proxy URL and token in Settings".into());
            });
            continue;
        }
        let action = if name == "poll" || name == "host" {
            Ok(Action::Snapshot)
        } else {
            kmuxd::client::action(&op)
        };
        let result = action.and_then(|action| {
            if let Some(machine) = op["expected_machine"].as_str().filter(|m| !m.is_empty()) {
                let active = status_mutex()
                    .lock()
                    .unwrap()
                    .active_host
                    .clone()
                    .unwrap_or_default();
                if machine != active {
                    return Err("machine changed; refresh before sending input".into());
                }
            }
            let expected = if matches!(
                action,
                Action::Text { .. }
                    | Action::Key { .. }
                    | Action::CloseTab
                    | Action::CopyEnter
                    | Action::CopySearch { .. }
            ) {
                op["expected_pane"]
                    .as_str()
                    .map(String::from)
                    .or_else(|| selected_pane.clone())
            } else {
                None
            };
            proxy.request(&cfg, action, expected)
        });
        match result {
            Ok(snapshot) => {
                selected_pane = Some(snapshot.selection.pane.clone());
                let old = status_mutex().lock().unwrap().clone();
                let new = kmuxd::client::status(snapshot, &old);
                set_status(|s| *s = new);
            }
            Err(e) => {
                log_line(&format!("proxy: {e}"));
                set_status(|s| {
                    s.last_error = Some(e);
                    if name == "poll" || name == "host" || name == "watch_start" {
                        s.connected = false;
                    }
                });
            }
        }
    }
}
