//! Host-testable Oasis button decoding and authoritative local history paging.
use crate::status::Status;

pub fn key(event_type: u16, code: u16, value: i32) -> Option<&'static str> {
    if event_type != 1 || value != 1 {
        return None;
    }
    match code {
        104 => Some("ScrollUp"),
        109 => Some("ScrollDown"),
        _ => None,
    }
}

pub fn scroll(status: &mut Status, up: bool, rows: usize, revision_seed: u64) {
    status.scroll_offset = if up {
        status
            .scroll_offset
            .saturating_add(rows)
            .min(status.scrollback.len())
    } else {
        status.scroll_offset.saturating_sub(rows)
    };
    // A fresh marker lets the WAF distinguish a real page-down/Live action
    // from an older status-file poll. Epoch seeding survives daemon restarts.
    status.scroll_revision = status.scroll_revision.saturating_add(1).max(revision_seed);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_both_buttons_once_per_press() {
        assert_eq!(key(1, 104, 1), Some("ScrollUp"));
        assert_eq!(key(1, 109, 1), Some("ScrollDown"));
        for (kind, code, value) in [(0, 104, 1), (1, 104, 0), (1, 104, 2), (1, 116, 1)] {
            assert_eq!(key(kind, code, value), None);
        }
    }
    #[test]
    fn pages_both_directions_and_reaches_live() {
        let mut s = Status {
            scrollback: vec![String::new(); 30],
            ..Status::default()
        };
        scroll(&mut s, true, 24, 1000);
        assert_eq!((s.scroll_offset, s.scroll_revision), (24, 1000));
        scroll(&mut s, true, 24, 1000);
        assert_eq!(s.scroll_offset, 30);
        scroll(&mut s, false, 24, 1000);
        assert_eq!(s.scroll_offset, 6);
        scroll(&mut s, false, 24, 1000);
        assert_eq!((s.scroll_offset, s.scroll_revision), (0, 1003));
    }
}
