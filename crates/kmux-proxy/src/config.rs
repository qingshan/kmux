//! Configuration loading, defaults, and legacy host normalization.
use crate::hosts;
use serde::Deserialize;
use std::process::Command;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Config {
    /// Legacy single-host ssh target; wrapped into `hosts` on load.
    #[serde(default)]
    target: String,
    #[serde(default)]
    session: String,
    pub(crate) token: String,
    #[serde(default = "default_port")]
    pub(crate) port: u16,
    #[serde(default)]
    pub(crate) bind: String,
    #[serde(default = "default_cols")]
    pub(crate) cols: u16,
    #[serde(default = "default_rows")]
    pub(crate) rows: u16,
    #[serde(default)]
    pub(crate) default_host: String,
    #[serde(default)]
    pub(crate) hosts: Vec<hosts::Host>,
}

fn default_port() -> u16 {
    8766
}
fn default_cols() -> u16 {
    80
}
fn default_rows() -> u16 {
    24
}

pub(crate) fn load(path: &str) -> Result<Config, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    parse(&text).map_err(|e| format!("invalid config {path}: {e}"))
}

fn parse(text: &str) -> Result<Config, String> {
    let mut cfg: Config = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if cfg.token.is_empty() {
        return Err("token is required".into());
    }
    hosts::normalize(
        &mut cfg.hosts,
        &mut cfg.default_host,
        &cfg.target,
        &cfg.session,
    )?;
    Ok(cfg)
}

pub(crate) fn tailnet_ip() -> String {
    if let Ok(out) = Command::new("tailscale").args(["ip", "-4"]).output() {
        let s = String::from_utf8_lossy(&out.stdout);
        for ip in s.split_whitespace() {
            if ip.starts_with("100.") {
                return ip.to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_config_keeps_defaults_and_normalizes_inventory() {
        let cfg = parse(r#"{"token":"fixture","target":"devbox","session":"work"}"#).unwrap();
        assert_eq!((cfg.port, cfg.cols, cfg.rows), (8766, 80, 24));
        assert!(cfg.bind.is_empty());
        assert_eq!(cfg.default_host, "devbox");
        assert_eq!(cfg.hosts.len(), 1);
        assert_eq!(cfg.hosts[0].session, "work");
    }

    #[test]
    fn named_hosts_keep_backend_settings_and_choose_valid_default() {
        let cfg = parse(
            r#"{"token":"fixture","defaultHost":"removed","hosts":[
            {"id":"work","backend":"herdr","herdrSession":"coding"}
        ]}"#,
        )
        .unwrap();
        assert_eq!(cfg.default_host, "work");
        assert_eq!(cfg.hosts[0].target, "work");
        assert_eq!(cfg.hosts[0].backend, kmuxd::api::Backend::Herdr);
        assert_eq!(cfg.hosts[0].herdr_session, "coding");
    }

    #[test]
    fn invalid_config_returns_an_error_to_the_caller() {
        for text in [
            "{",
            r#"{"target":"devbox"}"#,
            r#"{"token":"","target":"devbox"}"#,
            r#"{"token":"fixture"}"#,
            r#"{"token":"fixture","hosts":[{"id":"invalid host"}]}"#,
        ] {
            assert!(parse(text).is_err());
        }
    }
}
