//! Event polling and Oasis input readers feed the serialized dispatcher.
use super::{state::*, CONFIG_PATH};
use kmuxd::config;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

fn app_is_foreground() -> bool {
    let screen_active = Command::new("lipc-get-prop")
        .args(["com.lab126.powerd", "state"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "active");
    if !screen_active {
        return false;
    }
    Command::new("lipc-get-prop")
        .args(["com.lab126.appmgrd", "activeApp"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "dev.qingshan.kmux")
}

pub(crate) fn poll_thread() {
    let proxy = kmuxd::proxy::Client::new(Path::new(CONFIG_PATH));
    let mut next_foreground_check = Instant::now();
    let mut foreground = false;
    while KEEP_RUNNING.load(Ordering::SeqCst) {
        if Instant::now() >= next_foreground_check {
            let active = app_is_foreground();
            if active != foreground {
                foreground = active;
                set_status(|s| s.ui_active = active);
            }
            next_foreground_check = Instant::now() + Duration::from_secs(5);
        }
        if !foreground {
            thread::sleep(Duration::from_secs(1));
            continue;
        }
        if !WATCHING.load(Ordering::SeqCst) || POLL_PENDING.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        let (events, revision, machine) = {
            let s = status_mutex().lock().unwrap();
            (
                s.event_updates && s.connected,
                s.event_revision,
                s.active_host.clone().unwrap_or_default(),
            )
        };
        if events {
            let cfg = config::load(Path::new(CONFIG_PATH));
            // Wait outside the input dispatcher. Timed capture also covers
            // Herdr output, which is not a raw event stream.
            if proxy.wait_for_changes(&cfg, machine, revision).is_err() {
                thread::sleep(Duration::from_millis(650));
            }
            // Coalesce event bursts for the e-ink screen.
            thread::sleep(Duration::from_millis(100));
        } else {
            thread::sleep(Duration::from_millis(750));
        }
        if WATCHING.load(Ordering::SeqCst) && !POLL_PENDING.swap(true, Ordering::SeqCst) {
            op_send(serde_json::json!({"op":"poll"}));
        }
    }
}
pub(crate) fn button_thread() {
    let device = config::load(Path::new(CONFIG_PATH)).buttons_device;
    let cdev = std::ffi::CString::new(device.as_str()).unwrap();
    let fd = unsafe { libc::open(cdev.as_ptr(), libc::O_RDONLY) };
    if fd < 0 {
        log_line(&format!("buttons: cannot open {}", device));
        return;
    }
    log_line(&format!("buttons: listening on {}", device));
    let mut ev: libc::input_event = unsafe { std::mem::zeroed() };
    while KEEP_RUNNING.load(Ordering::SeqCst) {
        let n = unsafe {
            libc::read(
                fd,
                &mut ev as *mut _ as *mut libc::c_void,
                std::mem::size_of::<libc::input_event>(),
            )
        };
        if n as usize != std::mem::size_of::<libc::input_event>() {
            continue;
        }
        if let Some(key) = kmuxd::buttons::key(ev.type_, ev.code, ev.value) {
            if WATCHING.load(Ordering::SeqCst) {
                // Serialize with snapshot application; never overwrite a scroll
                // from this reader thread while the dispatcher installs a capture.
                op_send(serde_json::json!({"op":"key","key":key}));
                log_line(&format!("buttons: {key}"));
            }
        }
    }
    unsafe { libc::close(fd) };
}
