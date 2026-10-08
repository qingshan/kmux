//! Curl transport for the proxy API, independent of Kindle LIPC.
use crate::api::{Action, ChangesRequest, Request, Snapshot, VERSION};
use crate::config::{self, Config};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const CURL_TIMEOUT: &str = "10";

/// The configuration path is explicit so recovery can be exercised on a host.
pub struct Client<'a> {
    config_path: &'a Path,
}

impl<'a> Client<'a> {
    pub fn new(config_path: &'a Path) -> Self {
        Self { config_path }
    }
    pub fn request(
        &self,
        cfg: &Config,
        action: Action,
        expected_pane: Option<String>,
    ) -> Result<Snapshot, String> {
        self.request_timed(cfg, action, expected_pane, CURL_TIMEOUT)
    }
    pub fn request_timed(
        &self,
        cfg: &Config,
        action: Action,
        expected_pane: Option<String>,
        timeout: &str,
    ) -> Result<Snapshot, String> {
        self.request_using(cfg, action, expected_pane, |cfg, action, expected| {
            request_once(cfg, action, expected, timeout)
        })
    }

    fn request_using(
        &self,
        cfg: &Config,
        action: Action,
        expected_pane: Option<String>,
        mut send: impl FnMut(&Config, Action, Option<String>) -> Result<Snapshot, String>,
    ) -> Result<Snapshot, String> {
        let result = send(cfg, action.clone(), expected_pane.clone());
        match result {
            Err(error)
                if error == "unknown machine"
                    && !cfg.active_host.is_empty()
                    && matches!(&action, Action::Snapshot) =>
            {
                // The proxy's configured host list may change while the Kindle
                // still remembers the last selected machine. Recover only on a
                // read-only snapshot, so stale user actions are never replayed
                // against the proxy's default host.
                let mut fallback = cfg.clone();
                fallback.active_host.clear();
                config::save(self.config_path, &fallback)?;
                send(&fallback, action, expected_pane)
            }
            result => result,
        }
    }

    pub fn wait_for_changes(
        &self,
        cfg: &Config,
        machine: String,
        after: u64,
    ) -> Result<(), String> {
        let body = ChangesRequest {
            version: VERSION,
            token: cfg.token.clone(),
            machine,
            after,
            timeout_ms: 650,
        };
        http_json(
            cfg,
            "/v1/changes",
            &serde_json::to_value(body).unwrap(),
            CURL_TIMEOUT,
        )
        .map(|_| ())
    }
}

fn request_once(
    cfg: &Config,
    action: Action,
    expected_pane: Option<String>,
    timeout: &str,
) -> Result<Snapshot, String> {
    let request = Request {
        version: VERSION,
        token: cfg.token.clone(),
        machine: cfg.active_host.clone(),
        expected_pane,
        action,
    };
    let value = http_json(
        cfg,
        "/v1/action",
        &serde_json::to_value(request).unwrap(),
        timeout,
    )?;
    let snapshot: Snapshot =
        serde_json::from_value(value).map_err(|e| format!("incompatible proxy: {e}"))?;
    if snapshot.version != VERSION {
        return Err("API version mismatch".into());
    }
    Ok(snapshot)
}
fn http_json(
    cfg: &Config,
    path: &str,
    body: &serde_json::Value,
    timeout: &str,
) -> Result<serde_json::Value, String> {
    let mut args = vec![
        "-sS".into(),
        "-m".into(),
        timeout.into(),
        "-H".into(),
        "Content-Type: application/json".into(),
        "-d".into(),
        "@-".into(),
        "-w".into(),
        "\n%{http_code}".into(),
    ];
    if !cfg.socks5.is_empty() {
        args.extend(["--socks5-hostname".into(), cfg.socks5.clone()]);
    }
    args.push(format!("{}{path}", cfg.proxy_url.trim_end_matches('/')));
    let mut child = Command::new("curl")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("curl stdin missing")?
        .write_all(&serde_json::to_vec(body).unwrap())
        .map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(match out.status.code() {
            Some(5 | 6) => "Cannot resolve proxy hostname. Check DNS and Wi-Fi.",
            Some(7) => "Cannot connect to proxy. Check the server and network.",
            Some(28) => "Proxy request timed out. Retrying automatically.",
            Some(35 | 51 | 60) => {
                "Proxy HTTPS connection failed. Check the certificate and device clock."
            }
            _ => "Proxy connection failed. Check the network and retry.",
        }
        .into());
    }
    parse_http_response(&out.stdout)
}

fn parse_http_response(output: &[u8]) -> Result<serde_json::Value, String> {
    let split = output
        .iter()
        .rposition(|b| *b == b'\n')
        .ok_or("Missing proxy HTTP status")?;
    let code = std::str::from_utf8(&output[split + 1..])
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or("Invalid proxy HTTP status")?;
    let value = serde_json::from_slice::<serde_json::Value>(&output[..split]);
    if !(200..300).contains(&code) {
        // Preserve structured API errors (including stale-host recovery), but
        // never display an HTML gateway page or arbitrary response body.
        if code < 500 {
            if let Ok(ref value) = value {
                if let Some(error) = value.get("error").and_then(|v| v.as_str()) {
                    return Err(error.into());
                }
            }
        }
        return Err(format!(
            "Proxy unavailable — HTTP {code}. Check the proxy URL and server."
        ));
    }
    let value = value
        .map_err(|_| "Proxy returned an invalid JSON response. Check the proxy URL.".to_string())?;
    if let Some(error) = value.get("error") {
        return Err(error.as_str().unwrap_or("proxy error").into());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn gateway_errors_report_http_status_without_response_body() {
        for code in [502, 503, 530] {
            let response = format!("<html>private gateway details</html>\n{code}");
            let error = parse_http_response(response.as_bytes()).unwrap_err();
            assert!(error.contains(&format!("HTTP {code}")));
            assert!(!error.contains("private"));
        }
        assert!(parse_http_response(b"not JSON\n200")
            .unwrap_err()
            .contains("invalid JSON"));
        assert_eq!(
            parse_http_response(b"{\"error\":\"unknown machine\"}\n404").unwrap_err(),
            "unknown machine"
        );
        assert_eq!(
            parse_http_response(b"{\"ok\":true}\n200").unwrap()["ok"],
            true
        );
    }

    struct Fixture {
        path: PathBuf,
        cfg: Config,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir()
                .join(format!(
                    "kmux-proxy-client-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ))
                .join("config.json");
            let cfg = Config {
                active_host: "removed-demo".into(),
                token: "fixture".into(),
                ..Config::default()
            };
            config::save(&path, &cfg).unwrap();
            Self { path, cfg }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.path.parent().unwrap());
        }
    }

    #[test]
    fn stale_snapshot_recovers_once_and_persists_default_host() {
        let fixture = Fixture::new();
        let client = Client::new(&fixture.path);
        let mut calls = 0;
        let result = client
            .request_using(
                &fixture.cfg,
                Action::Snapshot,
                None,
                |cfg, action, expected| {
                    calls += 1;
                    assert!(matches!(action, Action::Snapshot));
                    assert!(expected.is_none());
                    if calls == 1 {
                        assert_eq!(cfg.active_host, "removed-demo");
                        return Err("unknown machine".into());
                    }
                    assert!(cfg.active_host.is_empty());
                    Ok(Snapshot {
                        version: VERSION,
                        machine: "default".into(),
                        machines: vec![],
                        sessions: vec![],
                        tabs: vec![],
                        panes: vec![],
                        selection: Default::default(),
                        terminal: Default::default(),
                    })
                },
            )
            .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(result.machine, "default");
        let mut expected = fixture.cfg.clone();
        expected.active_host.clear();
        assert_eq!(config::load(&fixture.path), expected);
    }

    #[test]
    fn stale_user_actions_are_never_replayed_on_another_host() {
        let fixture = Fixture::new();
        let client = Client::new(&fixture.path);
        for action in [
            Action::Text {
                text: "hello".into(),
                paste: false,
            },
            Action::Key {
                key: "Enter".into(),
            },
            Action::CloseTab,
            Action::Select {
                session: "s1".into(),
                tab: "t1".into(),
                pane: "p1".into(),
            },
        ] {
            let mut calls = 0;
            let result = client.request_using(
                &fixture.cfg,
                action,
                Some("p1".into()),
                |cfg, _, expected| {
                    calls += 1;
                    assert_eq!(cfg.active_host, fixture.cfg.active_host);
                    assert_eq!(expected.as_deref(), Some("p1"));
                    Err("unknown machine".into())
                },
            );
            assert_eq!(result.unwrap_err(), "unknown machine");
            assert_eq!(calls, 1);
            assert_eq!(config::load(&fixture.path), fixture.cfg);
        }
    }

    #[test]
    fn recovery_does_not_retry_other_errors_or_loop_on_default_failure() {
        let fixture = Fixture::new();
        let client = Client::new(&fixture.path);
        let mut calls = 0;
        let result = client.request_using(&fixture.cfg, Action::Snapshot, None, |_, _, _| {
            calls += 1;
            Err("connection timed out".into())
        });
        assert_eq!(result.unwrap_err(), "connection timed out");
        assert_eq!(calls, 1);
        assert_eq!(config::load(&fixture.path), fixture.cfg);
        calls = 0;
        let result = client.request_using(&fixture.cfg, Action::Snapshot, None, |_, _, _| {
            calls += 1;
            Err("unknown machine".into())
        });
        assert_eq!(result.unwrap_err(), "unknown machine");
        assert_eq!(calls, 2);
    }
}
