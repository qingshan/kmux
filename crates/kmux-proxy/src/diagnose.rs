//! Read-only health of the proxy and every configured machine.
//!
//! Diagnose never attaches to a pane or replaces the active backend: SSH and
//! inventory probes run on their own connections, in parallel.
use crate::{backend, hosts::Host, Config};
use kmuxd::api::{
    Backend, DiagnoseCheck, DiagnoseReport, HostDiagnose, Machine, Selection, Snapshot, VERSION,
};
use kmuxd::status::Status;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

pub fn snapshot(cfg: &Config) -> Snapshot {
    Snapshot {
        version: VERSION,
        machine: cfg.default_host.clone(),
        machines: cfg
            .hosts
            .iter()
            .map(|host| Machine {
                id: host.id.clone(),
                name: host.name.clone(),
                backend: host.backend,
            })
            .collect(),
        sessions: Vec::new(),
        tabs: Vec::new(),
        panes: Vec::new(),
        selection: Selection::default(),
        terminal: Status {
            configured: true,
            connected: true,
            diagnose: Some(run(cfg)),
            updated_at: kmuxd::now_epoch(),
            ..Status::default()
        },
    }
}

pub fn run(cfg: &Config) -> DiagnoseReport {
    let (tx, rx) = mpsc::channel();
    for host in &cfg.hosts {
        let cfg = cfg.clone();
        let host = host.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(probe_host(&cfg, &host));
        });
    }
    drop(tx);
    let mut found = Vec::new();
    for _ in 0..cfg.hosts.len() {
        match rx.recv_timeout(Duration::from_secs(18)) {
            Ok(row) => found.push(row),
            Err(_) => break,
        }
    }
    let hosts = cfg
        .hosts
        .iter()
        .filter_map(|host| found.iter().find(|row| row.id == host.id).cloned())
        .collect::<Vec<_>>();
    DiagnoseReport {
        running: false,
        proxy: DiagnoseCheck {
            ok: true,
            detail: format!(
                "API v{VERSION}, {} machine{}",
                cfg.hosts.len(),
                if cfg.hosts.len() == 1 { "" } else { "s" }
            ),
        },
        hosts,
    }
}

fn probe_host(cfg: &Config, host: &Host) -> HostDiagnose {
    let ssh = probe_ssh(host);
    let service = if ssh.ok {
        probe_service(cfg, host)
    } else {
        DiagnoseCheck {
            ok: false,
            detail: "skipped (host unreachable)".into(),
        }
    };
    HostDiagnose {
        id: host.id.clone(),
        name: host.name.clone(),
        backend: host.backend,
        ok: ssh.ok && service.ok,
        ssh,
        service,
    }
}

fn probe_ssh(host: &Host) -> DiagnoseCheck {
    if let Some(user) = host.target.strip_prefix("local:") {
        return DiagnoseCheck {
            ok: true,
            detail: format!("local {user}"),
        };
    }
    let output = Command::new("ssh")
        .args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=1",
            "--",
            &host.target,
            "true",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match output {
        Ok(out) if out.status.success() => DiagnoseCheck {
            ok: true,
            detail: host.target.clone(),
        },
        Ok(out) => DiagnoseCheck {
            ok: false,
            detail: first_line(&String::from_utf8_lossy(&out.stderr)),
        },
        Err(error) => DiagnoseCheck {
            ok: false,
            detail: error.to_string(),
        },
    }
}

fn probe_service(cfg: &Config, host: &Host) -> DiagnoseCheck {
    match backend::discover_inventory(cfg, host) {
        Ok(inventory) => DiagnoseCheck {
            ok: true,
            detail: service_detail(host.backend, &inventory),
        },
        Err(error) => DiagnoseCheck {
            ok: false,
            detail: first_line(&error),
        },
    }
}

pub fn service_detail(backend: Backend, inventory: &backend::Inventory) -> String {
    let names: Vec<&str> = inventory
        .sessions
        .iter()
        .map(|session| session.name.as_str())
        .collect();
    let noun = match backend {
        Backend::Herdr => {
            if names.len() == 1 {
                "workspace"
            } else {
                "workspaces"
            }
        }
        Backend::Tmux => {
            if names.len() == 1 {
                "session"
            } else {
                "sessions"
            }
        }
    };
    if names.is_empty() {
        format!("0 {noun}")
    } else if names.len() <= 4 {
        format!("{} {noun}: {}", names.len(), names.join(", "))
    } else {
        format!(
            "{} {noun}: {}, {}…",
            names.len(),
            names[..3].join(", "),
            names.len() - 3
        )
    }
}

fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("unavailable");
    if line.len() > 160 {
        format!("{}…", &line[..157])
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::Host;
    use kmuxd::api::Session;

    #[test]
    fn service_detail_names_sessions_and_workspaces() {
        let inventory = backend::Inventory {
            sessions: vec![
                Session {
                    id: "$0".into(),
                    name: "api".into(),
                    agents: None,
                },
                Session {
                    id: "$1".into(),
                    name: "build".into(),
                    agents: None,
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            service_detail(Backend::Tmux, &inventory),
            "2 sessions: api, build"
        );
        assert_eq!(
            service_detail(Backend::Herdr, &backend::Inventory::default()),
            "0 workspaces"
        );
    }

    #[test]
    fn local_targets_skip_ssh() {
        let check = probe_ssh(&Host {
            backend: Backend::Tmux,
            herdr_session: String::new(),
            herdr_socket: String::new(),
            tmux_socket: String::new(),
            id: "devbox".into(),
            name: "devbox".into(),
            target: "local:qingshan".into(),
            session: "main".into(),
        });
        assert!(check.ok);
        assert_eq!(check.detail, "local qingshan");
    }
}
