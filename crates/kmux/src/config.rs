//! kmuxd settings at `/mnt/us/kmux/var/config.json`: proxy URL, shared
//! token, an optional SOCKS5 proxy, and the terminal size.
//! and the terminal size. Lenient load (defaults on missing/corrupt), atomic
//! save.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_proxy_url")]
    pub proxy_url: String,
    #[serde(default)]
    pub token: String,
    #[serde(default = "default_socks5")]
    pub socks5: String,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
    /// Oasis page-turn buttons input device (gpio-keys). kmuxd reads it
    /// directly since the framework consumes the keys before the WebKit.
    #[serde(default = "default_buttons_device")]
    pub buttons_device: String,
    /// Last kmux-proxy host id (`/stream?host=`). Empty → proxy default.
    #[serde(default)]
    pub active_host: String,
}

fn default_buttons_device() -> String {
    "/dev/input/event3".to_string()
}

pub fn default_proxy_url() -> String {
    "https://kmux.qingshan.dev".to_string()
}
pub fn default_socks5() -> String {
    String::new()
}
pub fn default_cols() -> u16 {
    80
}
pub fn default_rows() -> u16 {
    24
}

impl Default for Config {
    fn default() -> Self {
        Config {
            proxy_url: default_proxy_url(),
            token: String::new(),
            socks5: default_socks5(),
            cols: default_cols(),
            rows: default_rows(),
            buttons_device: default_buttons_device(),
            active_host: String::new(),
        }
    }
}

/// `GET /stream` URL. `host` empty omits the query param (proxy default).
pub fn stream_url(proxy_url: &str, token: &str, host: &str, t: u64) -> String {
    let base = proxy_url.trim_end_matches('/');
    if host.is_empty() {
        format!("{base}/stream?token={token}&t={t}")
    } else {
        format!("{base}/stream?token={token}&host={host}&t={t}")
    }
}

/// `GET /hosts` URL (host inventory; no secrets in the response).
pub fn hosts_url(proxy_url: &str, token: &str, t: u64) -> String {
    format!(
        "{}/hosts?token={}&t={}",
        proxy_url.trim_end_matches('/'),
        token,
        t
    )
}

pub fn load(path: &Path) -> Config {
    let Ok(text) = fs::read_to_string(path) else {
        return Config::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save(path: &Path, config: &Config) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    let tmp = std::path::PathBuf::from(format!("{}.tmp", path.display()));
    fs::write(&tmp, json).map_err(|e| format!("write {}: {}", tmp.display(), e))?;
    fs::rename(&tmp, path)
        .map_err(|e| format!("rename {} -> {}: {}", tmp.display(), path.display(), e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = Config::default();
        assert_eq!(c.cols, 80);
        assert_eq!(c.rows, 24);
        assert_eq!(c.socks5, "");
        assert_eq!(c.proxy_url, "https://kmux.qingshan.dev");
    }

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("kmux-config-{}", crate::now_epoch()));
        let path = dir.join("config.json");
        let c = Config {
            proxy_url: "http://x:1".into(),
            token: "t".into(),
            socks5: "".into(),
            cols: 100,
            rows: 40,
            buttons_device: "/dev/input/event3".into(),
            active_host: "home".into(),
        };
        save(&path, &c).unwrap();
        assert_eq!(load(&path), c);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stream_url_omits_empty_host() {
        assert_eq!(
            stream_url("http://x:1/", "tok", "", 9),
            "http://x:1/stream?token=tok&t=9"
        );
        assert_eq!(
            stream_url("http://x:1", "tok", "home", 9),
            "http://x:1/stream?token=tok&host=home&t=9"
        );
    }
}
