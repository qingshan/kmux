use serde::Deserialize;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

mod backend;
mod catalog;
mod copy;
mod diagnose;
mod events;
mod hosts;
mod unified;

const MAX_BODY: usize = 1 << 20;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    /// Legacy single-host ssh target; wrapped into `hosts` on load.
    #[serde(default)]
    target: String,
    #[serde(default)]
    session: String,
    token: String,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default)]
    bind: String,
    #[serde(default = "default_cols")]
    cols: u16,
    #[serde(default = "default_rows")]
    rows: u16,
    #[serde(default)]
    default_host: String,
    #[serde(default)]
    hosts: Vec<hosts::Host>,
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

fn load_config(path: &str) -> Config {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {path}: {e}");
        std::process::exit(1);
    });
    let mut cfg: Config = serde_json::from_str(&text).unwrap_or_else(|e| {
        eprintln!("error: invalid config {path}: {e}");
        std::process::exit(1);
    });
    if cfg.token.is_empty() {
        eprintln!("error: token is required in {path}");
        std::process::exit(1);
    }
    if let Err(e) = hosts::normalize(
        &mut cfg.hosts,
        &mut cfg.default_host,
        &cfg.target,
        &cfg.session,
    ) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    cfg
}

fn tailnet_ip() -> String {
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

fn token_ok(cfg: &Config, given: &str) -> bool {
    !given.is_empty() && given == cfg.token
}

/// Allocate a pty sized cols x rows; returns (master_fd, slave_fd).
fn open_pty(cols: u16, rows: u16) -> Result<(i32, i32), String> {
    unsafe {
        let mut master: i32 = -1;
        let mut slave: i32 = -1;
        if libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        ) != 0
        {
            return Err(format!(
                "openpty failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        libc::ioctl(slave, libc::TIOCSWINSZ, &ws);
        Ok((master, slave))
    }
}

/// Spawn a control-mode tmux client under a sized pty.
///
/// Normal targets use `ssh -tt <target>`. `local:<user>` is deliberately
/// handled without an SSH hop, for the machine that hosts the proxy itself.
/// That avoids a self-SSH control channel while still running tmux as the
/// configured login user.
/// Returns the child and the pty master.
fn spawn_tmux(cfg: &Config, host: &hosts::Host) -> Result<(Child, i32), String> {
    if !hosts::valid_id(&host.session) {
        return Err("invalid session name".into());
    }
    let (master, slave) = open_pty(cfg.cols, cfg.rows)?;
    // Recreate the session if it no longer exists (it exits with its last
    // pane), so the daemon's reconnect always has something to attach to.
    let tmux = if host.tmux_socket.is_empty() {
        "tmux".to_string()
    } else {
        format!(
            "tmux -L {}",
            kmuxd::tmux_keys::shell_quote(&host.tmux_socket)
        )
    };
    let remote = format!(
        "{tmux} has-session -t '{}' 2>/dev/null || {tmux} new-session -d -s '{}'; {tmux} -CC attach -t '{}'",
        host.session, host.session, host.session
    );
    spawn_on_pty(host, master, slave, &remote)
}

/// Attach a Herdr TUI client at cols×rows. Herdr has no socket method that
/// sets pane cell size; an attached client is what actually resizes the
/// workspace. Kept alive by the active `Link` and killed on drop.
pub(crate) fn spawn_herdr_ui(
    host: &hosts::Host,
    cols: u16,
    rows: u16,
) -> Result<(Child, i32), String> {
    let (master, slave) = open_pty(cols, rows)?;
    let session = if host.herdr_session.is_empty() {
        String::new()
    } else {
        format!(
            " --session {}",
            kmuxd::tmux_keys::shell_quote(&host.herdr_session)
        )
    };
    let socket = if host.herdr_socket.is_empty() {
        String::new()
    } else {
        format!(
            "HERDR_SOCKET_PATH={} ",
            kmuxd::tmux_keys::shell_quote(&host.herdr_socket)
        )
    };
    let remote = format!("{socket}herdr{session}");
    spawn_on_pty(host, master, slave, &remote)
}

fn spawn_on_pty(
    host: &hosts::Host,
    master: i32,
    slave: i32,
    remote: &str,
) -> Result<(Child, i32), String> {
    let mut cmd = if let Some(user) = host.target.strip_prefix("local:") {
        if user.is_empty()
            || !user
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            unsafe { libc::close(master) };
            unsafe { libc::close(slave) };
            return Err(format!("invalid local user for host {}", host.id));
        }
        let mut cmd = Command::new("sudo");
        // kmux-proxy normally runs under systemd, whose environment has no
        // TERM. tmux control mode still requires one, unlike the SSH path
        // where the client supplies it automatically.
        cmd.arg("-n")
            .arg("-u")
            .arg(user)
            .arg("env")
            .arg("TERM=xterm-256color")
            .arg("sh")
            .arg("-c");
        cmd.arg(format!("export PATH=\"$PATH:$HOME/.local/bin\"; {remote}"));
        cmd
    } else {
        let mut cmd = Command::new("ssh");
        cmd.arg("-tt")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=8")
            .arg("-o")
            .arg("StrictHostKeyChecking=accept-new");
        cmd.arg(&host.target).arg(remote);
        cmd
    };
    // SAFETY: slave fd is owned by us; make it the child's controlling
    // terminal, then dup it for stdin/stdout/stderr. A plain set of stdio
    // descriptors is not sufficient for tmux control mode when the proxy is
    // started by systemd (there is no inherited controlling terminal).
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(slave, libc::TIOCSCTTY, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
        let stdin = File::from_raw_fd(libc::dup(slave));
        let stdout = File::from_raw_fd(libc::dup(slave));
        let stderr = File::from_raw_fd(libc::dup(slave));
        cmd.stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
    }
    let child = cmd.spawn().map_err(|e| format!("spawn ssh: {e}"))?;
    unsafe { libc::close(slave) };
    Ok((child, master))
}

fn handle(mut stream: TcpStream, cfg: Config) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let result = read_request(&mut stream);
    let (code, value) = match result {
        Err(e) => ("400 Bad Request", serde_json::json!({"error":e})),
        Ok((method, path, _body)) if method == "GET" && path == "/health" => (
            "200 OK",
            serde_json::json!({"ok":true,"version":kmuxd::api::VERSION}),
        ),
        Ok((method, path, body)) if method == "POST" && path == "/v1/changes" => {
            match serde_json::from_slice::<kmuxd::api::ChangesRequest>(&body) {
                Err(e) => (
                    "400 Bad Request",
                    serde_json::json!({"error": e.to_string()}),
                ),
                Ok(r) if !token_ok(&cfg, &r.token) => (
                    "401 Unauthorized",
                    serde_json::json!({"error":"unauthorized"}),
                ),
                Ok(r) => match unified::changes(&cfg, r) {
                    Ok(value) => ("200 OK", serde_json::to_value(value).unwrap()),
                    Err(e) => ("409 Conflict", serde_json::json!({"error": e})),
                },
            }
        }
        Ok((method, path, body)) if method == "POST" && path == "/v1/action" => {
            match serde_json::from_slice::<kmuxd::api::Request>(&body) {
                Err(e) => (
                    "400 Bad Request",
                    serde_json::json!({"error":e.to_string()}),
                ),
                Ok(r) if !token_ok(&cfg, &r.token) => (
                    "401 Unauthorized",
                    serde_json::json!({"error":"unauthorized"}),
                ),
                Ok(r) => {
                    if matches!(r.action, kmuxd::api::Action::Diagnose) {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(25)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(25)));
                    }
                    match unified::request(&cfg, r) {
                        Ok(snapshot) => ("200 OK", serde_json::to_value(snapshot).unwrap()),
                        Err(e) => ("409 Conflict", serde_json::json!({"error": e})),
                    }
                }
            }
        }
        _ => (
            "404 Not Found",
            serde_json::json!({"error":"use POST /v1/action with API version 1"}),
        ),
    };
    let body = value.to_string();
    let _=write!(stream,"HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
}
fn read_request(stream: &mut impl Read) -> Result<(String, String, Vec<u8>), String> {
    let mut head = Vec::new();
    let mut byte = [0; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16384 {
            return Err("headers too large".into());
        }
        stream.read_exact(&mut byte).map_err(|e| e.to_string())?;
        head.push(byte[0]);
    }
    let text = std::str::from_utf8(&head).map_err(|e| e.to_string())?;
    let mut lines = text.split("\r\n");
    let parts: Vec<_> = lines.next().unwrap_or("").split_whitespace().collect();
    if parts.len() != 3 {
        return Err("invalid request".into());
    }
    let mut length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("transfer-encoding") {
                return Err("chunked requests unsupported".into());
            }
            if name.eq_ignore_ascii_case("content-length") {
                if length.is_some() {
                    return Err("duplicate content-length".into());
                }
                length = Some(
                    value
                        .trim()
                        .parse::<usize>()
                        .map_err(|_| "bad content length")?,
                );
            }
        }
    }
    let length = length.unwrap_or(0);
    if length > MAX_BODY {
        return Err("request too large".into());
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok((parts[0].into(), parts[1].into(), body))
}
fn main() {
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/etc/kmux-proxy.json".to_string());
    let mut cfg = load_config(&config_path);
    if cfg.bind.is_empty() {
        cfg.bind = tailnet_ip();
    }
    let listener = TcpListener::bind((cfg.bind.as_str(), cfg.port)).unwrap_or_else(|e| {
        eprintln!("error: bind {}:{}: {e}", cfg.bind, cfg.port);
        std::process::exit(1);
    });
    println!("kmux-proxy listening on {}:{}", cfg.bind, cfg.port);
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let cfg = cfg.clone();
                std::thread::spawn(move || handle(s, cfg));
            }
            Err(_) => continue,
        }
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    #[test]
    fn reads_full_unicode_body_even_when_fragmented() {
        struct Fragmented(std::io::Cursor<Vec<u8>>);
        impl Read for Fragmented {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = buf.len().min(2);
                self.0.read(&mut buf[..n])
            }
        }
        let body = r#"{"text":"λ\nquotes"}"#;
        let bytes = format!(
            "POST /v1/action HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes();
        let (method, path, actual) =
            read_request(&mut Fragmented(std::io::Cursor::new(bytes))).unwrap();
        assert_eq!(method, "POST");
        assert_eq!(path, "/v1/action");
        assert_eq!(actual, body.as_bytes());
    }
    #[test]
    fn rejects_ambiguous_or_oversized_bodies() {
        for header in [
            "Content-Length: 0\r\nContent-Length: 1",
            "Content-Length: 1048577",
            "Transfer-Encoding: chunked",
        ] {
            let bytes = format!("POST /v1/action HTTP/1.1\r\n{header}\r\n\r\n");
            assert!(read_request(&mut bytes.as_bytes()).is_err());
        }
    }
}
