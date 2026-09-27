//! Shared runtime state and atomic publication of WAF status.
use super::{Op, LOG_PATH, STATUS_DIR, STATUS_PATH};
use kmuxd::status::Status;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc::Sender, Mutex, OnceLock};

pub(crate) static KEEP_RUNNING: AtomicBool = AtomicBool::new(true);
pub(crate) static WATCHING: AtomicBool = AtomicBool::new(false);
pub(crate) static POLL_PENDING: AtomicBool = AtomicBool::new(false);
pub(crate) static OP_TX: OnceLock<Sender<Op>> = OnceLock::new();
static STATUS: OnceLock<Mutex<Status>> = OnceLock::new();
static LAST_CMD: Mutex<Option<String>> = Mutex::new(None);
pub(crate) fn status_mutex() -> &'static Mutex<Status> {
    STATUS.get_or_init(|| Mutex::new(Status::default()))
}
pub(crate) fn last_cmd() -> std::sync::MutexGuard<'static, Option<String>> {
    LAST_CMD.lock().unwrap_or_else(|p| p.into_inner())
}
pub(crate) fn op_send(op: Op) {
    if let Some(tx) = OP_TX.get() {
        let _ = tx.send(op);
    }
}
pub(crate) fn copy_mode() -> bool {
    status_mutex().lock().unwrap().copy_mode
}
pub(crate) fn write_status_file() {
    let json = serde_json::to_vec(&*status_mutex().lock().unwrap()).unwrap();
    let _ = std::fs::create_dir_all(STATUS_DIR);
    let tmp = format!("{STATUS_PATH}.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(tmp, STATUS_PATH);
    }
}
pub(crate) fn set_status(f: impl FnOnce(&mut Status)) {
    {
        let mut s = status_mutex().lock().unwrap();
        f(&mut s);
        s.updated_at = kmuxd::now_epoch();
    }
    write_status_file();
}
pub(crate) fn log_line(msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_PATH)
    {
        let _ = writeln!(f, "[{}] {msg}", kmuxd::now_epoch());
    }
}
pub(crate) fn scroll_by(n: usize) {
    set_status(|s| kmuxd::buttons::scroll(s, true, n, kmuxd::now_epoch() * 1000));
}
pub(crate) fn scroll_down(n: usize) {
    set_status(|s| kmuxd::buttons::scroll(s, false, n, kmuxd::now_epoch() * 1000));
}
pub(crate) fn page_rows() -> usize {
    usize::from(status_mutex().lock().unwrap().rows.max(1))
}
