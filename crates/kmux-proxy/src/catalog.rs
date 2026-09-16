//! Background, read-only discovery never switches the active backend or pane.
use crate::{backend, Config};
use kmuxd::api::MachineSessions;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Entry {
    inventory: backend::Inventory,
    checked: Option<Instant>,
    loading: bool,
    unavailable: bool,
}
static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
const TTL: Duration = Duration::from_secs(15);

pub fn invalidate(machine: &str) {
    if let Some(cache) = CACHE.get() {
        if let Some(entry) = cache.lock().unwrap().get_mut(machine) {
            entry.checked = None;
        }
    }
}

pub fn snapshot(
    cfg: &Config,
    active: &str,
    inventory: &backend::Inventory,
) -> Vec<MachineSessions> {
    let cache = CACHE.get_or_init(Default::default);
    let mut entries = cache.lock().unwrap();
    cfg.hosts
        .iter()
        .map(|host| {
            let entry = entries.entry(host.id.clone()).or_default();
            if host.id == active {
                // Active inventory is authoritative and was already fetched this request.
                return machine_inventory(&host.id, inventory, false, false);
            }
            if !entry.loading && entry.checked.is_none_or(|t| t.elapsed() >= TTL) {
                entry.loading = true;
                let cfg = cfg.clone();
                let host = host.clone();
                std::thread::spawn(move || {
                    let result = backend::discover_inventory(&cfg, &host);
                    let mut entries = CACHE.get().unwrap().lock().unwrap();
                    let entry = entries.entry(host.id).or_default();
                    entry.unavailable = result.is_err();
                    // Do not offer stale session targets when a host becomes unavailable.
                    entry.inventory = result.unwrap_or_default();
                    entry.loading = false;
                    entry.checked = Some(Instant::now());
                });
            }
            machine_inventory(&host.id, &entry.inventory, entry.loading, entry.unavailable)
        })
        .collect()
}

fn machine_inventory(
    machine: &str,
    inventory: &backend::Inventory,
    loading: bool,
    unavailable: bool,
) -> MachineSessions {
    MachineSessions {
        machine: machine.into(),
        sessions: inventory.sessions.clone(),
        tabs: inventory.tabs.clone(),
        panes: inventory.panes.clone(),
        agent_panes: inventory
            .panes
            .iter()
            .filter(|p| p.agent.is_some())
            .cloned()
            .collect(),
        loading,
        unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_includes_all_panes_and_tabs_plus_agent_subset() {
        let inventory: backend::Inventory = serde_json::from_value(serde_json::json!({
            "sessions":[{"id":"s","name":"Project"}],
            "tabs":[{"id":"t","session":"s","name":"Review","agents":{"state":"blocked","count":1}},
                {"id":"shell","session":"s","name":"Shell"}],
            "panes":[{"id":"p","session":"s","tab":"t","cwd":"/tmp","command":"codex",
                "agent":{"name":"codex","state":"blocked","source":"herdr"}},
                {"id":"plain","session":"s","tab":"shell","cwd":"/tmp","command":"sh"}]
        }))
        .unwrap();
        let group = machine_inventory("devbox", &inventory, true, false);
        assert_eq!(group.machine, "devbox");
        assert_eq!(group.sessions[0].name, "Project");
        assert_eq!(group.panes.len(), 2);
        assert_eq!(group.agent_panes.len(), 1);
        assert_eq!(group.tabs.len(), 2);
        assert_eq!(group.tabs[0].name, "Review");
        assert_eq!(group.tabs[1].name, "Shell");
        assert!(group.loading);
        let empty = machine_inventory("offline", &backend::Inventory::default(), false, true);
        assert!(empty.unavailable && empty.panes.is_empty() && empty.agent_panes.is_empty());
    }
}
