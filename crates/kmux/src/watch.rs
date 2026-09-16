//! Window/pane identity and in-flight capture-pane tracking.
//!
//! tmux can renumber windows on close (`renumber-windows`): exiting window
//! 1 leaves the old window 2 at index 1. Comparing only the index then
//! misses the switch, and a capture-pane that was already in flight can
//! still dump the dying pane after we blanked the model.

use std::collections::VecDeque;

/// `W sess idx name flags [WxH]` — index of the starred window of `session`.
///
/// `list-windows -a` marks the current window of EVERY session with `*`;
/// scanning for the first star without a session would pick another
/// session's window index, so callers must pass the session they display.
pub fn active_window_index(session: &str, lines: &[String]) -> Option<String> {
    lines.iter().find_map(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        if t.len() >= 4 && t[0] == "W" && t[1] == session && t.iter().any(|f| f.contains('*')) {
            Some(t[2].to_string())
        } else {
            None
        }
    })
}

/// `P sess winIdx paneId … active` — pane id of the active pane of `win`
/// in `session`.
pub fn select_active_pane(
    session: Option<&str>,
    win: Option<&str>,
    pane_lines: &[String],
) -> Option<String> {
    let session = session?;
    let win = win?;
    for line in pane_lines {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() >= 6 && t[0] == "P" && t[1] == session && t[2] == win && t[t.len() - 1] == "1" {
            return Some(t[3].to_string());
        }
    }
    None
}

/// True for names safe to pass to `switch-client -t` (same rule as kmux-proxy).
pub fn valid_session_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

/// `list-sessions -F 'S #{session_name} #{session_id} attached nwindows'`.
pub fn parse_session_line(line: &str) -> Option<(&str, bool)> {
    let t: Vec<&str> = line.split_whitespace().collect();
    if t.len() < 4 || t[0] != "S" || !valid_session_name(t[1]) {
        return None;
    }
    Some((t[1], t[3] == "1"))
}

/// Pane ids (`%8`, …) in a tagged `P sess win paneId …` block.
pub fn pane_ids(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            if t.len() >= 4 && t[0] == "P" {
                Some(t[3].to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Whether `%output` from `pane` should paint the model.
///
/// `new_window_pending`: after create/close we do not yet know the pane
/// the WAF should follow. Known panes (the ones that existed before) are
/// dropped so a still-running grok cannot paint over a new/remaining
/// window; an unknown pane id is latched as the new active pane.
pub fn pane_output_accepted(
    pane: &str,
    active_pane: Option<&str>,
    new_window_pending: bool,
    known_panes: &[String],
) -> PaneOutput {
    if let Some(id) = active_pane {
        return PaneOutput {
            accept: id == pane,
            latch: None,
        };
    }
    if new_window_pending {
        if known_panes.iter().any(|p| p == pane) {
            return PaneOutput {
                accept: false,
                latch: None,
            };
        }
        return PaneOutput {
            accept: true,
            latch: Some(pane.to_string()),
        };
    }
    PaneOutput {
        accept: true,
        latch: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneOutput {
    pub accept: bool,
    pub latch: Option<String>,
}

/// Kind of an in-flight `capture-pane`. Regular and copy-mode dumps share
/// the same `%begin/%end` channel, so they must be matched FIFO — a mode
/// dump must not be applied as the local scrollback (or vice versa).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    Regular,
    Mode,
    /// `show-buffer` after copy-mode yank (not a pane dump).
    Clip,
}

/// Outstanding `capture-pane` commands in send order.
///
/// Regular dumps skip all but the latest Regular still queued (a dying
/// window's dump must not replace a recapture). Mode dumps always apply:
/// each page-up in copy mode needs its overlay. After a host/window
/// switch, unsolicited dumps (empty queue + hold) are the dying pane.
#[derive(Debug, Default)]
pub struct CaptureGate {
    pending: VecDeque<CaptureKind>,
    hold: bool,
}

impl CaptureGate {
    pub const fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            hold: false,
        }
    }

    pub fn hold(&mut self) {
        self.hold = true;
        self.pending.clear();
    }

    pub fn holding(&self) -> bool {
        self.hold
    }

    pub fn sent(&mut self) {
        self.pending.push_back(CaptureKind::Regular);
    }

    pub fn sent_mode(&mut self) {
        self.pending.push_back(CaptureKind::Mode);
    }

    pub fn sent_clip(&mut self) {
        self.pending.push_back(CaptureKind::Clip);
    }

    pub fn peek(&self) -> Option<CaptureKind> {
        self.pending.front().copied()
    }

    pub fn has_mode_pending(&self) -> bool {
        self.pending.iter().any(|k| *k == CaptureKind::Mode)
    }

    /// Classify the next dump. `None` = skip (stale Regular, or unsolicited
    /// during hold).
    pub fn take(&mut self) -> Option<CaptureKind> {
        match self.pending.pop_front() {
            Some(CaptureKind::Mode) => {
                if self.pending.is_empty() {
                    self.hold = false;
                }
                Some(CaptureKind::Mode)
            }
            Some(CaptureKind::Clip) => {
                if self.pending.is_empty() {
                    self.hold = false;
                }
                Some(CaptureKind::Clip)
            }
            Some(CaptureKind::Regular) => {
                let more_regular = self.pending.iter().any(|k| *k == CaptureKind::Regular);
                if self.pending.is_empty() {
                    self.hold = false;
                }
                if more_regular {
                    None
                } else {
                    Some(CaptureKind::Regular)
                }
            }
            None => {
                if self.hold {
                    None
                } else {
                    Some(CaptureKind::Regular)
                }
            }
        }
    }

    /// True when this snapshot is a Regular dump that should replace the
    /// model (or no capture was tracked — apply, matching the old path).
    pub fn should_apply(&mut self) -> bool {
        matches!(self.take(), Some(CaptureKind::Regular))
    }
}

/// A `capture-pane` dump is ~pane-height lines (blanks included). A tmux
/// command error is one or two short lines with no SGR.
pub fn looks_like_tmux_error(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() || t.contains('\x1b') {
        return false;
    }
    let first = t.lines().next().unwrap_or("");
    first.starts_with("unknown option")
        || first.starts_with("unknown command")
        || first.starts_with("unknown flag")
        || first.starts_with("can't find")
        || first.starts_with("couldn't find")
        || first.starts_with("no current")
        || first.starts_with("usage:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_window_index_reads_star_for_session() {
        let lines = vec![
            "W main 1 grok - [80x24]".into(),
            "W main 2 fish * [80x24]".into(),
            "W dotfiles 1 vim * [80x24]".into(),
        ];
        assert_eq!(active_window_index("main", &lines).as_deref(), Some("2"));
        assert_eq!(
            active_window_index("dotfiles", &lines).as_deref(),
            Some("1")
        );
        assert_eq!(active_window_index("other", &lines), None);
    }

    #[test]
    fn select_active_pane_picks_flag_1_for_window() {
        let panes = vec![
            "P main 1 %5 0 grok [80x24] 1".into(),
            "P main 2 %8 0 fish [80x24] 1".into(),
            "P work 1 %9 0 vim [80x24] 1".into(),
        ];
        assert_eq!(
            select_active_pane(Some("main"), Some("1"), &panes).as_deref(),
            Some("%5")
        );
        assert_eq!(
            select_active_pane(Some("main"), Some("2"), &panes).as_deref(),
            Some("%8")
        );
        assert_eq!(
            select_active_pane(Some("work"), Some("1"), &panes).as_deref(),
            Some("%9")
        );
        assert_eq!(select_active_pane(Some("main"), Some("3"), &panes), None);
    }

    #[test]
    fn close_renumber_changes_pane_at_same_index() {
        // Window 1 (grok %5) exits; tmux renumbers window 2 → 1 (fish %8).
        let before = vec![
            "P main 1 %5 0 grok [80x24] 1".into(),
            "P main 2 %8 0 fish [80x24] 1".into(),
        ];
        let after = vec!["P main 1 %8 0 fish [80x24] 1".into()];
        let old = select_active_pane(Some("main"), Some("1"), &before);
        let new = select_active_pane(Some("main"), Some("1"), &after);
        assert_eq!(old.as_deref(), Some("%5"));
        assert_eq!(new.as_deref(), Some("%8"));
        assert_ne!(old, new);
    }

    #[test]
    fn pending_new_window_drops_known_panes() {
        let known = vec!["%5".into(), "%8".into()];
        let drop_old = pane_output_accepted("%5", None, true, &known);
        assert!(!drop_old.accept);
        let latch_new = pane_output_accepted("%12", None, true, &known);
        assert!(latch_new.accept);
        assert_eq!(latch_new.latch.as_deref(), Some("%12"));
    }

    #[test]
    fn valid_session_name_rejects_metacharacters() {
        assert!(valid_session_name("main"));
        assert!(valid_session_name("grok-2"));
        assert!(valid_session_name("a.b_c"));
        assert!(!valid_session_name(""));
        assert!(!valid_session_name("has space"));
        assert!(!valid_session_name("foo;rm"));
        assert!(!valid_session_name("a/b"));
    }

    #[test]
    fn parse_session_line_reads_name_and_attached() {
        assert_eq!(parse_session_line("S main $0 1 2"), Some(("main", true)));
        assert_eq!(parse_session_line("S grok $1 0 1"), Some(("grok", false)));
        assert_eq!(parse_session_line("S bad;name $0 1 1"), None);
        assert_eq!(parse_session_line("main: 1 windows"), None);
    }

    #[test]
    fn capture_gate_skips_stale_then_applies_latest() {
        let mut g = CaptureGate::default();
        g.sent(); // dying window, already in flight
        g.sent(); // recapture of the remaining window
        assert!(!g.should_apply(), "dying dump must not replace the blank");
        assert!(g.should_apply(), "remaining-window dump is the latest");
        assert!(g.should_apply(), "untracked snapshot still applies");
    }

    #[test]
    fn hold_drops_unsolicited_until_sent_capture() {
        let mut g = CaptureGate::default();
        g.hold();
        assert!(g.holding());
        assert!(!g.should_apply(), "dying dump after host switch");
        g.sent();
        assert!(g.should_apply(), "handshake capture");
        assert!(!g.holding());
        assert!(g.should_apply(), "live path after hold lifts");
    }

    #[test]
    fn mode_dumps_do_not_steal_regular_captures() {
        let mut g = CaptureGate::default();
        g.sent();
        g.sent_mode();
        g.sent();
        assert_eq!(g.take(), None, "stale regular of the dying pane");
        assert_eq!(g.take(), Some(CaptureKind::Mode));
        assert_eq!(g.take(), Some(CaptureKind::Regular));
    }

    #[test]
    fn hold_drops_in_flight_mode_dumps() {
        let mut g = CaptureGate::default();
        g.sent_mode();
        g.hold();
        assert_eq!(g.take(), None);
        g.sent();
        assert_eq!(g.take(), Some(CaptureKind::Regular));
    }

    #[test]
    fn each_mode_dump_applies() {
        let mut g = CaptureGate::default();
        g.sent_mode();
        g.sent_mode();
        assert_eq!(g.take(), Some(CaptureKind::Mode));
        assert_eq!(g.take(), Some(CaptureKind::Mode));
    }

    #[test]
    fn clip_dump_does_not_steal_regular() {
        let mut g = CaptureGate::default();
        g.sent_clip();
        g.sent();
        assert_eq!(g.peek(), Some(CaptureKind::Clip));
        assert_eq!(g.take(), Some(CaptureKind::Clip));
        assert_eq!(g.take(), Some(CaptureKind::Regular));
    }

    #[test]
    fn tmux_error_is_short_unescaped_message() {
        assert!(looks_like_tmux_error("unknown option: -M\n"));
        assert!(looks_like_tmux_error("can't find pane %9\n"));
        assert!(!looks_like_tmux_error("prompt$\n\n\n"));
        assert!(!looks_like_tmux_error("\x1b[7mcopy\x1b[0m\n"));
        assert!(!looks_like_tmux_error(""));
        let mut pane = String::from("$ \n");
        for _ in 0..23 {
            pane.push('\n');
        }
        assert!(!looks_like_tmux_error(&pane));
    }
}
