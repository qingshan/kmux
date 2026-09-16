//! Pure logic for the kmuxd terminal daemon (kmux Kindle package).
//!
//! kmuxd holds a streaming connection to the kmux-proxy service on the
//! server (via the tailscale SOCKS5 proxy), parses the tmux control-mode
//! protocol (`control.rs`) and the VT escape sequences (`screen.rs`, via the
//! `vte` crate) into a screen model, and exposes it to the WAF through
//! status.json. The LIPC FFI helpers are vendored below like the other
//! dev.qingshan daemons.

use std::ffi::{c_char, c_int, c_void};
use std::time::{SystemTime, UNIX_EPOCH};

pub mod agents;
pub mod api;
pub mod buttons;
pub mod client;
pub mod config;
pub mod control;
pub mod screen;
pub mod status;
pub mod tmux_keys;
pub mod watch;

/// The WAF keeps a compact, session-scoped list of clipboard and compose
/// entries in status.json.  Keeping this logic here makes its limits and
/// Unicode handling host-testable without linking the Kindle LIPC shell.
pub const CLIPBOARD_HISTORY_LIMIT: usize = 12;
pub const CLIPBOARD_TEXT_LIMIT: usize = 64 * 1024;

/// Apply the daemon's clipboard normalization without splitting a UTF-8
/// character at the byte limit.
pub fn normalize_clipboard_text(text: &str) -> Option<String> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() {
        return None;
    }
    if text.len() <= CLIPBOARD_TEXT_LIMIT {
        return Some(text.to_string());
    }
    let mut end = CLIPBOARD_TEXT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(text[..end].to_string())
}

/// Wrap `text` in CSI 200~/201~ so the receiving program treats it as a paste.
///
/// Fish (and autopair plugins) bind `'`/`[` as typed keys; without this wrap,
/// compose/clipboard inserts gain extra `'` and `]`. Empty input is unchanged.
pub fn bracketed_paste_text(text: &str) -> String {
    const START: &str = "\x1b[200~";
    const END: &str = "\x1b[201~";
    if text.is_empty() || (text.starts_with(START) && text.ends_with(END)) {
        text.to_string()
    } else {
        format!("{START}{text}{END}")
    }
}

/// Promote `text` to the front of an LRU clipboard history. Duplicate text
/// appears once and the oldest entries fall off after the configured limit.
pub fn remember_clipboard(history: &mut Vec<String>, text: String) {
    history.retain(|entry| entry != &text);
    history.insert(0, text);
    history.truncate(CLIPBOARD_HISTORY_LIMIT);
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;

    #[test]
    fn clipboard_normalization_drops_one_trailing_newline() {
        assert_eq!(normalize_clipboard_text("hello\n"), Some("hello".into()));
        assert_eq!(normalize_clipboard_text("\n"), None);
    }

    #[test]
    fn clipboard_normalization_keeps_utf8_valid_at_limit() {
        let text = format!("{}€", "x".repeat(CLIPBOARD_TEXT_LIMIT - 1));
        let normalized = normalize_clipboard_text(&text).unwrap();
        assert_eq!(normalized.len(), CLIPBOARD_TEXT_LIMIT - 1);
        assert!(normalized.is_char_boundary(normalized.len()));
    }

    #[test]
    fn clipboard_history_promotes_duplicates_and_evicts_lru() {
        let mut history: Vec<String> = (0..CLIPBOARD_HISTORY_LIMIT)
            .map(|n| format!("item-{n}"))
            .collect();
        remember_clipboard(&mut history, "item-5".into());
        assert_eq!(history[0], "item-5");
        assert_eq!(history.len(), CLIPBOARD_HISTORY_LIMIT);
        remember_clipboard(&mut history, "new".into());
        assert_eq!(history[0], "new");
        assert_eq!(history.len(), CLIPBOARD_HISTORY_LIMIT);
        assert!(!history.iter().any(|item| item == "item-11"));
    }

    #[test]
    fn bracketed_paste_wraps_once() {
        assert_eq!(bracketed_paste_text(""), "");
        assert_eq!(
            bracketed_paste_text("printf '\\e[8;24;80t'"),
            "\x1b[200~printf '\\e[8;24;80t'\x1b[201~"
        );
        let already = bracketed_paste_text("x");
        assert_eq!(bracketed_paste_text(&already), already);
    }
}

/// Current time as unix epoch seconds.
pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// --- vendored from tsctl's lib.rs (see the other dev.qingshan daemons) ----

// LIPCcode values (from lipc.h)
pub const LIPC_OK: c_int = 0;
pub const LIPC_ERROR_INTERNAL: c_int = 2;
pub const LIPC_ERROR_BUFFER_TOO_SMALL: c_int = 10;
pub const LIPC_ERROR_INVALID_ARG: c_int = 12;
pub const LIPC_ERROR_DUPLICATE_SERVICE_NAME: c_int = 17;

/// Copy `s` into the caller's buffer (getter path).
///
/// `value` is the output buffer, `data` points to the capacity. Returns
/// [`LIPC_ERROR_BUFFER_TOO_SMALL`] and stores the needed size in `capacity` when
/// the buffer is too small.
///
/// # Safety
/// `value` must point to a writable buffer of at least `capacity` bytes and
/// `data` must point to a `size_t` holding that capacity.
pub unsafe fn write_string_prop(value: *mut c_void, data: *mut c_void, s: &str) -> c_int {
    let buf = value as *mut c_char;
    let capacity = &mut *(data as *mut usize);
    let needed = s.len() + 1;
    if needed > *capacity {
        *capacity = needed;
        return LIPC_ERROR_BUFFER_TOO_SMALL;
    }
    // SAFETY: caller-provided buffer of at least `needed` bytes, s is valid for
    // `needed` bytes. Copy s.len() bytes, then write the NUL explicitly.
    unsafe {
        std::ptr::copy_nonoverlapping(s.as_ptr(), buf as *mut u8, s.len());
        *buf.add(s.len()) = 0;
    };
    LIPC_OK
}

/// Read the caller-provided input string (setter path). Returns `None` on a
/// null buffer.
///
/// # Safety
/// `value` must point to a NUL-terminated C string (or be null).
pub unsafe fn read_string_prop(value: *mut c_void) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: caller-provided NUL-terminated buffer.
    Some(
        unsafe { std::ffi::CStr::from_ptr(value as *const c_char) }
            .to_string_lossy()
            .into_owned(),
    )
}
