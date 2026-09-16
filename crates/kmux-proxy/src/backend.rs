//! Backend adapters own all tmux commands and Herdr socket requests.
use crate::{hosts::Host, Config};
use kmuxd::api::{Backend, Pane, Selection, Session, Tab};
use kmuxd::tmux_keys::tmux_quote as quote;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::FromRawFd;
use std::process::{Child, Command, Stdio};
use std::sync::{
    mpsc::{self, Receiver},
    Arc,
};
use std::time::{Duration, Instant};

#[derive(Clone, Default, serde::Deserialize)]
pub struct Inventory {
    pub sessions: Vec<Session>,
    pub tabs: Vec<Tab>,
    pub panes: Vec<Pane>,
}
impl Inventory {
    fn with_agents(mut self) -> Self {
        for session in &mut self.sessions {
            session.agents = kmuxd::agents::summarize(
                self.panes
                    .iter()
                    .filter(|p| p.session == session.id)
                    .filter_map(|p| p.agent.as_ref()),
            );
        }
        for tab in &mut self.tabs {
            tab.agents = kmuxd::agents::summarize(
                self.panes
                    .iter()
                    .filter(|p| p.session == tab.session && p.tab == tab.id)
                    .filter_map(|p| p.agent.as_ref()),
            );
        }
        self
    }
}
pub struct Link {
    child: Child,
    input: Box<dyn Write + Send>,
    replies: Receiver<Result<String, String>>,
    tmux: bool,
    pub events: Arc<crate::events::Events>,
    inventory_cache: Option<(u64, Instant, Inventory)>,
    cols: u16,
    rows: u16,
    herdr_host: Option<Host>,
    herdr_ui: Option<(Child, i32)>,
}
impl Drop for Link {
    fn drop(&mut self) {
        self.events.signal(false);
        if let Some((mut ui, master)) = self.herdr_ui.take() {
            let _ = ui.kill();
            let _ = ui.wait();
            unsafe { libc::close(master) };
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Link {
    pub fn open(cfg: &Config, host: &Host) -> Result<Self, String> {
        Self::open_with_events(cfg, host, true)
    }
    fn open_with_events(cfg: &Config, host: &Host, subscribe: bool) -> Result<Self, String> {
        let (child, input, output, tmux): (
            Child,
            Box<dyn Write + Send>,
            Box<dyn std::io::Read + Send>,
            bool,
        ) = if host.backend == Backend::Tmux {
            let (child, fd) = crate::spawn_tmux(cfg, host)?;
            let file = unsafe { std::fs::File::from_raw_fd(fd) };
            let read = file.try_clone().map_err(|e| e.to_string())?;
            (child, Box::new(file), Box::new(read), true)
        } else {
            let socket = if !host.herdr_socket.is_empty() {
                host.herdr_socket.clone()
            } else if host.herdr_session.is_empty() {
                "~/.config/herdr/herdr.sock".into()
            } else {
                format!("~/.config/herdr/sessions/{}/herdr.sock", host.herdr_session)
            };
            // A socket override may refer to an externally managed server;
            // only auto-start when the configured/default session is known.
            let start_session = if host.herdr_socket.is_empty() {
                if host.herdr_session.is_empty() {
                    "default"
                } else {
                    host.herdr_session.as_str()
                }
            } else {
                ""
            };
            let remote = format!(
                "python3 -u -c {} {} {} {}",
                quote(include_str!("herdr_bridge.py")),
                quote(&socket),
                if subscribe { "events" } else { "metadata" },
                quote(start_session)
            );
            let mut command = remote_command(host, &remote);
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())?;
            let input = child.stdin.take().ok_or("missing input")?;
            let output = child.stdout.take().ok_or("missing output")?;
            (child, Box::new(input), Box::new(output), false)
        };
        let (tx, replies) = mpsc::channel();
        let events = Arc::new(crate::events::Events::default());
        let reader_events = events.clone();
        let machine = host.id.clone();
        std::thread::spawn(move || {
            let mut block = None::<(String, String)>;
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                let line = line.trim_start_matches("\u{1b}P1000p");
                if !tmux {
                    if let Ok(value) = serde_json::from_str::<Value>(line) {
                        if let Some(event) = value["kmux_event"].as_str() {
                            reader_events.signal(event != "offline");
                            crate::catalog::invalidate(&machine);
                            continue;
                        }
                    }
                    if tx.send(Ok(line.to_string())).is_err() {
                        break;
                    }
                    continue;
                }
                if block.is_none() && line.starts_with("%begin ") {
                    block = Some((line[7..].to_string(), String::new()));
                } else if block.as_ref().is_some_and(|(id, _)| {
                    line.strip_prefix("%end ") == Some(id.as_str())
                        || line.strip_prefix("%error ") == Some(id.as_str())
                }) {
                    if let Some((_, text)) = block.take() {
                        let reply = if line.starts_with("%error ") {
                            Err(text)
                        } else {
                            Ok(text)
                        };
                        if tx.send(reply).is_err() {
                            break;
                        }
                    }
                } else if let Some((_, text)) = block.as_mut() {
                    text.push_str(line);
                    text.push('\n');
                }
            }
            reader_events.signal(false);
        });
        let mut link = Self {
            child,
            input,
            replies,
            tmux,
            events,
            inventory_cache: None,
            cols: cfg.cols,
            rows: cfg.rows,
            herdr_host: if tmux { None } else { Some(host.clone()) },
            herdr_ui: None,
        };
        if tmux {
            writeln!(link.input, "display-message -p KMUX_READY").map_err(|e| e.to_string())?;
            loop {
                if link.receive()?.trim() == "KMUX_READY" {
                    break;
                }
            }
            link.command(&format!("refresh-client -C {},{}", cfg.cols, cfg.rows))?;
        } else {
            link.rpc("ping", json!({}))?;
        }
        Ok(link)
    }

    /// Make the active window or workspace 80×24 (or `cols`×`rows`) if it is not.
    ///
    /// tmux: move the control client onto the selected session/window, then
    /// `resize-window` if that window is still not 80×24. `display-message -t`
    /// takes a pane, not a window — targeting the window id read the previously
    /// attached session and skipped the resize. Herdr has no cols/rows RPC, so
    /// a sized TUI client is attached for the life of this link, and a split
    /// pane is zoomed so it fills that viewport.
    pub fn fit(&mut self, selection: &Selection) -> Result<(), String> {
        if self.tmux {
            if selection.session.is_empty() || selection.tab.is_empty() {
                return Ok(());
            }
            self.command(&format!("switch-client -t {}", quote(&selection.session)))?;
            self.command(&format!("select-window -t {}", quote(&selection.tab)))?;
            if !selection.pane.is_empty() {
                self.command(&format!("select-pane -t {}", quote(&selection.pane)))?;
            }
            self.command(&format!("refresh-client -C {},{}", self.cols, self.rows))?;
            let size_target = if selection.pane.is_empty() {
                quote(&selection.tab)
            } else {
                quote(&selection.pane)
            };
            let size = self.command(&format!(
                "display-message -p -t {size_target} '#{{window_width}} #{{window_height}}'"
            ))?;
            if parse_cells(&size) == Some((self.cols, self.rows)) {
                return Ok(());
            }
            self.command(&format!(
                "resize-window -t {} -x {} -y {}",
                quote(&selection.tab),
                self.cols,
                self.rows
            ))?;
            Ok(())
        } else {
            if selection.pane.is_empty() {
                return Ok(());
            }
            let layout = self.rpc("pane.layout", json!({"pane_id": selection.pane}))?;
            let layout = layout.get("layout").unwrap_or(&layout);
            let area = json_cells(&layout["area"]);
            if area != Some((self.cols, self.rows)) {
                self.ensure_herdr_ui();
            }
            let zoomed = layout["zoomed"].as_bool().unwrap_or(false);
            let panes = layout["panes"].as_array().map(|a| a.len()).unwrap_or(0);
            if !zoomed && panes > 1 {
                let _ = self.rpc(
                    "pane.zoom",
                    json!({"pane_id": selection.pane, "mode": "on"}),
                );
            }
            Ok(())
        }
    }

    fn ensure_herdr_ui(&mut self) {
        if self.herdr_ui.is_some() {
            return;
        }
        let Some(host) = self.herdr_host.clone() else {
            return;
        };
        if let Ok((child, master)) = crate::spawn_herdr_ui(&host, self.cols, self.rows) {
            self.herdr_ui = Some((child, master));
        }
    }
    fn receive(&mut self) -> Result<String, String> {
        self.replies
            .recv_timeout(Duration::from_secs(8))
            .map_err(|_| {
                "backend disconnected or timed out; reconnect before retrying input".to_string()
            })?
    }
    pub fn command(&mut self, command: &str) -> Result<String, String> {
        writeln!(self.input, "{command}").map_err(|e| e.to_string())?;
        self.input.flush().map_err(|e| e.to_string())?;
        self.receive()
    }
    pub fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let reply =
            self.command(&json!({"id":"kmux","method":method,"params":params}).to_string())?;
        let value: Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
        if let Some(error) = value.get("error") {
            return Err(error.to_string());
        }
        value
            .get("result")
            .cloned()
            .ok_or("missing Herdr result".into())
    }
    pub fn inventory(&mut self) -> Result<Inventory, String> {
        if self.tmux {
            let sessions = self
                .command("list-sessions -F '#{session_id}\t#{session_name}'")?
                .lines()
                .filter_map(|l| l.split_once('\t'))
                .map(|(id, name)| Session {
                    id: id.into(),
                    name: name.into(),
                    agents: None,
                })
                .collect();
            let tabs = self
                .command("list-windows -a -F '#{session_id}\t#{window_id}\t#{window_name}'")?
                .lines()
                .filter_map(|l| {
                    let p: Vec<_> = l.splitn(3, '\t').collect();
                    if p.len() != 3 {
                        return None;
                    }
                    Some(Tab {
                        id: p[1].into(),
                        session: p[0].into(),
                        name: p[2].into(),
                        agents: None,
                    })
                })
                .collect();
            let panes = self.command("list-panes -a -F '#{session_id}\t#{window_id}\t#{pane_id}\t#{pane_current_command}\t#{pane_current_path}'")?.lines().filter_map(|l| {
                let p:Vec<_> = l.splitn(5,'\t').collect(); if p.len()!=5 {return None;} Some(Pane{id:p[2].into(),tab:p[1].into(),session:p[0].into(),command:p[3].into(),cwd:p[4].into(),agent:kmuxd::agents::from_command(p[3])})
            }).collect();
            Ok(Inventory {
                sessions,
                tabs,
                panes,
            }
            .with_agents())
        } else {
            let stamp = self.events.stamp();
            if stamp.connected {
                if let Some((revision, fetched, inventory)) = &self.inventory_cache {
                    if *revision == stamp.revision && fetched.elapsed() < Duration::from_secs(10) {
                        return Ok(inventory.clone());
                    }
                }
            }
            let r = self.rpc("session.snapshot", json!({}))?;
            let s = r.get("snapshot").unwrap_or(&r);
            let array = |key: &str| s[key].as_array().cloned().unwrap_or_default();
            let sessions = array("workspaces")
                .iter()
                .map(|v| Session {
                    id: string(v, "workspace_id"),
                    name: string(v, "label"),
                    agents: None,
                })
                .collect();
            let tabs = array("tabs")
                .iter()
                .map(|v| Tab {
                    id: string(v, "tab_id"),
                    session: string(v, "workspace_id"),
                    name: string(v, "label"),
                    agents: None,
                })
                .collect();
            let panes = array("panes")
                .iter()
                .map(|v| Pane {
                    id: string(v, "pane_id"),
                    tab: string(v, "tab_id"),
                    session: string(v, "workspace_id"),
                    command: string(v, "agent"),
                    agent: herdr_agent(v),
                    cwd: v["foreground_cwd"]
                        .as_str()
                        .or(v["cwd"].as_str())
                        .unwrap_or("")
                        .into(),
                })
                .collect();
            let inventory = Inventory {
                sessions,
                tabs,
                panes,
            }
            .with_agents();
            // Use the pre-fetch revision: an event during the RPC must force
            // another read, not be lost when the snapshot is installed.
            self.inventory_cache = Some((stamp.revision, Instant::now(), inventory.clone()));
            Ok(inventory)
        }
    }
    pub fn invalidate_inventory(&mut self) {
        self.inventory_cache = None;
    }
    pub fn capture(&mut self, pane: &str) -> Result<String, String> {
        if self.tmux {
            self.command(&format!("capture-pane -p -e -N -t {} -S -500", quote(pane)))
        } else {
            let r=self.rpc("pane.read",json!({"pane_id":pane,"source":"recent","lines":524,"format":"ansi","strip_ansi":false}))?;
            Ok(string(&r["read"], "text"))
        }
    }
    pub fn text(&mut self, pane: &str, text: &str, paste: bool) -> Result<(), String> {
        let payload = if paste {
            kmuxd::bracketed_paste_text(text)
        } else {
            text.to_string()
        };
        if self.tmux {
            self.command(&tmux_send_keys_literal(pane, &payload))?;
        } else {
            self.rpc("pane.send_text", json!({"pane_id":pane,"text":payload}))?;
        }
        Ok(())
    }
    pub fn key(&mut self, pane: &str, key: &str) -> Result<(), String> {
        if self.tmux {
            let cmd = kmuxd::tmux_keys::send_keys_command(key).ok_or("unsupported key")?;
            let rest = cmd.strip_prefix("send-keys ").ok_or("unsupported key")?;
            if rest.starts_with("-K") {
                return Err("client prefix keys are not supported by the unified API".into());
            }
            self.command(&format!("send-keys -t {} {rest}", quote(pane)))?;
        } else {
            self.rpc(
                "pane.send_keys",
                json!({"pane_id":pane,"keys":[herdr_key(key)]}),
            )?;
        }
        Ok(())
    }
    pub fn create_session(&mut self, name: &str) -> Result<String, String> {
        if name.trim().is_empty() || name.chars().any(char::is_control) {
            return Err("invalid session name".into());
        }
        if self.tmux {
            Ok(self
                .command(&format!(
                    "new-session -d -P -F '#{{session_id}}' -s {}",
                    quote(name)
                ))?
                .trim()
                .into())
        } else {
            let r = self.rpc("workspace.create", json!({"label":name,"focus":false}))?;
            Ok(string(&r["workspace"], "workspace_id"))
        }
    }
    pub fn create_tab(
        &mut self,
        selection: &Selection,
        cwd: Option<&str>,
    ) -> Result<String, String> {
        if self.tmux {
            Ok(self
                .command(&format!(
                    "new-window -d -P -F '#{{window_id}}' -t {} {}",
                    quote(&format!("{}:", selection.session)),
                    cwd.map(|p| format!("-c {}", quote(p))).unwrap_or_default()
                ))?
                .trim()
                .into())
        } else {
            let r = self.rpc(
                "tab.create",
                json!({"workspace_id":selection.session,"cwd":cwd,"focus":false}),
            )?;
            Ok(string(&r["tab"], "tab_id"))
        }
    }
    pub fn close_tab(&mut self, selection: &Selection) -> Result<(), String> {
        if self.tmux {
            self.command(&format!(
                "kill-window -t {}",
                quote(&format!("{}:{}", selection.session, selection.tab))
            ))?;
        } else {
            self.rpc("tab.close", json!({"tab_id":selection.tab}))?;
        }
        Ok(())
    }
}
fn string(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").into()
}

/// `send-keys -l` argument for a control-mode command. Newlines and the
/// characters tmux's double-quoted parser treats specially are escaped.
fn tmux_send_keys_literal(pane: &str, text: &str) -> String {
    let literal = format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "\\$")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    );
    format!("send-keys -l -t {} {literal}", quote(pane))
}

fn herdr_agent(v: &Value) -> Option<kmuxd::api::Agent> {
    let name = v["display_agent"]
        .as_str()
        .filter(|name| !name.is_empty())
        .or_else(|| v["agent"].as_str().filter(|name| !name.is_empty()))?;
    Some(kmuxd::api::Agent {
        name: name.into(),
        state: serde_json::from_value(v["agent_status"].clone()).unwrap_or_default(),
        source: "herdr".into(),
    })
}

#[cfg(test)]
mod agent_tests {
    use super::*;
    use kmuxd::api::AgentState;
    #[test]
    fn herdr_labels_and_future_states_are_safe() {
        let agent = herdr_agent(
            &json!({"agent":"codex","display_agent":"Review <project>","agent_status":"blocked"}),
        )
        .unwrap();
        assert_eq!(agent.name, "Review <project>");
        assert_eq!(agent.state, AgentState::Blocked);
        assert_eq!(agent.source, "herdr");
        assert_eq!(
            herdr_agent(&json!({"agent":"codex","agent_status":"future"}))
                .unwrap()
                .state,
            AgentState::Unknown
        );
        assert!(herdr_agent(&json!({"agent":null,"agent_status":"idle"})).is_none());
    }

    #[test]
    fn parse_cells_reads_width_then_height() {
        assert_eq!(parse_cells("80 24\n"), Some((80, 24)));
        assert_eq!(parse_cells("120 40 extra"), Some((120, 40)));
        assert_eq!(parse_cells("80"), None);
        assert_eq!(json_cells(&json!({"width":80,"height":24})), Some((80, 24)));
        assert_eq!(json_cells(&json!({"width":80})), None);
    }

    #[test]
    fn tmux_literal_keeps_bracketed_paste_and_quotes() {
        let wrapped = kmuxd::bracketed_paste_text("printf '\\e[8;24;80t'");
        let cmd = tmux_send_keys_literal("%1", &wrapped);
        assert!(cmd.starts_with("send-keys -l -t '%1' \""));
        assert!(cmd.contains("\x1b[200~"));
        assert!(cmd.contains("\x1b[201~"));
        assert!(cmd.contains("printf '\\\\e[8;24;80t'"));
    }
}

pub fn parse_cells(text: &str) -> Option<(u16, u16)> {
    let mut nums = text.split_whitespace().filter_map(|s| s.parse().ok());
    Some((nums.next()?, nums.next()?))
}

fn json_cells(value: &Value) -> Option<(u16, u16)> {
    Some((
        value.get("width")?.as_u64()? as u16,
        value.get("height")?.as_u64()? as u16,
    ))
}

/// Metadata-only queries must not attach, resize panes, or create tmux sessions.
pub fn discover_inventory(cfg: &Config, host: &Host) -> Result<Inventory, String> {
    if host.backend == Backend::Herdr {
        return Link::open_with_events(cfg, host, false)?.inventory();
    }
    let code = include_str!("tmux_inventory.py");
    let remote = format!("python3 -c {} {}", quote(code), quote(&host.tmux_socket));
    let out = remote_command(host, &remote)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("inventory discovery unavailable".into());
    }
    let mut inventory: Inventory =
        serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    for pane in &mut inventory.panes {
        pane.agent = kmuxd::agents::from_command(&pane.command);
    }
    Ok(inventory.with_agents())
}
pub fn herdr_key(key: &str) -> String {
    key.replace("C-", "ctrl+")
        .replace("M-", "alt+")
        .replace("S-", "shift+")
        .to_lowercase()
        .replace("escape", "esc")
}
pub fn remote_command(host: &Host, remote: &str) -> Command {
    if let Some(user) = host.target.strip_prefix("local:") {
        let mut c = Command::new("sudo");
        c.args(["-n", "-H", "-u", user, "sh", "-c", remote]);
        c
    } else {
        let mut c = Command::new("ssh");
        c.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
        ]);
        c.arg("--").arg(&host.target).arg(remote);
        c
    }
}
