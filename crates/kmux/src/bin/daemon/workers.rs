//! Event polling and Oasis input readers feed the serialized dispatcher.
use super::{state::*, CONFIG_PATH};
use kmuxd::config;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

pub(crate) fn poll_thread() {
    let proxy = kmuxd::proxy::Client::new(Path::new(CONFIG_PATH));
    while KEEP_RUNNING.load(Ordering::SeqCst) {
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
    let fd = unsafe { libc::open(cdev.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
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
            std::thread::sleep(std::time::Duration::from_millis(20));
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
