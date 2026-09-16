//! tmux `send-keys` names used by kmuxd for WAF key ops.

/// Map a WAF key token to a control-mode `send-keys` line.
///
/// `"K:…"` is `send-keys -K` (client key table). Super sends `"K:C-b Up"`
/// as one command so the prefix and the key cannot reorder across POSTs.
pub fn send_keys_command(key: &str) -> Option<String> {
    if let Some(rest) = key.strip_prefix("K:") {
        let mut parts = Vec::new();
        for tok in rest.split_whitespace() {
            parts.push(normalize_tmux_key(tok)?);
        }
        if parts.is_empty() {
            return None;
        }
        return Some(format!("send-keys -K {}", parts.join(" ")));
    }
    Some(format!("send-keys {}", normalize_tmux_key(key)?))
}

/// A control command to drive a pane that is already in copy/view mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyModeSend {
    pub command: String,
    /// True when the command leaves copy mode (`cancel`, etc.).
    pub leaves: bool,
    /// True when the command copies the selection (follow with `show-buffer`).
    pub yank: bool,
}

/// Map a WAF key onto `send-keys -X` (copy-mode commands) or a pane-targeted
/// `send-keys`. Plain `send-keys Up` from a -CC client does not move the
/// copy-mode cursor. `repeat` is a vi-style count (`10j` → `-N 10`).
pub fn copy_mode_send(key: &str, pane: &str) -> Option<CopyModeSend> {
    copy_mode_send_n(key, pane, 0)
}

pub fn copy_mode_send_n(key: &str, pane: &str, repeat: u32) -> Option<CopyModeSend> {
    if !pane.starts_with('%') {
        return None;
    }
    let (xcmd, leaves) = match key {
        "k" | "Up" => ("cursor-up", false),
        "j" | "Down" => ("cursor-down", false),
        "h" | "Left" => ("cursor-left", false),
        "l" | "Right" => ("cursor-right", false),
        "PageUp" | "ScrollUp" | "C-b" => ("page-up", false),
        "PageDown" | "ScrollDown" | "C-f" => ("page-down", false),
        "C-u" => ("halfpage-up", false),
        "C-d" => ("halfpage-down", false),
        "0" | "^" | "Home" => ("start-of-line", false),
        "$" | "End" => ("end-of-line", false),
        "g" => ("history-top", false),
        "G" => ("history-bottom", false),
        "H" => ("top-line", false),
        "M" => ("middle-line", false),
        "L" => ("bottom-line", false),
        "w" => ("next-word", false),
        "b" => ("previous-word", false),
        "e" => ("next-word-end", false),
        "W" => ("next-space", false),
        "B" => ("previous-space", false),
        "C-Up" => ("scroll-up", false),
        "C-Down" => ("scroll-down", false),
        "C-Left" => ("previous-word", false),
        "C-Right" => ("next-word", false),
        " " | "Space" | "v" => ("begin-selection", false),
        "V" => ("select-line", false),
        "C-v" => ("rectangle-toggle", false),
        "Enter" | "C-j" | "y" => {
            return Some(CopyModeSend {
                command: format!("send-keys -X -t {pane} copy-selection-and-cancel"),
                leaves: true,
                yank: true,
            });
        }
        "q" | "C-c" | "Esc" | "Escape" | "ScrollBottom" => {
            return Some(CopyModeSend {
                command: format!("copy-mode -q -t {pane}"),
                leaves: true,
                yank: false,
            });
        }
        other => {
            let sk = send_keys_command(other)?;
            let command = if let Some(rest) = sk.strip_prefix("send-keys ") {
                format!("send-keys -t {pane} {rest}")
            } else {
                sk
            };
            return Some(CopyModeSend {
                command,
                leaves: false,
                yank: false,
            });
        }
    };
    let nflag = if repeat >= 2 {
        format!("-N {repeat} ")
    } else {
        String::new()
    };
    Some(CopyModeSend {
        command: format!("send-keys -X {nflag}-t {pane} {xcmd}"),
        leaves,
        yank: false,
    })
}

/// `C-a`, `M-x`, `C-M-z`, `C-Up`, or a named special after C-/M-/S- prefixes.
fn normalize_tmux_key(key: &str) -> Option<String> {
    let mut prefixes = String::new();
    let mut s = key;
    let mut n = 0;
    while n < 3 {
        if let Some(rest) = s.strip_prefix("C-") {
            prefixes.push_str("C-");
            s = rest;
            n += 1;
            continue;
        }
        if let Some(rest) = s.strip_prefix("M-") {
            prefixes.push_str("M-");
            s = rest;
            n += 1;
            continue;
        }
        if let Some(rest) = s.strip_prefix("S-") {
            prefixes.push_str("S-");
            s = rest;
            n += 1;
            continue;
        }
        break;
    }
    let base = match s {
        "Backspace" => "BSpace",
        "Delete" => "DC",
        "PageUp" => "PPage",
        "PageDown" => "NPage",
        "Esc" => "Escape",
        other => other,
    };
    if !tmux_base_ok(base) {
        return None;
    }
    Some(format!("{prefixes}{base}"))
}

/// Single-quote `s` for a tmux command argument (`run-shell -c`, the
/// `run-shell` script). `'` becomes `'\''`.
pub fn tmux_quote(s: &str) -> String {
    shell_single_quote(s)
}

/// True for an absolute path with no NUL/CR/LF — safe to pass as `run-shell -c`.
pub fn path_is_abs_safe(path: &str) -> bool {
    path.starts_with('/')
        && !path.is_empty()
        && !path.bytes().any(|b| b == 0 || b == b'\n' || b == b'\r')
}

/// A shell word: unquoted when `s` is `[A-Za-z0-9._+/-]+`, otherwise
/// single-quoted (`'` → `'\''`).
pub fn shell_quote(s: &str) -> String {
    if is_safe_shell_word(s) {
        s.to_string()
    } else {
        shell_single_quote(s)
    }
}

fn is_safe_shell_word(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '+' | '-'))
}

fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

fn tmux_base_ok(s: &str) -> bool {
    matches!(
        s,
        "Enter"
            | "Escape"
            | "Tab"
            | "Space"
            | "Up"
            | "Down"
            | "Left"
            | "Right"
            | "Home"
            | "End"
            | "BSpace"
            | "NPage"
            | "PPage"
            | "DC"
            | "|"
            | "/"
            | "\\"
            | "~"
            | "-"
            | "_"
    ) || (s.len() == 1 && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_specials() {
        assert_eq!(
            send_keys_command("Enter").as_deref(),
            Some("send-keys Enter")
        );
        assert_eq!(
            send_keys_command("Backspace").as_deref(),
            Some("send-keys BSpace")
        );
        assert_eq!(send_keys_command("Delete").as_deref(), Some("send-keys DC"));
        assert_eq!(
            send_keys_command("Esc").as_deref(),
            Some("send-keys Escape")
        );
    }

    #[test]
    fn page_keys_are_not_swapped() {
        assert_eq!(
            send_keys_command("PageUp").as_deref(),
            Some("send-keys PPage")
        );
        assert_eq!(
            send_keys_command("PageDown").as_deref(),
            Some("send-keys NPage")
        );
        assert_eq!(
            send_keys_command("C-PageUp").as_deref(),
            Some("send-keys C-PPage")
        );
    }

    #[test]
    fn modifiers() {
        assert_eq!(send_keys_command("C-c").as_deref(), Some("send-keys C-c"));
        assert_eq!(send_keys_command("M-x").as_deref(), Some("send-keys M-x"));
        assert_eq!(
            send_keys_command("C-M-z").as_deref(),
            Some("send-keys C-M-z")
        );
        assert_eq!(send_keys_command("C-Up").as_deref(), Some("send-keys C-Up"));
    }

    #[test]
    fn copy_mode_arrows_use_dash_x() {
        let s = copy_mode_send("Up", "%8").unwrap();
        assert_eq!(s.command, "send-keys -X -t %8 cursor-up");
        assert!(!s.leaves);
        assert_eq!(
            copy_mode_send("j", "%8").unwrap().command,
            "send-keys -X -t %8 cursor-down"
        );
        assert_eq!(
            copy_mode_send("k", "%8").unwrap().command,
            "send-keys -X -t %8 cursor-up"
        );
        assert_eq!(
            copy_mode_send("h", "%8").unwrap().command,
            "send-keys -X -t %8 cursor-left"
        );
        assert_eq!(
            copy_mode_send("l", "%8").unwrap().command,
            "send-keys -X -t %8 cursor-right"
        );
        assert_eq!(
            copy_mode_send_n("j", "%8", 10).unwrap().command,
            "send-keys -X -N 10 -t %8 cursor-down"
        );
        assert_eq!(
            copy_mode_send("G", "%8").unwrap().command,
            "send-keys -X -t %8 history-bottom"
        );
        let cancel = copy_mode_send("Escape", "%8").unwrap();
        assert_eq!(cancel.command, "copy-mode -q -t %8");
        assert!(cancel.leaves);
        assert_eq!(
            copy_mode_send(" ", "%8").unwrap().command,
            "send-keys -X -t %8 begin-selection"
        );
        assert_eq!(
            copy_mode_send("V", "%8").unwrap().command,
            "send-keys -X -t %8 select-line"
        );
        let yank = copy_mode_send("Enter", "%8").unwrap();
        assert_eq!(yank.command, "send-keys -X -t %8 copy-selection-and-cancel");
        assert!(yank.leaves);
        assert!(yank.yank);
        assert!(copy_mode_send("y", "%8").unwrap().yank);
        assert!(!copy_mode_send("Escape", "%8").unwrap().yank);
        assert_eq!(
            copy_mode_send("a", "%8").unwrap().command,
            "send-keys -t %8 a"
        );
        assert!(copy_mode_send("Up", "not-a-pane").is_none());
    }

    #[test]
    fn super_prefix_chord() {
        assert_eq!(
            send_keys_command("K:C-b Up").as_deref(),
            Some("send-keys -K C-b Up")
        );
        assert_eq!(
            send_keys_command("K:C-b C-c").as_deref(),
            Some("send-keys -K C-b C-c")
        );
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(send_keys_command("not a key"), None);
        assert_eq!(send_keys_command("K:"), None);
        assert_eq!(send_keys_command("C-"), None);
    }

    #[test]
    fn tmux_quote_single_quotes() {
        assert_eq!(tmux_quote("/tmp"), "'/tmp'");
        assert_eq!(tmux_quote("/tmp/foo bar"), "'/tmp/foo bar'");
        assert_eq!(tmux_quote("/tmp/it's"), "'/tmp/it'\\''s'");
    }

    #[test]
    fn path_abs_safe() {
        assert!(path_is_abs_safe("/home/qs"));
        assert!(path_is_abs_safe("/tmp/foo bar"));
        assert!(!path_is_abs_safe(""));
        assert!(!path_is_abs_safe("relative"));
        assert!(!path_is_abs_safe("/tmp/\nfoo"));
        assert!(!path_is_abs_safe("/tmp/\0"));
    }

    #[test]
    fn shell_quote_safe_and_special() {
        assert_eq!(shell_quote("README.md"), "README.md");
        assert_eq!(shell_quote("src/main.rs"), "src/main.rs");
        assert_eq!(shell_quote("foo-bar_1+2.txt"), "foo-bar_1+2.txt");
        assert_eq!(shell_quote("file with space.txt"), "'file with space.txt'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
        assert_eq!(shell_quote(""), "''");
    }
}
