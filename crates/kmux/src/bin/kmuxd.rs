//! Kindle LIPC/IO shell for the backend-neutral kmux proxy API.
use kmuxd::api::{Action, ChangesRequest, Request, Snapshot, VERSION};
use kmuxd::config::{self, Config};
use kmuxd::status::Status;
use kmuxd::{
    read_string_prop, write_string_prop, LIPC_ERROR_DUPLICATE_SERVICE_NAME, LIPC_ERROR_INVALID_ARG,
    LIPC_OK,
};
use std::ffi::{c_char, c_int, c_void, CString};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{
    mpsc::{self, Sender},
    Mutex, OnceLock,
};
use std::thread;
use std::time::Duration;
type Op = serde_json::Value;
const SERVICE_NAME: &str = "dev.qingshan.kmuxd";
const CONFIG_PATH: &str = "/mnt/us/kmux/var/config.json";
const STATUS_DIR: &str = "/var/local/mesquite/kmux";
const STATUS_PATH: &str = "/var/local/mesquite/kmux/status.json";
const LOG_PATH: &str = "/mnt/us/kmux/var/tmux_log.txt";
const CURL_TIMEOUT: &str = "10";

type LipcHandle = *mut c_void;
type LipcCallback = extern "C" fn(LipcHandle, *const c_char, *mut c_void, *mut c_void) -> c_int;

#[link(name = "lipc")]
extern "C" {
    fn LipcOpenEx(service: *const c_char, code: *mut c_int) -> LipcHandle;
    fn LipcClose(handle: LipcHandle) -> c_int;
    fn LipcRegisterStringProperty(
        lipc: LipcHandle,
        prop: *const c_char,
        getter: Option<LipcCallback>,
        setter: Option<LipcCallback>,
        data: *mut c_void,
    ) -> c_int;
}

static KEEP_RUNNING: AtomicBool = AtomicBool::new(true);
static WATCHING: AtomicBool = AtomicBool::new(false);
static POLL_PENDING: AtomicBool = AtomicBool::new(false);
static OP_TX: OnceLock<Sender<Op>> = OnceLock::new();
static STATUS: OnceLock<Mutex<Status>> = OnceLock::new();
static LAST_CMD: Mutex<Option<String>> = Mutex::new(None);
fn status_mutex() -> &'static Mutex<Status> {
    STATUS.get_or_init(|| Mutex::new(Status::default()))
}
fn last_cmd() -> std::sync::MutexGuard<'static, Option<String>> {
    LAST_CMD.lock().unwrap_or_else(|p| p.into_inner())
}
fn op_send(op: Op) {
    if let Some(tx) = OP_TX.get() {
        let _ = tx.send(op);
    }
}
fn copy_mode() -> bool {
    status_mutex().lock().unwrap().copy_mode
}
fn write_status_file() {
    let json = serde_json::to_vec(&*status_mutex().lock().unwrap()).unwrap();
    let _ = std::fs::create_dir_all(STATUS_DIR);
    let tmp = format!("{STATUS_PATH}.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(tmp, STATUS_PATH);
    }
}
fn set_status(f: impl FnOnce(&mut Status)) {
    {
        let mut s = status_mutex().lock().unwrap();
        f(&mut s);
        s.updated_at = kmuxd::now_epoch();
    }
    write_status_file();
}
fn log_line(msg: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_PATH)
    {
        let _ = writeln!(f, "[{}] {msg}", kmuxd::now_epoch());
    }
}
fn scroll_by(n: usize) {
    set_status(|s| kmuxd::buttons::scroll(s, true, n, kmuxd::now_epoch() * 1000));
}
fn scroll_down(n: usize) {
    set_status(|s| kmuxd::buttons::scroll(s, false, n, kmuxd::now_epoch() * 1000));
}
fn request(
    cfg: &Config,
    action: Action,
    expected_pane: Option<String>,
) -> Result<Snapshot, String> {
    request_timed(cfg, action, expected_pane, CURL_TIMEOUT)
}
fn request_timed(
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
        return Err(String::from_utf8_lossy(&out.stderr).into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    if let Some(error) = value.get("error") {
        return Err(error.as_str().unwrap_or("proxy error").into());
    }
    Ok(value)
}
fn poll_thread() {
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
            let body = ChangesRequest {
                version: VERSION,
                token: cfg.token.clone(),
                machine,
                after: revision,
                timeout_ms: 650,
            };
            // Wait on a separate worker, never the input dispatcher. A timeout
            // still captures terminal output: Herdr has no raw output subscription.
            if http_json(
                &cfg,
                "/v1/changes",
                &serde_json::to_value(body).unwrap(),
                CURL_TIMEOUT,
            )
            .is_err()
            {
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
fn dispatch(rx: mpsc::Receiver<Op>) {
    let mut selected_pane = None;
    for op in rx {
        let name = op["op"].as_str().unwrap_or("");
        let mut cfg = config::load(Path::new(CONFIG_PATH));
        match name {
            "clipboard_remember" => {
                if let Some(t) = op["text"]
                    .as_str()
                    .and_then(kmuxd::normalize_clipboard_text)
                {
                    set_status(|s| kmuxd::remember_clipboard(&mut s.clipboard_history, t));
                }
                continue;
            }
            "config_set" => {
                if let Some(value) = op["proxy_url"].as_str() {
                    cfg.proxy_url = value.into();
                }
                // An empty token means “keep the saved token” in the WAF.
                if let Some(value) = op["token"].as_str().filter(|v| !v.is_empty()) {
                    cfg.token = value.into();
                }
                if let Some(value) = op["socks5"].as_str() {
                    cfg.socks5 = value.into();
                }
                if let Err(e) = config::save(Path::new(CONFIG_PATH), &cfg) {
                    set_status(|s| s.last_error = Some(e));
                }
                set_status(|s| {
                    s.configured = !cfg.proxy_url.is_empty() && !cfg.token.is_empty();
                    s.proxy_url = cfg.proxy_url.clone();
                    s.socks5 = cfg.socks5.clone();
                });
                selected_pane = None;
                continue;
            }
            "host" | "host_session" | "host_pane" => {
                if name == "host_pane" {
                    if let Err(error) = kmuxd::client::action(&op) {
                        set_status(|s| s.last_error = Some(error));
                        continue;
                    }
                }
                cfg.active_host = op["id"].as_str().unwrap_or("").into();
                if let Err(e) = config::save(Path::new(CONFIG_PATH), &cfg) {
                    set_status(|s| s.last_error = Some(e));
                    continue;
                }
                selected_pane = None;
                set_status(|s| {
                    s.connected = false;
                    s.event_updates = false;
                    s.active_host = Some(cfg.active_host.clone());
                    s.screen.clear();
                    s.windows.clear();
                    s.panes.clear();
                    s.agent_panes.clear();
                    s.tab_agents.clear();
                    s.sessions.clear();
                    s.scrollback.clear();
                    s.copy_mode = false;
                    s.cwd = None;
                    s.files.clear();
                });
                WATCHING.store(true, Ordering::SeqCst);
            }
            "watch_start" => {
                WATCHING.store(true, Ordering::SeqCst);
            }
            "watch_stop" | "detach" => {
                WATCHING.store(false, Ordering::SeqCst);
                let _ = request(&cfg, Action::Detach, None);
                set_status(|s| {
                    s.connected = false;
                    s.copy_mode = false;
                });
                continue;
            }
            "scroll_to" => {
                let offset = op["offset"].as_u64().unwrap_or(0) as usize;
                set_status(|s| s.scroll_offset = offset.min(s.scrollback.len()));
                continue;
            }
            "diagnose" => {
                if cfg.proxy_url.is_empty() || cfg.token.is_empty() {
                    set_status(|s| {
                        s.diagnose = Some(kmuxd::api::DiagnoseReport {
                            running: false,
                            proxy: kmuxd::api::DiagnoseCheck {
                                ok: false,
                                detail: "Configure the proxy URL and token first".into(),
                            },
                            hosts: Vec::new(),
                        });
                    });
                    continue;
                }
                set_status(|s| {
                    s.diagnose = Some(kmuxd::api::DiagnoseReport {
                        running: true,
                        proxy: kmuxd::api::DiagnoseCheck {
                            ok: true,
                            detail: "checking…".into(),
                        },
                        hosts: Vec::new(),
                    });
                });
                match request_timed(&cfg, Action::Diagnose, None, "25") {
                    Ok(snapshot) => set_status(|s| {
                        s.diagnose = snapshot.terminal.diagnose;
                    }),
                    Err(error) => set_status(|s| {
                        s.diagnose = Some(kmuxd::api::DiagnoseReport {
                            running: false,
                            proxy: kmuxd::api::DiagnoseCheck {
                                ok: false,
                                detail: error,
                            },
                            hosts: Vec::new(),
                        });
                    }),
                }
                continue;
            }
            "key" if !copy_mode() => match op["key"].as_str().unwrap_or("") {
                "ScrollUp" => {
                    scroll_by(24);
                    continue;
                }
                "ScrollDown" => {
                    scroll_down(24);
                    continue;
                }
                "ScrollBottom" => {
                    scroll_down(usize::MAX);
                    continue;
                }
                _ => {}
            },
            "poll" => {
                POLL_PENDING.store(false, Ordering::SeqCst);
                if !WATCHING.load(Ordering::SeqCst) {
                    continue;
                }
            }
            _ => {}
        }
        if cfg.proxy_url.is_empty() || cfg.token.is_empty() {
            set_status(|s| {
                s.configured = false;
                s.last_error = Some("Configure the proxy URL and token in Settings".into());
            });
            continue;
        }
        let action = if name == "poll" || name == "host" {
            Ok(Action::Snapshot)
        } else {
            kmuxd::client::action(&op)
        };
        let result = action.and_then(|action| {
            if let Some(machine) = op["expected_machine"].as_str().filter(|m| !m.is_empty()) {
                let active = status_mutex()
                    .lock()
                    .unwrap()
                    .active_host
                    .clone()
                    .unwrap_or_default();
                if machine != active {
                    return Err("machine changed; refresh before sending input".into());
                }
            }
            let expected = if matches!(
                action,
                Action::Text { .. }
                    | Action::Key { .. }
                    | Action::CloseTab
                    | Action::CopyEnter
                    | Action::CopySearch { .. }
            ) {
                op["expected_pane"]
                    .as_str()
                    .map(String::from)
                    .or_else(|| selected_pane.clone())
            } else {
                None
            };
            request(&cfg, action, expected)
        });
        match result {
            Ok(snapshot) => {
                selected_pane = Some(snapshot.selection.pane.clone());
                let old = status_mutex().lock().unwrap().clone();
                let new = kmuxd::client::status(snapshot, &old);
                set_status(|s| *s = new);
            }
            Err(e) => {
                log_line(&format!("proxy: {e}"));
                set_status(|s| {
                    s.last_error = Some(e);
                    if name == "poll" || name == "host" || name == "watch_start" {
                        s.connected = false;
                    }
                });
            }
        }
    }
}
fn button_thread() {
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

extern "C" fn cmd_getter(
    _h: LipcHandle,
    _p: *const c_char,
    value: *mut c_void,
    data: *mut c_void,
) -> c_int {
    let out = last_cmd()
        .clone()
        .unwrap_or_else(|| "No command yet.".to_string());
    unsafe { write_string_prop(value, data, &out) }
}

extern "C" fn cmd_setter(
    _h: LipcHandle,
    _p: *const c_char,
    value: *mut c_void,
    _d: *mut c_void,
) -> c_int {
    let Some(cmd) = (unsafe { read_string_prop(value) }) else {
        return LIPC_ERROR_INVALID_ARG;
    };
    *last_cmd() = Some(cmd.clone());
    match serde_json::from_str::<Op>(&cmd) {
        Ok(op) => {
            if let Some(tx) = OP_TX.get() {
                let _ = tx.send(op);
            }
        }
        Err(e) => set_status(|s| {
            s.last_error = Some(format!("invalid command: {}", e));
        }),
    }
    LIPC_OK
}

extern "C" fn status_getter(
    _h: LipcHandle,
    _p: *const c_char,
    value: *mut c_void,
    data: *mut c_void,
) -> c_int {
    let json = serde_json::to_string(&*status_mutex().lock().unwrap_or_else(|p| p.into_inner()))
        .unwrap_or_else(|_| "{}".to_string());
    unsafe { write_string_prop(value, data, &json) }
}

extern "C" fn exit_getter(
    _h: LipcHandle,
    _p: *const c_char,
    value: *mut c_void,
    data: *mut c_void,
) -> c_int {
    unsafe { write_string_prop(value, data, "Write into this property to exit kmuxd") }
}

extern "C" fn exit_setter(
    _h: LipcHandle,
    _p: *const c_char,
    _value: *mut c_void,
    _d: *mut c_void,
) -> c_int {
    KEEP_RUNNING.store(false, Ordering::SeqCst);
    LIPC_OK
}

extern "C" fn info_getter(
    _h: LipcHandle,
    _p: *const c_char,
    value: *mut c_void,
    data: *mut c_void,
) -> c_int {
    let msg = format!(
        "Build Info: Branch: {}, Commit: {}, Built On: {}",
        env!("GIT_BRANCH"),
        env!("GIT_COMMIT"),
        env!("BUILD_TIME")
    );
    unsafe { write_string_prop(value, data, &msg) }
}

fn daemonize() {
    unsafe {
        if libc::fork() > 0 {
            std::process::exit(0);
        }
        if libc::setsid() < 0 {
            std::process::exit(1);
        }
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        if libc::fork() > 0 {
            std::process::exit(0);
        }
        libc::umask(0o000);
        libc::chdir(c"/mnt/us/kmux".as_ptr());
        for fd in 0..libc::sysconf(libc::_SC_OPEN_MAX) {
            libc::close(fd as c_int);
        }
    }
}

extern "C" fn handle_signal(_sig: c_int) {
    KEEP_RUNNING.store(false, Ordering::SeqCst);
}

fn main() {
    let run_as_daemon = !std::env::args()
        .skip(1)
        .any(|a| a == "-n" || a == "--no-daemon");
    if run_as_daemon {
        println!("Forking into the background.");
        daemonize();
    } else {
        println!("Running in foreground mode.");
        unsafe {
            libc::signal(
                libc::SIGINT,
                handle_signal as *const () as libc::sighandler_t,
            );
            libc::signal(
                libc::SIGTERM,
                handle_signal as *const () as libc::sighandler_t,
            );
        }
    }

    let (tx, rx) = mpsc::channel::<Op>();
    let _ = OP_TX.set(tx);
    thread::spawn(move || dispatch(rx));

    let service = CString::new(SERVICE_NAME).expect("static, no NUL");
    let mut code: c_int = -1;
    let handle = unsafe { LipcOpenEx(service.as_ptr(), &mut code) };
    if code != LIPC_OK {
        if code == LIPC_ERROR_DUPLICATE_SERVICE_NAME {
            return;
        }
        eprintln!("Failed to open LIPC (code {code})");
        std::process::exit(1);
    }

    status_mutex();
    let cfg = config::load(Path::new(CONFIG_PATH));
    set_status(|s| {
        s.configured = !cfg.proxy_url.is_empty() && !cfg.token.is_empty();
        s.proxy_url = cfg.proxy_url.clone();
        s.socks5 = cfg.socks5.clone();
        s.cols = cfg.cols;
        s.rows = cfg.rows;
        if !cfg.active_host.is_empty() {
            s.active_host = Some(cfg.active_host.clone());
        }
    });
    write_status_file();
    log_line("daemon start");

    let props: [(&str, Option<LipcCallback>, Option<LipcCallback>); 4] = [
        ("cmd", Some(cmd_getter), Some(cmd_setter)),
        ("status", Some(status_getter), None),
        ("exit", Some(exit_getter), Some(exit_setter)),
        ("info", Some(info_getter), None),
    ];
    for (prop, getter, setter) in props {
        let prop = CString::new(prop).expect("static, no NUL");
        unsafe {
            LipcRegisterStringProperty(handle, prop.as_ptr(), getter, setter, std::ptr::null_mut())
        };
    }

    std::thread::spawn(button_thread);
    std::thread::spawn(poll_thread);

    while KEEP_RUNNING.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_secs(1));
    }

    unsafe { LipcClose(handle) };
    log_line("daemon stop");
    println!("kmuxd shutting down.");
}
