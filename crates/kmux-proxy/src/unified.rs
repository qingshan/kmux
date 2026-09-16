use crate::{
    backend::{Inventory, Link},
    copy::CopyView,
    hosts, Config,
};
use kmuxd::{api::*, screen::Screen, status::Status};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

struct Active {
    machine: String,
    link: Link,
    selection: Selection,
    copy: Option<CopyView>,
    clipboard: Option<String>,
}
static ACTIVE: OnceLock<Mutex<Option<Active>>> = OnceLock::new();

pub fn changes(cfg: &Config, r: ChangesRequest) -> Result<Changes, String> {
    if r.version != VERSION {
        return Err("API version mismatch".into());
    }
    let host =
        hosts::resolve(&cfg.hosts, &cfg.default_host, &r.machine).map_err(|_| "unknown machine")?;
    let events = {
        let slot = ACTIVE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map_err(|_| "backend lock poisoned")?;
        let active = slot
            .as_ref()
            .filter(|a| a.machine == host.id)
            .ok_or("machine changed; refresh first")?;
        active.link.events.clone()
    };
    // Input and host changes are free to proceed while this request waits.
    let stamp = events.wait(r.after, Duration::from_millis(r.timeout_ms.min(5000)));
    Ok(Changes {
        revision: stamp.revision,
        event_updates: stamp.connected,
    })
}

fn choose(inv: &Inventory, s: &mut Selection) {
    if !inv.sessions.iter().any(|v| v.id == s.session) {
        s.session = inv
            .sessions
            .first()
            .map(|v| v.id.clone())
            .unwrap_or_default();
    }
    if !inv
        .tabs
        .iter()
        .any(|v| v.id == s.tab && v.session == s.session)
    {
        s.tab = inv
            .tabs
            .iter()
            .find(|v| v.session == s.session)
            .map(|v| v.id.clone())
            .unwrap_or_default();
    }
    if !inv
        .panes
        .iter()
        .any(|v| v.id == s.pane && v.tab == s.tab && v.session == s.session)
    {
        s.pane = inv
            .panes
            .iter()
            .find(|v| v.tab == s.tab && v.session == s.session)
            .map(|v| v.id.clone())
            .unwrap_or_default();
    }
}
pub fn request(cfg: &Config, r: Request) -> Result<Snapshot, String> {
    if r.version != VERSION {
        return Err("API version mismatch: upgrade proxy and Kindle together".into());
    }
    if matches!(r.action, Action::Diagnose) {
        return Ok(crate::diagnose::snapshot(cfg));
    }
    let host =
        hosts::resolve(&cfg.hosts, &cfg.default_host, &r.machine).map_err(|_| "unknown machine")?;
    let mut slot = ACTIVE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| "backend lock poisoned")?;
    let switching = slot.as_ref().is_none_or(|a| a.machine != host.id);
    if switching {
        if !matches!(
            r.action,
            Action::Snapshot | Action::Select { .. } | Action::Diagnose
        ) {
            return Err("machine changed; refresh before sending input".into());
        }
        let mut link = Link::open(cfg, host)?;
        let inventory = link.inventory()?;
        let mut selection = Selection::default();
        if host.backend == Backend::Tmux {
            selection.session = inventory
                .sessions
                .iter()
                .find(|s| s.name == host.session)
                .map(|s| s.id.clone())
                .unwrap_or_default();
        }
        choose(&inventory, &mut selection);
        *slot = Some(Active {
            machine: host.id.clone(),
            link,
            selection,
            copy: None,
            clipboard: None,
        });
    }
    let detach = matches!(r.action, Action::Detach);
    let mut result = execute(cfg, r, slot.as_mut().unwrap());
    if detach {
        if let Ok(s) = result.as_mut() {
            s.terminal.connected = false;
        }
        *slot = None;
    }
    if result
        .as_ref()
        .is_err_and(|e| e.contains("disconnected") || e.contains("timed out"))
    {
        *slot = None;
    }
    result
}
fn execute(cfg: &Config, r: Request, a: &mut Active) -> Result<Snapshot, String> {
    let mutating = !matches!(r.action, Action::Snapshot | Action::Diagnose);
    if mutating {
        // Before input, revalidate the expected pane against a fresh inventory.
        a.link.invalidate_inventory();
        a.link.events.signal(a.link.events.stamp().connected);
    }
    let stamp = a.link.events.stamp();
    let mut inv = a.link.inventory()?;
    let old_pane = a.selection.pane.clone();
    choose(&inv, &mut a.selection);
    if old_pane != a.selection.pane {
        a.copy = None;
    }
    if let Some(expected) = r.expected_pane {
        if expected != a.selection.pane {
            return Err("pane changed; refresh before sending input".into());
        }
    }
    let mut files = Vec::new();
    let mut cwd = None;
    let mut truncated = false;
    match r.action {
        Action::Snapshot | Action::Diagnose => {}
        Action::Select { session, tab, pane } => {
            if !session.is_empty() {
                if !inv.sessions.iter().any(|s| s.id == session) {
                    return Err("session no longer exists".into());
                }
                a.selection.session = session;
            }
            if !tab.is_empty() {
                if !inv
                    .tabs
                    .iter()
                    .any(|t| t.id == tab && t.session == a.selection.session)
                {
                    return Err("tab no longer exists".into());
                }
                a.selection.tab = tab;
            }
            choose(&inv, &mut a.selection);
            if !pane.is_empty() {
                if !inv
                    .panes
                    .iter()
                    .any(|p| p.id == pane && p.tab == a.selection.tab)
                {
                    return Err("pane no longer exists".into());
                }
                a.selection.pane = pane;
            }
            a.copy = None;
        }
        Action::Text { text, paste } => {
            if let Some(copy) = a.copy.as_mut() {
                for ch in text.chars() {
                    if let Some(t) = copy.key(&ch.to_string())? {
                        if !t.is_empty() {
                            a.clipboard = Some(t);
                        }
                        a.copy = None;
                        break;
                    }
                }
            } else {
                a.link.text(&a.selection.pane, &text, paste)?;
            }
        }
        Action::CopyKey { .. } if a.copy.is_none() => {
            return Err("enter copy mode first".into());
        }
        Action::Key { key } | Action::CopyKey { key } => {
            if let Some(copy) = a.copy.as_mut() {
                if let Some(t) = copy.key(&key)? {
                    if !t.is_empty() {
                        a.clipboard = Some(t);
                    }
                    a.copy = None;
                }
            } else {
                a.link.key(&a.selection.pane, &key)?;
            }
        }
        Action::CopyEnter => {
            a.copy = Some(CopyView::new(&a.link.capture(&a.selection.pane)?));
        }
        Action::CopySearch { query, backwards } => {
            a.copy
                .as_mut()
                .ok_or("enter copy mode first")?
                .search(&query, backwards)?;
        }
        Action::CreateSession { name } => {
            a.selection.session = a.link.create_session(&name)?;
            a.copy = None;
        }
        Action::CreateTab { cwd } => {
            a.selection.tab = a.link.create_tab(&a.selection, cwd.as_deref())?;
            a.copy = None;
        }
        Action::CloseTab => {
            a.link.close_tab(&a.selection)?;
            a.selection.tab.clear();
            a.copy = None;
        }
        Action::NextTab { previous } => {
            let tabs: Vec<_> = inv
                .tabs
                .iter()
                .filter(|t| t.session == a.selection.session)
                .collect();
            if !tabs.is_empty() {
                let i = tabs
                    .iter()
                    .position(|t| t.id == a.selection.tab)
                    .unwrap_or(0);
                let j = (i + if previous { tabs.len() - 1 } else { 1 }) % tabs.len();
                a.selection.tab = tabs[j].id.clone();
            }
            a.copy = None;
        }
        Action::Detach => {
            a.copy = None;
        }
        Action::ListFiles { path } => {
            let path = path
                .or_else(|| {
                    inv.panes
                        .iter()
                        .find(|p| p.id == a.selection.pane)
                        .map(|p| p.cwd.clone())
                })
                .ok_or("pane directory unavailable")?;
            let host = cfg.hosts.iter().find(|h| h.id == a.machine).unwrap();
            // This separate SSH command cannot write into the user's terminal.
            let code="import os,json,sys,signal; signal.alarm(5); p=sys.argv[1]; assert os.path.isabs(p); n=sorted(os.listdir(p)); print(json.dumps({'files':[{'name':x,'dir':os.path.isdir(os.path.join(p,x))} for x in n[:500]],'truncated':len(n)>500}))";
            let command = format!(
                "python3 -c {} {}",
                kmuxd::tmux_keys::shell_quote(code),
                kmuxd::tmux_keys::shell_quote(&path)
            );
            let out = crate::backend::remote_command(host, &command)
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err("directory listing failed".into());
            }
            let result: serde_json::Value =
                serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
            files = serde_json::from_value(result["files"].clone()).map_err(|e| e.to_string())?;
            truncated = result["truncated"].as_bool().unwrap_or(false);
            cwd = Some(path);
        }
    }
    if a.copy.is_none() {
        let _ = a.link.fit(&a.selection);
    }
    if mutating {
        a.link.invalidate_inventory();
    }
    inv = a.link.inventory()?;
    choose(&inv, &mut a.selection);
    let mut screen = if let Some(copy) = a.copy.as_ref() {
        copy.screen()
    } else {
        let mut s = Screen::new(80, 24);
        if !a.selection.pane.is_empty() {
            s.load_snapshot(&a.link.capture(&a.selection.pane)?);
        }
        s
    };
    if a.copy.is_none() {
        let host = cfg.hosts.iter().find(|h| h.id == a.machine).unwrap();
        // Herdr's public read API doesn't expose the cursor. Don't invent one.
        screen.cursor_y = 24;
        if host.backend == Backend::Tmux && !a.selection.pane.is_empty() {
            let text = a.link.command(&format!(
                "display-message -p -t {} '#{{cursor_x}} #{{cursor_y}} #{{pane_height}}'",
                kmuxd::tmux_keys::tmux_quote(&a.selection.pane)
            ))?;
            let n: Vec<usize> = text
                .split_whitespace()
                .filter_map(|v| v.parse().ok())
                .collect();
            if n.len() == 3 {
                screen.cursor_x = n[0];
                screen.cursor_y = n[1].saturating_sub(n[2].saturating_sub(24));
            }
        }
    }
    let (rows, attrs) = screen.render();
    let terminal = Status {
        event_updates: a.link.events.stamp().connected,
        event_revision: stamp.revision,
        session_catalog: crate::catalog::snapshot(cfg, &a.machine, &inv),
        connected: true,
        configured: true,
        screen: rows,
        attrs,
        scrollback: screen.scrollback,
        cursor_x: screen.cursor_x,
        cursor_y: screen.cursor_y,
        copy_mode: a.copy.is_some(),
        clipboard: a.clipboard.clone(),
        cwd,
        files,
        files_truncated: truncated,
        updated_at: kmuxd::now_epoch(),
        ..Status::default()
    };
    Ok(Snapshot {
        version: VERSION,
        machine: a.machine.clone(),
        machines: cfg
            .hosts
            .iter()
            .map(|h| Machine {
                id: h.id.clone(),
                name: h.name.clone(),
                backend: h.backend,
            })
            .collect(),
        sessions: inv.sessions,
        tabs: inv.tabs,
        panes: inv.panes,
        selection: a.selection.clone(),
        terminal,
    })
}
