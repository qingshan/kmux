//! Native kmux-proxy startup and connection dispatch.
use std::net::TcpListener;
use std::sync::Arc;

mod backend;
mod catalog;
mod config;
mod copy;
mod diagnose;
mod events;
mod hosts;
mod http;
mod pty;
mod unified;

fn main() {
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/etc/kmux-proxy.json".to_string());
    let mut cfg = config::load(&config_path).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        std::process::exit(1);
    });
    if cfg.bind.is_empty() {
        cfg.bind = config::tailnet_ip();
    }
    let listener = TcpListener::bind((cfg.bind.as_str(), cfg.port)).unwrap_or_else(|e| {
        eprintln!("error: bind {}:{}: {e}", cfg.bind, cfg.port);
        std::process::exit(1);
    });
    println!("kmux-proxy listening on {}:{}", cfg.bind, cfg.port);
    let cfg = Arc::new(cfg);
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let cfg = Arc::clone(&cfg);
                std::thread::spawn(move || http::handle(s, &cfg));
            }
            Err(_) => continue,
        }
    }
}
