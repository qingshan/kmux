//! Sized terminal processes for tmux control mode and Herdr attachment.
use crate::{config::Config, hosts};
use std::fs::File;
use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

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
pub(crate) fn spawn_tmux(cfg: &Config, host: &hosts::Host) -> Result<(Child, i32), String> {
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
