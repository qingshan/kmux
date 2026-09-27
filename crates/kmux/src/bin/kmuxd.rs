//! Kindle kmuxd process startup and shutdown.
mod daemon;

use daemon::{dispatch, lipc, state::*, workers, Op, CONFIG_PATH};
use kmuxd::{config, LIPC_ERROR_DUPLICATE_SERVICE_NAME};
use std::ffi::c_int;
use std::path::Path;
use std::sync::{atomic::Ordering, mpsc};
use std::thread;
use std::time::Duration;

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
    thread::spawn(move || dispatch::dispatch(rx));

    let service = match lipc::Service::open() {
        Ok(service) => service,
        Err(LIPC_ERROR_DUPLICATE_SERVICE_NAME) => return,
        Err(code) => {
            eprintln!("Failed to open LIPC (code {code})");
            std::process::exit(1);
        }
    };

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

    service.register_properties();

    std::thread::spawn(workers::button_thread);
    std::thread::spawn(workers::poll_thread);

    while KEEP_RUNNING.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_secs(1));
    }

    drop(service);
    log_line("daemon stop");
    println!("kmuxd shutting down.");
}
