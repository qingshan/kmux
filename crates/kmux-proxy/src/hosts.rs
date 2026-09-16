//! Named ssh+tmux hosts behind one kmux-proxy.
//!
//! The Kindle sends a host **id** (never `user@host`). Legacy configs with a
//! single `target`/`session` pair are wrapped as one host on load.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Host {
    #[serde(default)]
    pub backend: kmuxd::api::Backend,
    #[serde(default)]
    pub herdr_session: String,
    #[serde(default)]
    pub herdr_socket: String,
    #[serde(default)]
    pub tmux_socket: String,
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub session: String,
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// ssh `user@host` → `host`, stripped to the id charset.
pub fn id_from_target(target: &str) -> String {
    let host = target.rsplit('@').next().unwrap_or(target).trim();
    let cleaned: String = host
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
        .collect();
    if cleaned.is_empty() {
        "host".into()
    } else {
        cleaned
    }
}

/// Fill `hosts` from a legacy target/session; pick `default_host`; set
/// missing names/sessions. Errors on empty inventory or a bad id.
pub fn normalize(
    hosts: &mut Vec<Host>,
    default_host: &mut String,
    legacy_target: &str,
    legacy_session: &str,
) -> Result<(), String> {
    if hosts.is_empty() {
        if legacy_target.trim().is_empty() {
            return Err("target or hosts required".into());
        }
        let id = id_from_target(legacy_target);
        let session = if legacy_session.is_empty() {
            "main".into()
        } else {
            legacy_session.to_string()
        };
        hosts.push(Host {
            backend: kmuxd::api::Backend::Tmux,
            herdr_session: String::new(),
            herdr_socket: String::new(),
            tmux_socket: String::new(),
            id: id.clone(),
            name: id.clone(),
            target: legacy_target.to_string(),
            session,
        });
        if default_host.is_empty() {
            *default_host = id;
        }
    }
    for h in hosts.iter_mut() {
        if !valid_id(&h.id) {
            return Err(format!("invalid host id {}", h.id));
        }
        if !h.herdr_session.is_empty() && !valid_id(&h.herdr_session) {
            return Err(format!("invalid Herdr server session for {}", h.id));
        }
        if !h.tmux_socket.is_empty() && !valid_id(&h.tmux_socket) {
            return Err(format!("invalid tmux socket for {}", h.id));
        }
        if h.name.is_empty() {
            h.name = h.id.clone();
        }
        if h.target.trim().is_empty() {
            // SSH resolves IDs through the service user's ssh_config by
            // default. Keep an explicit target only for nonmatching aliases.
            h.target = h.id.clone();
        }
        if h.session.is_empty() {
            h.session = "main".into();
        }
        if !valid_id(&h.session) {
            return Err(format!("invalid session name for host {}", h.id));
        }
    }
    if default_host.is_empty() || hosts.iter().all(|h| h.id != *default_host) {
        *default_host = hosts[0].id.clone();
    }
    Ok(())
}

/// `requested` empty → `default_host`. Unknown/invalid id is an error.
pub fn resolve<'a>(
    hosts: &'a [Host],
    default_host: &str,
    requested: &str,
) -> Result<&'a Host, ResolveError> {
    if !requested.is_empty() && !valid_id(requested) {
        return Err(ResolveError::Invalid);
    }
    let id = if requested.is_empty() {
        default_host
    } else {
        requested
    };
    hosts
        .iter()
        .find(|h| h.id == id)
        .ok_or(ResolveError::Unknown)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    Invalid,
    Unknown,
}

#[cfg(test)]
pub fn public_list(hosts: &[Host], active: &str) -> serde_json::Value {
    serde_json::json!({
        "hosts": hosts.iter().map(|h| {
            serde_json::json!({ "id": h.id, "name": h.name })
        }).collect::<Vec<_>>(),
        "active": active,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_legacy_target() {
        let mut hosts = Vec::new();
        let mut default = String::new();
        normalize(&mut hosts, &mut default, "qingshan@outbox", "main").unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].id, "outbox");
        assert_eq!(hosts[0].target, "qingshan@outbox");
        assert_eq!(default, "outbox");
    }

    #[test]
    fn default_falls_back_to_first() {
        let mut hosts = vec![
            Host {
                backend: kmuxd::api::Backend::Tmux,
                herdr_session: String::new(),
                herdr_socket: String::new(),
                tmux_socket: String::new(),
                id: "a".into(),
                name: String::new(),
                target: "a@a".into(),
                session: String::new(),
            },
            Host {
                backend: kmuxd::api::Backend::Tmux,
                herdr_session: String::new(),
                herdr_socket: String::new(),
                tmux_socket: String::new(),
                id: "b".into(),
                name: "B".into(),
                target: "b@b".into(),
                session: "work".into(),
            },
        ];
        let mut default = "missing".into();
        normalize(&mut hosts, &mut default, "", "").unwrap();
        assert_eq!(default, "a");
        assert_eq!(hosts[0].name, "a");
        assert_eq!(hosts[0].session, "main");
        assert_eq!(hosts[1].session, "work");
    }

    #[test]
    fn resolve_default_and_named() {
        let hosts = vec![
            Host {
                backend: kmuxd::api::Backend::Tmux,
                herdr_session: String::new(),
                herdr_socket: String::new(),
                tmux_socket: String::new(),
                id: "outbox".into(),
                name: "outbox".into(),
                target: "q@outbox".into(),
                session: "main".into(),
            },
            Host {
                backend: kmuxd::api::Backend::Tmux,
                herdr_session: String::new(),
                herdr_socket: String::new(),
                tmux_socket: String::new(),
                id: "home".into(),
                name: "home".into(),
                target: "q@home".into(),
                session: "main".into(),
            },
        ];
        assert_eq!(resolve(&hosts, "outbox", "").unwrap().id, "outbox");
        assert_eq!(resolve(&hosts, "outbox", "home").unwrap().target, "q@home");
        assert_eq!(
            resolve(&hosts, "outbox", "nope"),
            Err(ResolveError::Unknown)
        );
        assert_eq!(
            resolve(&hosts, "outbox", "bad id"),
            Err(ResolveError::Invalid)
        );
    }

    #[test]
    fn rejects_empty_inventory() {
        let mut hosts = Vec::new();
        let mut default = String::new();
        assert!(normalize(&mut hosts, &mut default, "", "").is_err());
    }

    #[test]
    fn defaults_ssh_target_name_session_and_backend() {
        let mut hosts = vec![Host {
            backend: kmuxd::api::Backend::Tmux,
            herdr_session: String::new(),
            herdr_socket: String::new(),
            tmux_socket: String::new(),
            id: "outbox".into(),
            name: String::new(),
            target: String::new(),
            session: String::new(),
        }];
        let mut default = String::new();
        normalize(&mut hosts, &mut default, "", "").unwrap();
        assert_eq!(hosts[0].target, "outbox");
        assert_eq!(hosts[0].name, "outbox");
        assert_eq!(hosts[0].session, "main");
        assert_eq!(default, "outbox");
    }

    #[test]
    fn public_list_omits_targets() {
        let hosts = vec![Host {
            backend: kmuxd::api::Backend::Tmux,
            herdr_session: String::new(),
            herdr_socket: String::new(),
            tmux_socket: String::new(),
            id: "outbox".into(),
            name: "Outbox".into(),
            target: "secret@host".into(),
            session: "main".into(),
        }];
        let v = public_list(&hosts, "outbox");
        let s = v.to_string();
        assert!(s.contains("Outbox"));
        assert!(!s.contains("secret@host"));
        assert_eq!(v["active"], "outbox");
    }
}
