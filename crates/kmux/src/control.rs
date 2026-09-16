//! The tmux control-mode protocol parser: line-based `%begin/%end/%output`
//! blocks and notifications, with DCS envelope stripping and octal
//! unescaping of `%output` payloads. Stateful across chunk boundaries
//! (streaming input). Host-testable.

/// Cap on a file-picker listing so a huge directory cannot balloon status.json.
pub const FILE_LIST_LIMIT: usize = 500;

/// One `ls -1ap` row after the `F ` tag (`dir` when the name had a trailing `/`).
#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    pub name: String,
    pub dir: bool,
}

/// `run-shell` listing for the file picker: `CWD <abs>` then `F <name>` rows.
#[derive(Debug, Clone, PartialEq)]
pub struct FileList {
    pub cwd: String,
    pub entries: Vec<FileEntry>,
    pub truncated: bool,
}

/// tmux `selection_start_*` / `selection_end_*` while `screen.sel` is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopySel {
    pub rect: bool,
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

/// How a command-output block should be handled: a windows list, a panes
/// list, or plain screen content (capture-pane). Format-based routing so
/// timing can never misroute a block.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockKind {
    Windows(Vec<String>),
    Panes(Vec<String>),
    /// `list-sessions -F 'S #{session_name} …'` — tagged so default
    /// `name: N windows` output cannot be mistaken for a window list.
    Sessions(Vec<String>),
    /// A `display -p` cursor query reply: "KMUXCURSOR <x> <y>".
    Cursor {
        x: usize,
        y: usize,
    },
    /// `display -p 'KMUXCWD #{pane_current_path}'` — the pane directory
    /// before a file listing (`run-shell -c` does not expand formats).
    PaneCwd(String),
    /// `display -p 'KMUXSESS #{session_name}'` — this client's session,
    /// so list-windows can bind the starred window before `%session-changed`.
    SessionName(String),
    /// `display -p 'KMUXMODE #{pane_id} #{pane_in_mode}'`.
    PaneInMode {
        pane: String,
        on: bool,
    },
    /// `display -p 'KMUXCOPY #{copy_cursor_x} #{copy_cursor_y} #{scroll_position} #{pane_height} …'`.
    CopyView {
        x: usize,
        y: usize,
        oy: usize,
        h: usize,
        hist: usize,
        /// Backing-grid selection (`selx/sely`); `y` is absolute, not dump-relative.
        sel: Option<CopySel>,
    },
    /// Tagged reply we do not apply as a pane dump (bad payload).
    Discard,
    Files(FileList),
    Screen,
}

/// Empty `%begin/%end` from `send-keys` / `copy-mode` (not a pane dump).
pub fn is_empty_command_ack(text: &str) -> bool {
    if text.lines().any(|l| !l.trim().is_empty()) {
        return false;
    }
    text.bytes().filter(|&b| b == b'\n').count() <= 2
}

/// Route a command-output block by a tag prefix so captures cannot look
/// like lists: `S ` sessions, `W ` windows, `P ` panes.
pub fn route_block(text: &str) -> BlockKind {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        // `send-keys` / `copy-mode` return an empty %begin/%end. That must
        // not consume an in-flight capture-pane (copy-mode overlay would
        // freeze). A real blank pane dump is ~rows of newlines.
        if is_empty_command_ack(text) {
            return BlockKind::Discard;
        }
        return BlockKind::Screen;
    }
    // "KMUXCURSOR <x> <y>" — the cursor position query reply.
    if let Some(first) = lines.first() {
        if let Some(rest) = first.strip_prefix("KMUXCURSOR ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() == 2 {
                if let (Ok(x), Ok(y)) = (parts[0].parse::<usize>(), parts[1].parse::<usize>()) {
                    return BlockKind::Cursor { x, y };
                }
            }
        }
        if let Some(rest) = first.strip_prefix("KMUXCOPY ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 4 {
                if let (Ok(x), Ok(y), Ok(oy), Ok(h)) = (
                    parts[0].parse::<usize>(),
                    parts[1].parse::<usize>(),
                    parts[2].parse::<usize>(),
                    parts[3].parse::<usize>(),
                ) {
                    let hist = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let sel = if parts.len() >= 11 {
                        match (
                            parts[7].parse::<usize>(),
                            parts[8].parse::<usize>(),
                            parts[9].parse::<usize>(),
                            parts[10].parse::<usize>(),
                        ) {
                            (Ok(x0), Ok(y0), Ok(x1), Ok(y1)) => Some(CopySel {
                                rect: parts[6] == "1",
                                x0,
                                y0,
                                x1,
                                y1,
                            }),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    return BlockKind::CopyView {
                        x,
                        y,
                        oy,
                        h,
                        hist,
                        sel,
                    };
                }
            }
            return BlockKind::Discard;
        }
        if lines.len() == 1 {
            if let Some(rest) = first.strip_prefix("KMUXCWD ") {
                if !rest.is_empty() {
                    return BlockKind::PaneCwd(rest.to_string());
                }
            }
            if let Some(rest) = first.strip_prefix("KMUXSESS ") {
                if !rest.is_empty() {
                    return BlockKind::SessionName(rest.to_string());
                }
            }
            if let Some(rest) = first.strip_prefix("KMUXMODE ") {
                let t: Vec<&str> = rest.split_whitespace().collect();
                if t.len() == 2 && t[0].starts_with('%') && (t[1] == "0" || t[1] == "1") {
                    return BlockKind::PaneInMode {
                        pane: t[0].to_string(),
                        on: t[1] == "1",
                    };
                }
                if t.len() == 1 && (t[0] == "0" || t[0] == "1") {
                    return BlockKind::PaneInMode {
                        pane: String::new(),
                        on: t[0] == "1",
                    };
                }
                return BlockKind::Discard;
            }
        }
    }
    // File picker: "CWD /abs" then zero or more "F name" rows (name may
    // contain spaces; a trailing / marks a directory). Any other first
    // line, or a later line without the F tag, is a capture.
    if let Some(files) = parse_file_list(&lines) {
        return BlockKind::Files(files);
    }
    // Tagged session list: "S name $id attached nwindows". Checked before
    // windows so a default `list-sessions` line ("main: 1 windows") is
    // never this kind — that form has no `S ` prefix.
    let all_sessions = lines.iter().all(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        t.len() >= 4 && t[0] == "S" && (t[3] == "0" || t[3] == "1")
    });
    if all_sessions {
        return BlockKind::Sessions(lines.iter().map(|l| l.to_string()).collect());
    }
    let all_windows = lines.iter().all(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        t.len() >= 4 && t[0] == "W" && t[2].chars().all(|c| c.is_ascii_digit())
    });
    if all_windows {
        return BlockKind::Windows(lines.iter().map(|l| l.to_string()).collect());
    }
    let all_panes = lines.iter().all(|l| {
        let t: Vec<&str> = l.split_whitespace().collect();
        t.len() >= 6 && t[0] == "P" && t[3].starts_with('%')
    });
    if all_panes {
        return BlockKind::Panes(lines.iter().map(|l| l.to_string()).collect());
    }
    BlockKind::Screen
}

/// `CWD <abs>` plus `F <name>` rows. `.` / `./` are dropped; a trailing
/// `/` on the name is a directory. Later lines that are not `F `-tagged
/// mean this is not a listing.
fn parse_file_list(lines: &[&str]) -> Option<FileList> {
    let cwd = lines.first()?.strip_prefix("CWD ")?;
    if cwd.is_empty() {
        return None;
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    for l in &lines[1..] {
        let rest = l.strip_prefix("F ")?;
        let (name, dir) = if let Some(n) = rest.strip_suffix('/') {
            (n, true)
        } else {
            (rest, false)
        };
        if name.is_empty() || name == "." {
            continue;
        }
        if entries.len() < FILE_LIST_LIMIT {
            entries.push(FileEntry {
                name: name.to_string(),
                dir,
            });
        } else {
            truncated = true;
        }
    }
    Some(FileList {
        cwd: cwd.to_string(),
        entries,
        truncated,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControlEvent {
    /// A command-output block completed (the collected lines, joined).
    Block(String),
    /// A pane produced output (unescaped raw bytes with escapes).
    Output { pane: String, data: Vec<u8> },
    /// tmux client exited.
    Exit(String),
    /// Session changed (name).
    SessionChanged(String),
    /// `%pane-mode-changed %pane` — copy/view/etc. entered or left.
    PaneModeChanged(String),
    /// OSC 52 clipboard payload (`ESC ] 52 ; Pc ; <base64> BEL/ST`).
    Clipboard(String),
    /// Other notification (raw line, e.g. %layout-change) — ignored by v1.
    Other(String),
}

/// Unescape a control-mode value: tmux escapes non-printables AND backslash
/// as `\` + three octal digits (per tmux(1): "escapes non-printable
/// characters and backslash as octal \xxx").
pub fn unescape(value: &str) -> Vec<u8> {
    unescape_bytes(value.as_bytes())
}

/// Byte-oriented unescape so a `%output` payload that is not valid UTF-8
/// (a `─` split across two notifications) is passed through as raw bytes
/// instead of being replaced with U+FFFD.
pub fn unescape_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let d2 = bytes[i + 1];
            let d1 = bytes[i + 2];
            let d0 = bytes[i + 3];
            if (b'0'..=b'7').contains(&d2)
                && (b'0'..=b'7').contains(&d1)
                && (b'0'..=b'7').contains(&d0)
            {
                out.push((d2 - b'0') * 64 + (d1 - b'0') * 8 + (d0 - b'0'));
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn trim_cr(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(&[b'\r']).unwrap_or(bytes)
}

/// Strip a DCS envelope if present (ESC P 1 0 0 0 p ... ESC \).
fn strip_dcs(mut bytes: &[u8]) -> &[u8] {
    const PRE: &[u8] = b"\x1bP1000p";
    const SUF: &[u8] = b"\x1b\\";
    if let Some(rest) = bytes.strip_prefix(PRE) {
        bytes = rest;
    }
    if let Some(rest) = bytes.strip_suffix(SUF) {
        bytes = rest;
    }
    bytes
}

/// Stateful control-protocol parser: feed bytes, collect completed events.
/// Block state persists across `feed` calls (a `%begin...%end` block may
/// span arbitrary chunk boundaries). Incomplete UTF-8 at a chunk boundary
/// stays in `buf` until the rest of the line arrives. `%output` payloads
/// are unescaped as bytes so a `─` split across two notifications is not
/// turned into U+FFFD cells by `from_utf8_lossy`.
#[derive(Default)]
pub struct ControlParser {
    buf: Vec<u8>,
    in_block: bool,
    block: String,
}

impl ControlParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, chunk: &[u8], events: &mut Vec<ControlEvent>) {
        self.buf.extend_from_slice(chunk);
        for text in extract_osc52(&mut self.buf) {
            events.push(ControlEvent::Clipboard(text));
        }
        let mut start = 0;
        while let Some(rel) = self.buf[start..].iter().position(|&b| b == b'\n') {
            let nl = start + rel;
            let line_bytes = strip_dcs(trim_cr(&self.buf[start..nl]));
            start = nl + 1;

            // Empty lines are meaningful INSIDE a block (a terminal screen
            // snapshot has blank rows) — only skip them between events.
            if line_bytes.is_empty() && !self.in_block {
                continue;
            }

            if line_bytes.starts_with(b"%begin") {
                self.in_block = true;
                self.block.clear();
                continue;
            }
            if self.in_block {
                if line_bytes.starts_with(b"%end") || line_bytes.starts_with(b"%error") {
                    self.in_block = false;
                    events.push(ControlEvent::Block(std::mem::take(&mut self.block)));
                    continue;
                }
                self.block.push_str(&String::from_utf8_lossy(line_bytes));
                self.block.push('\n');
                continue;
            }
            // Notifications (outside blocks). `%output` stays on bytes: tmux
            // may split a UTF-8 scalar across two notifications, and each
            // notification is already a complete line.
            if let Some(rest) = line_bytes.strip_prefix(b"%output ") {
                if let Some(space) = rest.iter().position(|&b| b == b' ') {
                    let pane = String::from_utf8_lossy(&rest[..space]).into_owned();
                    events.push(ControlEvent::Output {
                        pane,
                        data: unescape_bytes(&rest[space + 1..]),
                    });
                }
                continue;
            }
            let line = String::from_utf8_lossy(line_bytes).into_owned();
            if let Some(reason) = line.strip_prefix("%exit") {
                events.push(ControlEvent::Exit(reason.trim_start().to_string()));
            } else if let Some(rest) = line.strip_prefix("%session-changed ") {
                if let Some(name) = rest.rsplit(' ').next() {
                    events.push(ControlEvent::SessionChanged(name.to_string()));
                }
            } else if let Some(rest) = line.strip_prefix("%pane-mode-changed ") {
                let pane = rest.split_whitespace().next().unwrap_or("").to_string();
                if !pane.is_empty() {
                    events.push(ControlEvent::PaneModeChanged(pane));
                }
            } else if line.starts_with('%') {
                events.push(ControlEvent::Other(line));
            }
            // non-% lines outside blocks are ignored (should not happen).
        }
        self.buf.drain(..start);
    }
}

/// Pull complete OSC 52 sequences out of `buf`, leaving any trailing
/// incomplete sequence and all other bytes in place.
pub fn extract_osc52(buf: &mut Vec<u8>) -> Vec<String> {
    const PRE: &[u8] = b"\x1b]52;";
    let mut out = Vec::new();
    loop {
        let Some(start) = find_bytes(buf, PRE) else {
            return out;
        };
        let after_pre = start + PRE.len();
        let Some(semi) = buf[after_pre..].iter().position(|&b| b == b';') else {
            return out;
        };
        let b64_at = after_pre + semi + 1;
        let Some((term_at, term_len)) = osc_term(&buf[b64_at..]) else {
            return out;
        };
        let b64 = buf[b64_at..b64_at + term_at].to_vec();
        let end = b64_at + term_at + term_len;
        buf.drain(start..end);
        if let Some(text) = decode_osc52_payload(&b64) {
            out.push(text);
        }
    }
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn osc_term(rest: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == 0x07 {
            return Some((i, 1));
        }
        if rest[i] == 0x1b && rest.get(i + 1) == Some(&b'\\') {
            return Some((i, 2));
        }
        i += 1;
    }
    None
}

fn decode_osc52_payload(b64: &[u8]) -> Option<String> {
    let bytes: Vec<u8> = b64
        .iter()
        .copied()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    if bytes.is_empty() || bytes == b"?" {
        return None;
    }
    let raw = base64_decode(&bytes)?;
    Some(String::from_utf8_lossy(&raw).into_owned())
}

fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            b'=' => Some(0x80),
            _ => None,
        }
    }
    let mut padded = input.to_vec();
    while padded.len() % 4 != 0 {
        padded.push(b'=');
    }
    let mut out = Vec::with_capacity(padded.len() / 4 * 3);
    for chunk in padded.chunks(4) {
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        let c = val(chunk[2])?;
        let d = val(chunk[3])?;
        if a >= 64 || b >= 64 {
            return None;
        }
        out.push((a << 2) | (b >> 4));
        if c < 64 {
            out.push((b << 4) | (c >> 2));
            if d < 64 {
                out.push((c << 6) | d);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_cursor_query() {
        assert_eq!(
            route_block("KMUXCURSOR 2 23\n"),
            BlockKind::Cursor { x: 2, y: 23 }
        );
        assert_eq!(
            route_block("KMUXCURSOR 0 0\n"),
            BlockKind::Cursor { x: 0, y: 0 }
        );
        // not a cursor query -> screen
        assert_eq!(route_block("2 23\n"), BlockKind::Screen);
    }

    #[test]
    fn routes_pane_cwd() {
        assert_eq!(
            route_block("KMUXCWD /home/qs/proj\n"),
            BlockKind::PaneCwd("/home/qs/proj".into())
        );
        assert_eq!(
            route_block("KMUXCWD /tmp/foo bar\n"),
            BlockKind::PaneCwd("/tmp/foo bar".into())
        );
        assert_eq!(route_block("KMUXCWD \n"), BlockKind::Screen);
        assert_eq!(route_block("KMUXCWD /tmp\nF extra\n"), BlockKind::Screen);
    }

    #[test]
    fn routes_session_name() {
        assert_eq!(
            route_block("KMUXSESS main\n"),
            BlockKind::SessionName("main".into())
        );
        assert_eq!(
            route_block("KMUXSESS grok-2\n"),
            BlockKind::SessionName("grok-2".into())
        );
        assert_eq!(route_block("KMUXSESS \n"), BlockKind::Screen);
        assert_eq!(route_block("KMUXSESS main\nW extra\n"), BlockKind::Screen);
    }

    #[test]
    fn routes_pane_in_mode() {
        assert_eq!(
            route_block("KMUXMODE %16 1\n"),
            BlockKind::PaneInMode {
                pane: "%16".into(),
                on: true
            }
        );
        assert_eq!(
            route_block("KMUXMODE %16 0\n"),
            BlockKind::PaneInMode {
                pane: "%16".into(),
                on: false
            }
        );
        assert_eq!(
            route_block("KMUXMODE 1\n"),
            BlockKind::PaneInMode {
                pane: String::new(),
                on: true
            }
        );
        assert_eq!(route_block("KMUXMODE yes\n"), BlockKind::Discard);
    }

    #[test]
    fn routes_copy_view() {
        assert_eq!(
            route_block("KMUXCOPY 3 11 24 50\n"),
            BlockKind::CopyView {
                x: 3,
                y: 11,
                oy: 24,
                h: 50,
                hist: 0,
                sel: None,
            }
        );
        assert_eq!(
            route_block("KMUXCOPY 3 11 0 24 100 1 0 2 90 8 92\n"),
            BlockKind::CopyView {
                x: 3,
                y: 11,
                oy: 0,
                h: 24,
                hist: 100,
                sel: Some(CopySel {
                    rect: false,
                    x0: 2,
                    y0: 90,
                    x1: 8,
                    y1: 92,
                }),
            }
        );
        assert_eq!(route_block("KMUXCOPY 3 11\n"), BlockKind::Discard);
        assert_eq!(route_block("KMUXCOPY \n"), BlockKind::Discard);
    }

    #[test]
    fn empty_command_ack_is_discard() {
        assert!(is_empty_command_ack(""));
        assert!(is_empty_command_ack("\n"));
        assert!(is_empty_command_ack("\n\n"));
        assert!(!is_empty_command_ack("hello\n"));
        assert!(!is_empty_command_ack(&"\n".repeat(24)));
    }

    #[test]
    fn parses_pane_mode_changed() {
        let mut p = ControlParser::new();
        let mut events = Vec::new();
        p.feed(b"%pane-mode-changed %8\n", &mut events);
        assert_eq!(events, vec![ControlEvent::PaneModeChanged("%8".into())]);
    }

    #[test]
    fn extracts_osc52_bel_and_st() {
        let hello = base64_decode(b"aGVsbG8=").unwrap();
        assert_eq!(hello, b"hello");
        let mut buf = b"\x1b]52;c;aGVsbG8=\x07%output %1 x\n".to_vec();
        let got = extract_osc52(&mut buf);
        assert_eq!(got, vec!["hello".to_string()]);
        assert_eq!(buf, b"%output %1 x\n");

        let mut buf = b"\x1b]52;c;d29ybGQ=\x1b\\".to_vec();
        assert_eq!(extract_osc52(&mut buf), vec!["world".to_string()]);
        assert!(buf.is_empty());
    }

    #[test]
    fn osc52_incomplete_stays_in_buf() {
        let mut buf = b"\x1b]52;c;aGVs".to_vec();
        assert!(extract_osc52(&mut buf).is_empty());
        assert_eq!(buf, b"\x1b]52;c;aGVs");
    }

    #[test]
    fn parser_emits_clipboard_from_osc52() {
        let mut p = ControlParser::new();
        let mut events = Vec::new();
        p.feed(b"\x1b]52;c;aGk=\x07%pane-mode-changed %1\n", &mut events);
        assert_eq!(
            events,
            vec![
                ControlEvent::Clipboard("hi".into()),
                ControlEvent::PaneModeChanged("%1".into()),
            ]
        );
    }

    #[test]
    fn block_keeps_empty_lines() {
        // A capture-pane snapshot has blank rows; they must survive the
        // block (the screen parser needs them to place content correctly).
        let mut p = ControlParser::new();
        let mut events = Vec::new();
        p.feed(b"%begin 1 2 0\nline1\n\nline3\n%end 1 2 0\n", &mut events);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ControlEvent::Block("line1\n\nline3\n".into()));
    }

    #[test]
    fn parses_blocks_and_notifications() {
        let mut p = ControlParser::new();
        let mut events = Vec::new();
        p.feed(
            b"%begin 1 2 0\nhello\nworld\n%end 1 2 0\n%output %0 hello\\012\\033[1m\\134\n%session-changed $0 main\n%exit\n",
            &mut events,
        );
        assert_eq!(events.len(), 4);
        assert_eq!(events[0], ControlEvent::Block("hello\nworld\n".into()));
        match &events[1] {
            ControlEvent::Output { pane, data } => {
                assert_eq!(pane, "%0");
                assert_eq!(data, b"hello\n\x1b[1m\\");
            }
            other => panic!("expected Output, got {:?}", other),
        }
        assert_eq!(events[2], ControlEvent::SessionChanged("main".into()));
        assert!(matches!(events[3], ControlEvent::Exit(_)));
    }

    #[test]
    fn strips_dcs_envelope() {
        let mut p = ControlParser::new();
        let mut events = Vec::new();
        p.feed(
            b"\x1bP1000p%begin 1 2 0\nhi\n%end 1 2 0\n\x1b\\",
            &mut events,
        );
        assert_eq!(events, vec![ControlEvent::Block("hi\n".into())]);
    }

    #[test]
    fn unescapes_octal() {
        assert_eq!(unescape(r"a\040b\134c"), b"a b\\c");
        assert_eq!(unescape(r"\033[31m"), b"\x1b[31m".to_vec());
    }

    #[test]
    fn routes_windows_format() {
        assert_eq!(
            route_block("W main 1 grok * [80x24]\nW main 2 fish - [80x24]\n"),
            BlockKind::Windows(vec![
                "W main 1 grok * [80x24]".into(),
                "W main 2 fish - [80x24]".into()
            ])
        );
        // Untagged "N: name" is a capture row, not a window list.
        assert_eq!(
            route_block("0: main * [80x24]\n1: second  [80x24]\n"),
            BlockKind::Screen
        );
    }

    #[test]
    fn routes_panes_format() {
        assert_eq!(
            route_block("P main 1 %5 0 grok [80x24] 1\nP work 1 %8 0 bash [80x24] 0\n"),
            BlockKind::Panes(vec![
                "P main 1 %5 0 grok [80x24] 1".into(),
                "P work 1 %8 0 bash [80x24] 0".into()
            ])
        );
    }

    #[test]
    fn routes_sessions_format() {
        assert_eq!(
            route_block("S main $0 1 2\nS grok $1 0 1\n"),
            BlockKind::Sessions(vec!["S main $0 1 2".into(), "S grok $1 0 1".into()])
        );
        // Default list-sessions text is "name: N windows …" — that is
        // Windows-shaped (`main:` is not all digits, so Screen) and must
        // not become Sessions.
        assert_eq!(
            route_block("main: 1 windows (created) [80x24] (attached)\n"),
            BlockKind::Screen
        );
    }

    #[test]
    fn screen_content_not_routed() {
        assert_eq!(
            route_block("qingshan in outbox in ~\n❯ echo hi\n"),
            BlockKind::Screen
        );
        assert_eq!(route_block(""), BlockKind::Discard);
        assert_eq!(route_block("\n"), BlockKind::Discard);
        assert_eq!(route_block("   \n"), BlockKind::Discard);
        assert_eq!(route_block(&"\n".repeat(24)), BlockKind::Screen);
        // A screen with one "N:" line must not be mistaken for a window list.
        assert_eq!(
            route_block("1: not a list\nplain line\n"),
            BlockKind::Screen
        );
    }

    #[test]
    fn routes_file_list() {
        let kind =
            route_block("CWD /home/qs/proj\nF ../\nF src/\nF README.md\nF file with space.txt\n");
        match kind {
            BlockKind::Files(list) => {
                assert_eq!(list.cwd, "/home/qs/proj");
                assert_eq!(
                    list.entries,
                    vec![
                        FileEntry {
                            name: "..".into(),
                            dir: true
                        },
                        FileEntry {
                            name: "src".into(),
                            dir: true
                        },
                        FileEntry {
                            name: "README.md".into(),
                            dir: false
                        },
                        FileEntry {
                            name: "file with space.txt".into(),
                            dir: false
                        },
                    ]
                );
                assert!(!list.truncated);
            }
            other => panic!("expected Files, got {:?}", other),
        }
    }

    #[test]
    fn file_list_skips_dot_and_empty_is_still_files() {
        let kind = route_block("CWD /tmp\nF ./\nF .\n");
        match kind {
            BlockKind::Files(list) => {
                assert_eq!(list.cwd, "/tmp");
                assert!(list.entries.is_empty());
            }
            other => panic!("expected Files, got {:?}", other),
        }
        assert_eq!(
            route_block("CWD /tmp\n"),
            BlockKind::Files(FileList {
                cwd: "/tmp".into(),
                entries: vec![],
                truncated: false,
            })
        );
    }

    #[test]
    fn file_list_does_not_steal_captures() {
        assert_eq!(route_block("CWD /tmp\nhello\n"), BlockKind::Screen);
        assert_eq!(route_block("F README.md\n"), BlockKind::Screen);
        assert_eq!(route_block("not a list\nF extra\n"), BlockKind::Screen);
    }

    #[test]
    fn file_list_truncates_after_limit() {
        let mut text = String::from("CWD /tmp\n");
        for i in 0..(FILE_LIST_LIMIT + 3) {
            text.push_str(&format!("F f{i}\n"));
        }
        match route_block(&text) {
            BlockKind::Files(list) => {
                assert_eq!(list.entries.len(), FILE_LIST_LIMIT);
                assert!(list.truncated);
                assert_eq!(list.entries[0].name, "f0");
            }
            other => panic!("expected Files, got {:?}", other),
        }
    }

    #[test]
    fn handles_chunk_boundaries() {
        let mut p = ControlParser::new();
        let mut events = Vec::new();
        p.feed(b"%begin 1 2 0\nhel", &mut events);
        assert!(events.is_empty());
        p.feed(b"lo\n%end 1 2 0\n", &mut events);
        assert_eq!(events, vec![ControlEvent::Block("hello\n".into())]);
    }

    #[test]
    fn utf8_split_across_chunks_in_block() {
        // `─` is e2 94 80. Lossy-decoding each TCP chunk used to replace
        // the split scalar with two U+FFFD cells in the capture.
        let mut p = ControlParser::new();
        let mut events = Vec::new();
        p.feed(b"%begin 1 2 0\n\xe2\x94", &mut events);
        assert!(events.is_empty());
        p.feed(b"\x80\n%end 1 2 0\n", &mut events);
        assert_eq!(events, vec![ControlEvent::Block("─\n".into())]);
        match &events[0] {
            ControlEvent::Block(t) => assert!(!t.contains('\u{FFFD}')),
            other => panic!("expected Block, got {:?}", other),
        }
    }

    #[test]
    fn output_utf8_split_across_notifications_keeps_raw_bytes() {
        // Grok's prompt box is a run of U+2500 `─`. tmux emits each
        // `%output` as its own line, so a scalar can be split across two
        // complete notifications. Lossy-decoding the line turned that
        // into three U+FFFD cells on the same row.
        let mut p = ControlParser::new();
        let mut events = Vec::new();
        p.feed(b"%output %0 \xe2\n", &mut events);
        p.feed(b"%output %0 \x94\n", &mut events);
        p.feed(b"%output %0 \x80\n", &mut events);
        assert_eq!(events.len(), 3);
        match (&events[0], &events[1], &events[2]) {
            (
                ControlEvent::Output { data: a, .. },
                ControlEvent::Output { data: b, .. },
                ControlEvent::Output { data: c, .. },
            ) => {
                assert_eq!(a, &[0xe2]);
                assert_eq!(b, &[0x94]);
                assert_eq!(c, &[0x80]);
                assert!(!a.contains(&0xef) && !b.contains(&0xef) && !c.contains(&0xef));
            }
            other => panic!("expected three Output events, got {:?}", other),
        }
    }

    #[test]
    fn output_utf8_split_across_notifications_renders_one_dash() {
        let mut p = ControlParser::new();
        let mut events = Vec::new();
        p.feed(
            b"%output %0 \xe2\n%output %0 \x94\n%output %0 \x80\n",
            &mut events,
        );
        let mut s = crate::screen::Screen::new(10, 1);
        for ev in events {
            if let ControlEvent::Output { data, .. } = ev {
                s.feed(&data);
            }
        }
        let row = &s.render().0[0];
        assert_eq!(row.chars().next(), Some('─'));
        assert!(!row.contains('\u{FFFD}'));
        assert_eq!(row.chars().filter(|&c| c == '─').count(), 1);
    }
}
