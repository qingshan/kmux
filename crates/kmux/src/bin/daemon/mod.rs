//! Kindle runtime: LIPC, serialized commands, status storage, and workers.
pub(crate) mod dispatch;
pub(crate) mod lipc;
pub(crate) mod state;
pub(crate) mod workers;

pub(crate) type Op = serde_json::Value;
pub(crate) const SERVICE_NAME: &str = "dev.qingshan.kmuxd";
pub(crate) const CONFIG_PATH: &str = "/mnt/us/kmux/var/config.json";
pub(crate) const STATUS_DIR: &str = "/var/local/mesquite/kmux";
pub(crate) const STATUS_PATH: &str = "/var/local/mesquite/kmux/status.json";
pub(crate) const LOG_PATH: &str = "/mnt/us/kmux/var/tmux_log.txt";
