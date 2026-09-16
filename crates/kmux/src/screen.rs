//! The terminal screen model + VT escape-sequence handler (via the `vte`
//! crate). Host-testable: feed it a byte stream and inspect the resulting
//! screen.
//!
//! v1 rendering scope: character cells with bold/inverse attributes (e-ink
//! friendly); colors are parsed but flattened to grayscale (fg/bg stored,
//! rendering optional later). Cursor, erase, scrolling, tab stops, alternate
//! screen, saved cursor, and a scrollback buffer (most recent lines) are
//! supported — enough for vim/less/htop over a control-mode tmux session.

use vte::{Params, Parser, Perform};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub ch: char,
    pub bold: bool,
    pub inverse: bool,
    pub fg: u8, // 0-7 ANSI, 9 = default
    pub bg: u8,
}

impl Cell {
    fn blank() -> Self {
        Cell {
            ch: ' ',
            bold: false,
            inverse: false,
            fg: 9,
            bg: 9,
        }
    }
}

/// Copy-mode selection in capture-dump rows (`y` is dump-relative, 0 =
/// first captured line). `capture-pane` of history has no mode-style, so
/// the overlay paints this as inverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopySelection {
    pub rect: bool,
    pub x0: usize,
    pub y0: isize,
    pub x1: usize,
    pub y1: isize,
}

impl CopySelection {
    /// `y` from tmux is backing-grid absolute (`hsize + cy - oy`).
    pub fn from_backing(
        rect: bool,
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
        hist: usize,
        oy: usize,
    ) -> Self {
        Self {
            rect,
            x0,
            y0: abs_to_dump_y(y0, hist, oy),
            x1,
            y1: abs_to_dump_y(y1, hist, oy),
        }
    }
}

fn abs_to_dump_y(abs_y: usize, hist: usize, oy: usize) -> isize {
    abs_y as isize + oy as isize - hist as isize
}

pub struct Screen {
    pub rows: Vec<Vec<Cell>>,
    pub cols: usize,
    pub rows_count: usize,
    pub cursor_x: usize,
    pub cursor_y: usize,
    /// Scrolled-out lines (newest first), plain text per line.
    pub scrollback: Vec<String>,
    pub scrollback_limit: usize,
    pub alt_screen: bool,
    saved_x: usize,
    saved_y: usize,
    saved_alt: Option<(Vec<Vec<Cell>>, usize, usize)>,
    tab_stops: Vec<usize>,
    wrap_pending: bool,
    /// When set, printable characters that would wrap are clipped instead
    /// of moving to the next row. Used while placing capture-pane lines
    /// so a 183-col dump does not scramble an 80-col model.
    clip_wrap: bool,
    cur_bold: bool,
    cur_inverse: bool,
    cur_fg: u8,
    cur_bg: u8,
    /// Kept across `feed` calls so a UTF-8 scalar split between tmux
    /// `%output` chunks (e.g. box-drawing `─` = e2 94 80) is completed
    /// instead of turning into U+FFFD replacement characters.
    parser: Parser,
}

impl Screen {
    pub fn new(cols: usize, rows_count: usize) -> Self {
        let mut s = Screen {
            rows: vec![vec![Cell::blank(); cols]; rows_count],
            cols,
            rows_count,
            cursor_x: 0,
            cursor_y: 0,
            scrollback: Vec::new(),
            scrollback_limit: 500,
            alt_screen: false,
            saved_x: 0,
            saved_y: 0,
            saved_alt: None,
            tab_stops: Vec::new(),
            wrap_pending: false,
            clip_wrap: false,
            cur_bold: false,
            cur_inverse: false,
            cur_fg: 9,
            cur_bg: 9,
            parser: Parser::new(),
        };
        let mut t = 8;
        while t < cols {
            s.tab_stops.push(t);
            t += 8;
        }
        s
    }

    fn push_scrollback(&mut self) {
        if self.alt_screen {
            return;
        }
        let line: String = self.rows[0].iter().map(|c| c.ch).collect();
        if !line.trim_end().is_empty() {
            self.scrollback.insert(0, line);
            if self.scrollback.len() > self.scrollback_limit {
                self.scrollback.truncate(self.scrollback_limit);
            }
        }
    }

    fn scroll_up(&mut self, n: usize) {
        for _ in 0..n {
            self.push_scrollback();
            self.rows.remove(0);
            self.rows.push(vec![Cell::blank(); self.cols]);
        }
    }

    fn scroll_down(&mut self, n: usize) {
        for _ in 0..n {
            self.rows.insert(0, vec![Cell::blank(); self.cols]);
            self.rows.truncate(self.rows_count);
        }
    }

    /// Process a byte stream through the VT parser.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut parser = std::mem::take(&mut self.parser);
        parser.advance(self, bytes);
        self.parser = parser;
    }

    /// Blank the cells, cursor, scrollback, and parser. Size is kept.
    pub fn reset(&mut self) {
        let cols = self.cols;
        let rows = self.rows_count;
        let limit = self.scrollback_limit;
        *self = Screen::new(cols, rows);
        self.scrollback_limit = limit;
    }

    /// Load a `capture-pane` dump as the whole screen.
    ///
    /// `capture-pane -p -S -N -e` is history lines followed by every row of
    /// the pane, including trailing blanks. When the dump is at least
    /// `rows_count` lines and the last `rows_count` contain a printable
    /// cell, those rows are the live view and everything above them is
    /// scrollback. Trailing blanks are only dropped when the last
    /// `rows_count` rows are empty (a taller pane with the prompt at the
    /// top) so that prompt is not pushed into scrollback on an 80x24
    /// model.
    pub fn load_snapshot(&mut self, text: &str) {
        self.reset();
        let mut lines: Vec<&str> = text.split('\n').collect();
        if lines.last() == Some(&"") {
            lines.pop();
        }
        if lines.is_empty() {
            return;
        }

        if lines.len() >= self.rows_count {
            let screen_start = lines.len() - self.rows_count;
            let screen_blank = lines[screen_start..]
                .iter()
                .all(|l| line_is_visually_blank(l));
            if !screen_blank {
                self.push_history_lines(&lines[..screen_start]);
                for (i, line) in lines[screen_start..].iter().enumerate() {
                    self.place_line(i, line);
                }
                return;
            }
        }

        while lines.last().is_some_and(|l| line_is_visually_blank(l)) {
            lines.pop();
        }
        if lines.is_empty() {
            return;
        }
        let vis_start = lines.len().saturating_sub(self.rows_count);
        self.push_history_lines(&lines[..vis_start]);
        for (i, line) in lines[vis_start..].iter().enumerate() {
            self.place_line(i, line);
        }
    }

    fn push_history_lines(&mut self, lines: &[&str]) {
        for line in lines {
            let plain = visible_text(line);
            if !plain.trim().is_empty() {
                self.scrollback.insert(0, plain);
                if self.scrollback.len() > self.scrollback_limit {
                    self.scrollback.truncate(self.scrollback_limit);
                }
            }
        }
    }

    fn place_line(&mut self, y: usize, line: &str) {
        if y >= self.rows_count {
            return;
        }
        self.cursor_x = 0;
        self.cursor_y = y;
        self.wrap_pending = false;
        self.clip_wrap = true;
        self.feed(line.as_bytes());
        self.clip_wrap = false;
        self.wrap_pending = false;
    }

    /// Load a copy-mode overlay dump (`capture-pane -M`).
    ///
    /// Extra lines are more pane rows (a desktop client is often 183x50),
    /// not scrollback. Keep `cursor_y` visible in this model's `rows_count`
    /// so arrow keys slide the 24-row Kindle view instead of sitting on
    /// the last 24 of a 50-row dump.
    pub fn load_mode_snapshot(
        &mut self,
        text: &str,
        cursor_x: usize,
        cursor_y: usize,
        sel: Option<CopySelection>,
    ) {
        self.reset();
        let mut lines: Vec<&str> = text.split('\n').collect();
        if lines.last() == Some(&"") {
            lines.pop();
        }
        if lines.is_empty() {
            return;
        }
        let n = lines.len();
        let cy = cursor_y.min(n - 1);
        let start = if n <= self.rows_count {
            0
        } else {
            let max_start = n - self.rows_count;
            cy.saturating_sub(self.rows_count - 1).min(max_start)
        };
        let end = (start + self.rows_count).min(n);
        for (i, line) in lines[start..end].iter().enumerate() {
            self.place_line(i, line);
        }
        self.cursor_x = cursor_x.min(self.cols.saturating_sub(1));
        self.cursor_y = cy
            .saturating_sub(start)
            .min(self.rows_count.saturating_sub(1));
        if let Some(sel) = sel {
            self.paint_copy_selection(start, sel);
        }
    }

    fn paint_copy_selection(&mut self, dump_start: usize, sel: CopySelection) {
        if self.cols == 0 || self.rows_count == 0 {
            return;
        }
        let xmax = self.cols - 1;
        if sel.rect {
            let ymin = sel.y0.min(sel.y1);
            let ymax = sel.y0.max(sel.y1);
            let xmin = sel.x0.min(sel.x1);
            let xmax_s = sel.x1.max(sel.x0).min(xmax);
            if xmin > xmax_s {
                return;
            }
            for y in 0..self.rows_count {
                let dy = dump_start as isize + y as isize;
                if dy < ymin || dy > ymax {
                    continue;
                }
                for x in xmin..=xmax_s {
                    self.rows[y][x].inverse = true;
                }
            }
            return;
        }
        let (x0, y0, x1, y1) = if sel.y0 < sel.y1 || (sel.y0 == sel.y1 && sel.x0 <= sel.x1) {
            (sel.x0, sel.y0, sel.x1, sel.y1)
        } else {
            (sel.x1, sel.y1, sel.x0, sel.y0)
        };
        for y in 0..self.rows_count {
            let dy = dump_start as isize + y as isize;
            if dy < y0 || dy > y1 {
                continue;
            }
            let from = if dy == y0 { x0.min(xmax) } else { 0 };
            let to = if dy == y1 { x1.min(xmax) } else { xmax };
            if from > to {
                continue;
            }
            for x in from..=to {
                self.rows[y][x].inverse = true;
            }
        }
    }

    /// Line count of a `capture-pane -M` dump (trailing newline ignored).
    pub fn mode_dump_line_count(text: &str) -> usize {
        let mut n = text.split('\n').count();
        if text.ends_with('\n') {
            n = n.saturating_sub(1);
        }
        n
    }

    /// Place the cursor on the first inverse cell (copy-mode highlight).
    pub fn cursor_from_inverse(&mut self) {
        for y in 0..self.rows_count {
            for x in 0..self.cols {
                if self.rows[y][x].inverse {
                    self.cursor_x = x;
                    self.cursor_y = y;
                    return;
                }
            }
        }
    }

    /// True when a capture-pane dump has no printable cells (a brand-new
    /// window, captured before the shell prints its prompt). Applying that
    /// dump would wipe live `%output` that already drew the prompt.
    pub fn snapshot_is_blank(text: &str) -> bool {
        text.lines().all(line_is_visually_blank)
    }

    /// Render the screen as (text, attrs) rows for status.json. attrs is a
    /// parallel string per row: '.' normal, 'b' bold, 'i' inverse, 'B' both.
    pub fn render(&self) -> (Vec<String>, Vec<String>) {
        let mut text = Vec::with_capacity(self.rows_count);
        let mut attrs = Vec::with_capacity(self.rows_count);
        for row in &self.rows {
            let mut t = String::with_capacity(self.cols);
            let mut a = String::with_capacity(self.cols);
            for c in row {
                t.push(c.ch);
                a.push(match (c.bold, c.inverse) {
                    (true, true) => 'B',
                    (true, false) => 'b',
                    (false, true) => 'i',
                    (false, false) => '.',
                });
            }
            text.push(t);
            attrs.push(a);
        }
        (text, attrs)
    }
}

/// Drop CSI/OSC so a capture line of only `\x1b[0m` counts as blank.
fn line_is_visually_blank(s: &str) -> bool {
    visible_text(s).trim().is_empty()
}

fn visible_text(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b {
            i += 1;
            if i >= b.len() {
                break;
            }
            match b[i] {
                b'[' => {
                    i += 1;
                    while i < b.len() && !(b[i] >= b'@' && b[i] <= b'~') {
                        i += 1;
                    }
                    if i < b.len() {
                        i += 1;
                    }
                }
                b']' => {
                    i += 1;
                    while i < b.len() && b[i] != 0x07 {
                        if b[i] == 0x1b {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                    if i < b.len() && b[i] == 0x07 {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
            continue;
        }
        let ch_len = match b[i] {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => 1,
        };
        let end = (i + ch_len).min(b.len());
        if let Ok(ch) = std::str::from_utf8(&b[i..end]) {
            out.push_str(ch);
        }
        i = end;
    }
    out
}

/// vte 0.14 exposes params as subparam slices; take the first subvalue of
/// param `i` as i64 (or `dflt`).
fn param_or(params: &Params, i: usize, dflt: i64) -> i64 {
    param_at(params, i).unwrap_or(dflt)
}

fn param_at(params: &Params, i: usize) -> Option<i64> {
    params
        .iter()
        .nth(i)
        .and_then(|p| p.first())
        .map(|&v| v as i64)
}

impl Perform for Screen {
    fn print(&mut self, c: char) {
        if c == '\u{0}' {
            return;
        }
        if self.clip_wrap && self.cursor_x >= self.cols {
            return;
        }
        if self.wrap_pending {
            self.wrap_pending = false;
            if self.cursor_y + 1 >= self.rows_count {
                self.scroll_up(1);
            } else {
                self.cursor_y += 1;
            }
            self.cursor_x = 0;
        }
        let cell = Cell {
            ch: c,
            bold: self.cur_bold,
            inverse: self.cur_inverse,
            fg: self.cur_fg,
            bg: self.cur_bg,
        };
        if let Some(row) = self.rows.get_mut(self.cursor_y) {
            if self.cursor_x < self.cols {
                row[self.cursor_x] = cell;
                self.cursor_x += 1;
            }
            if self.cursor_x >= self.cols {
                if !self.clip_wrap {
                    self.wrap_pending = true;
                }
            }
        }
    }

    fn execute(&mut self, b: u8) {
        match b {
            b'\r' => {
                self.cursor_x = 0;
                self.wrap_pending = false;
            }
            b'\n' | b'\x0b' | b'\x0c' => {
                if self.cursor_y + 1 >= self.rows_count {
                    self.scroll_up(1);
                } else {
                    self.cursor_y += 1;
                }
                self.cursor_x = 0; // LF is line feed + carriage return here
                self.wrap_pending = false;
            }
            b'\x08' => {
                if self.cursor_x > 0 {
                    self.cursor_x -= 1;
                }
                self.wrap_pending = false;
            }
            b'\t' => {
                self.wrap_pending = false;
                for &t in &self.tab_stops {
                    if t > self.cursor_x {
                        self.cursor_x = t;
                        return;
                    }
                }
                self.cursor_x = self.cols - 1;
            }
            b'\x07' => {} // bell: ignore on e-ink
            b'\x1b' => {} // handled by the parser
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        match action {
            'A' => {
                self.cursor_y = self
                    .cursor_y
                    .saturating_sub(param_or(params, 0, 1) as usize)
            }
            'B' => {
                self.cursor_y =
                    (self.cursor_y + param_or(params, 0, 1) as usize).min(self.rows_count - 1)
            }
            'C' => {
                self.cursor_x = (self.cursor_x + param_or(params, 0, 1) as usize).min(self.cols - 1)
            }
            'D' => {
                self.cursor_x = self
                    .cursor_x
                    .saturating_sub(param_or(params, 0, 1) as usize)
            }
            'E' => {
                self.cursor_x = 0;
                self.cursor_y =
                    (self.cursor_y + param_or(params, 0, 1) as usize).min(self.rows_count - 1)
            }
            'F' => {
                self.cursor_x = 0;
                self.cursor_y = self
                    .cursor_y
                    .saturating_sub(param_or(params, 0, 1) as usize)
            }
            'G' => {
                self.cursor_x = (param_or(params, 0, 1) as usize)
                    .saturating_sub(1)
                    .min(self.cols - 1)
            }
            'H' | 'f' => {
                let row = (param_or(params, 0, 1) as usize)
                    .saturating_sub(1)
                    .min(self.rows_count - 1);
                let col = (param_or(params, 1, 1) as usize)
                    .saturating_sub(1)
                    .min(self.cols - 1);
                self.cursor_y = row;
                self.cursor_x = col;
                self.wrap_pending = false;
            }
            'J' => match param_or(params, 0, 0) {
                0 => self.erase_display(false),
                1 => self.erase_display(true),
                2 | 3 => {
                    self.erase_display(false);
                    self.erase_display(true);
                }
                _ => {}
            },
            'K' => match param_or(params, 0, 0) {
                0 => self.erase_line(false),
                1 => self.erase_line(true),
                2 => {
                    self.erase_line(false);
                    self.erase_line(true);
                }
                _ => {}
            },
            'X' => {
                let n = param_or(params, 0, 1) as usize;
                if let Some(row) = self.rows.get_mut(self.cursor_y) {
                    for cell in row.iter_mut().skip(self.cursor_x).take(n) {
                        *cell = Cell::blank();
                    }
                }
            }
            'S' => self.scroll_up(param_or(params, 0, 1) as usize),
            'T' => self.scroll_down(param_or(params, 0, 1) as usize),
            'm' => self.sgr(params),
            'h' | 'l' => {
                // DEC private modes (alt screen, etc.): vte delivers a
                // leading '?' private-mode prefix as param 0 = 0, with the
                // mode number in param 1.
                let mode = param_at(params, 1).or_else(|| param_at(params, 0));
                if mode == Some(1049) || mode == Some(1047) {
                    self.set_alt_screen(action == 'h');
                }
            }
            'r' => {} // scroll region: v1 treats the whole screen
            'd' => {
                self.cursor_y = (param_or(params, 0, 1) as usize)
                    .saturating_sub(1)
                    .min(self.rows_count - 1)
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (intermediates.first().copied(), byte) {
            (None, b'7') => {
                self.saved_x = self.cursor_x;
                self.saved_y = self.cursor_y;
            }
            (None, b'8') => {
                self.cursor_x = self.saved_x.min(self.cols - 1);
                self.cursor_y = self.saved_y.min(self.rows_count - 1);
            }
            (Some(b'='), _) | (Some(b'>'), _) => {} // keypad modes: ignore
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bel_term: bool) {
        // Window title etc.: not rendered.
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _b: u8) {}
    fn unhook(&mut self) {}
}

impl Screen {
    fn erase_display(&mut self, above: bool) {
        if above {
            for y in 0..=self.cursor_y {
                if y == self.cursor_y {
                    for x in 0..=self.cursor_x {
                        self.rows[y][x] = Cell::blank();
                    }
                } else {
                    self.rows[y] = vec![Cell::blank(); self.cols];
                }
            }
        } else {
            for y in self.cursor_y..self.rows_count {
                if y == self.cursor_y {
                    for x in self.cursor_x..self.cols {
                        self.rows[y][x] = Cell::blank();
                    }
                } else {
                    self.rows[y] = vec![Cell::blank(); self.cols];
                }
            }
        }
    }

    fn erase_line(&mut self, left: bool) {
        if let Some(row) = self.rows.get_mut(self.cursor_y) {
            if left {
                for cell in row.iter_mut().take(self.cursor_x + 1) {
                    *cell = Cell::blank();
                }
            } else {
                for cell in row.iter_mut().skip(self.cursor_x) {
                    *cell = Cell::blank();
                }
            }
        }
    }

    fn set_alt_screen(&mut self, on: bool) {
        if on == self.alt_screen {
            return;
        }
        if on {
            self.saved_alt = Some((
                std::mem::replace(
                    &mut self.rows,
                    vec![vec![Cell::blank(); self.cols]; self.rows_count],
                ),
                self.cursor_x,
                self.cursor_y,
            ));
            self.cursor_x = 0;
            self.cursor_y = 0;
        } else if let Some((main, x, y)) = self.saved_alt.take() {
            self.rows = main;
            self.cursor_x = x;
            self.cursor_y = y;
        }
        self.alt_screen = on;
    }

    fn sgr(&mut self, params: &Params) {
        if params.is_empty() {
            self.cur_bold = false;
            self.cur_inverse = false;
            self.cur_fg = 9;
            self.cur_bg = 9;
            return;
        }
        let mut i = 0usize;
        let n = params.len();
        while i < n {
            let p = param_at(params, i).unwrap_or(0);
            match p {
                0 => {
                    self.cur_bold = false;
                    self.cur_inverse = false;
                    self.cur_fg = 9;
                    self.cur_bg = 9;
                }
                1 | 22 => self.cur_bold = p == 1,
                7 | 27 => self.cur_inverse = p == 7,
                30..=37 => self.cur_fg = (p - 30) as u8,
                39 => self.cur_fg = 9,
                40..=47 => self.cur_bg = (p - 40) as u8,
                49 => self.cur_bg = 9,
                90..=97 => self.cur_fg = (p - 90) as u8 + 8,
                100..=107 => self.cur_bg = (p - 100) as u8 + 8,
                38 | 48 => {
                    // Extended colors (256/truecolor): consume the params,
                    // store a flattened grayscale hint (bright if > 8).
                    let fg = if p == 38 {
                        &mut self.cur_fg
                    } else {
                        &mut self.cur_bg
                    };
                    if param_at(params, i + 1) == Some(5) {
                        // 256-color: keep the index (0-15 → grayscale levels).
                        *fg = param_at(params, i + 2).unwrap_or(7) as u8 % 16;
                        i += 2;
                    } else if param_at(params, i + 1) == Some(2) {
                        let r = param_at(params, i + 2).unwrap_or(255) as u8;
                        let g = param_at(params, i + 3).unwrap_or(255) as u8;
                        let b = param_at(params, i + 4).unwrap_or(255) as u8;
                        let lum = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) as u8;
                        *fg = if lum > 128 { 15 } else { 0 };
                        i += 4;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// Offset is rows-from-live (0 = the live screen). A scrolled view stays
/// on the same history lines when new output is pushed; live stays live.
///
/// A wipe (`new_len == 0`, switch/create/close recapture) keeps the offset
/// so the following snapshot does not snap to live. Growth from an empty
/// buffer is a replacement dump, not new output, so it does not add to the
/// offset. Other shrinks clamp to the new length.
pub fn pin_scroll_offset(offset: usize, old_len: usize, new_len: usize) -> usize {
    if new_len == 0 {
        return offset;
    }
    let mut off = offset;
    if off > 0 && old_len > 0 && new_len > old_len {
        off += new_len - old_len;
    }
    off.min(new_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank_screen(cols: usize, rows: usize) -> Screen {
        Screen::new(cols, rows)
    }

    #[test]
    fn pin_scroll_offset_keeps_live_at_zero() {
        assert_eq!(pin_scroll_offset(0, 10, 20), 0);
    }

    #[test]
    fn pin_scroll_offset_grows_with_new_history() {
        assert_eq!(pin_scroll_offset(24, 50, 60), 34);
    }

    #[test]
    fn pin_scroll_offset_clamps_when_history_shrinks() {
        assert_eq!(pin_scroll_offset(40, 50, 10), 10);
    }

    #[test]
    fn pin_scroll_offset_keeps_offset_across_wipe() {
        assert_eq!(pin_scroll_offset(24, 50, 0), 24);
        assert_eq!(pin_scroll_offset(24, 0, 80), 24);
    }

    #[test]
    fn pin_scroll_offset_unchanged_when_length_holds() {
        assert_eq!(pin_scroll_offset(24, 500, 500), 24);
    }

    #[test]
    fn prints_chars_and_wraps() {
        let mut s = blank_screen(5, 3);
        s.feed(b"abc");
        assert_eq!(s.render().0[0], "abc  ");
        s.feed(b"de"); // 5 chars fill the row
        assert_eq!(s.render().0[0], "abcde");
        s.feed(b"f"); // next char wraps to row 1
        assert_eq!(s.render().0[1], "f    ");
        assert_eq!(s.cursor_y, 1);
    }

    #[test]
    fn snapshot_without_trailing_newline_keeps_cursor_on_last_row() {
        // A capture-pane snapshot fed without its final line terminator must
        // leave the cursor right after the last content, not on a fresh line.
        let mut s = blank_screen(6, 3);
        s.feed(b"ab\ncd\nef");
        assert_eq!(s.cursor_x, 2);
        assert_eq!(s.cursor_y, 2);
        assert_eq!(s.render().0[2], "ef    ");
    }

    #[test]
    fn line_feed_scrolls_and_records_scrollback() {
        let mut s = blank_screen(5, 2);
        s.feed(b"aaaa");
        s.feed(b"\n");
        s.feed(b"bbbb");
        s.feed(b"\n");
        assert_eq!(s.render().0[0], "bbbb ");
        assert_eq!(s.scrollback[0], "aaaa ");
    }

    #[test]
    fn sgr_bold_inverse() {
        let mut s = blank_screen(10, 2);
        s.feed(b"\x1b[1;7mX");
        assert_eq!(s.render().1[0].chars().next(), Some('B'));
        s.feed(b"\x1b[0mY");
        assert_eq!(s.render().1[0].chars().nth(1), Some('.'));
    }

    #[test]
    fn cursor_moves_and_erases() {
        let mut s = blank_screen(10, 3);
        s.feed(b"hello");
        s.feed(b"\x1b[1D\x1b[K"); // back, erase to end
        assert_eq!(s.render().0[0], "hell      ");
        s.feed(b"\x1b[2;3HZ");
        assert_eq!(s.render().0[1], "  Z       ");
    }

    #[test]
    fn alternate_screen_roundtrip() {
        let mut s = blank_screen(10, 3);
        s.feed(b"main");
        s.feed(b"\x1b[?1049h");
        s.feed(b"alt");
        assert!(s.alt_screen);
        assert_eq!(s.render().0[0], "alt       ");
        s.feed(b"\x1b[?1049l");
        assert!(!s.alt_screen);
        assert_eq!(s.render().0[0], "main      ");
    }

    #[test]
    fn clear_screen() {
        let mut s = blank_screen(5, 2);
        s.feed(b"abcd\x1b[H\x1b[2J");
        assert_eq!(s.render().0[0], "     ");
    }

    #[test]
    fn extended_color_flattens_to_grayscale() {
        let mut s = blank_screen(10, 1);
        s.feed(b"\x1b[38;5;9mX\x1b[38;2;10;10;10mY");
        let row = &s.rows[0];
        assert_eq!(row[0].fg, 9); // bright red index kept
        assert_eq!(row[1].fg, 0); // dark -> dark
    }

    #[test]
    fn load_snapshot_replaces_existing_content() {
        let mut s = blank_screen(8, 4);
        s.feed(b"oldold\nOLDOLD\nzzzz\nwwww");
        // 4-row dump of a fresh pane: prompt on row 0, rest blank. The
        // trailing newline is what capture-pane emits (and kmuxd strips).
        s.load_snapshot("prompt\n\n\n");
        let rows = s.render().0;
        assert_eq!(rows[0].trim_end(), "prompt");
        assert_eq!(rows[1].trim_end(), "");
        assert_eq!(rows[2].trim_end(), "");
        assert_eq!(rows[3].trim_end(), "");
        assert!(s.scrollback.is_empty());
    }

    #[test]
    fn feed_full_pane_dump_at_bottom_scrolls_prompt_away() {
        // Why load_snapshot exists: a new-window capture is ~rows of text
        // (prompt + trailing blanks). Feeding it at the live cursor (the
        // last row) scrolls the prompt into scrollback.
        let mut s = blank_screen(8, 4);
        s.feed(b"AAAA\nBBBB\nCCCC\nDDDD");
        s.feed(b"prompt\n\n\n");
        let rows = s.render().0;
        assert!(
            rows.iter().all(|r| r.trim_end() != "prompt"),
            "prompt should have scrolled off the live view: {:?}",
            rows
        );
    }

    #[test]
    fn load_mode_snapshot_follows_cursor_on_tall_pane() {
        // 10-row mode screen, 4-row Kindle view: arrows must slide the
        // window, not pin to the last 4 lines (load_snapshot's live view).
        let dump: String = (0..10).map(|i| format!("L{i}\n")).collect();
        let mut s = blank_screen(8, 4);
        s.load_mode_snapshot(&dump, 0, 9, None);
        let rows = s.render().0;
        assert_eq!(rows[0].trim_end(), "L6");
        assert_eq!(rows[3].trim_end(), "L9");
        assert_eq!(s.cursor_y, 3);

        s.load_mode_snapshot(&dump, 0, 8, None);
        let rows = s.render().0;
        assert_eq!(rows[0].trim_end(), "L5");
        assert_eq!(rows[3].trim_end(), "L8");
        assert_eq!(s.cursor_y, 3);

        s.load_mode_snapshot(&dump, 2, 0, None);
        let rows = s.render().0;
        assert_eq!(rows[0].trim_end(), "L0");
        assert_eq!((s.cursor_x, s.cursor_y), (2, 0));
    }

    #[test]
    fn copy_selection_from_backing_is_dump_relative() {
        let s = CopySelection::from_backing(false, 3, 95, 8, 97, 100, 10);
        assert_eq!((s.x0, s.y0, s.x1, s.y1), (3, 5, 8, 7));
    }

    #[test]
    fn load_mode_snapshot_paints_stream_selection() {
        let dump: String = (0..4).map(|_| "xxxxxxxx\n".to_string()).collect();
        let mut s = blank_screen(8, 4);
        s.load_mode_snapshot(
            &dump,
            0,
            1,
            Some(CopySelection {
                rect: false,
                x0: 2,
                y0: 1,
                x1: 3,
                y1: 2,
            }),
        );
        let attrs = s.render().1;
        assert_eq!(&attrs[1][2..], "iiiiii");
        assert_eq!(&attrs[2][..4], "iiii");
        assert!(attrs[0].chars().all(|c| c == '.'));
        assert!(attrs[3].chars().all(|c| c == '.'));
    }

    #[test]
    fn load_mode_snapshot_paints_rect_selection() {
        let dump: String = (0..4).map(|_| "xxxxxxxx\n".to_string()).collect();
        let mut s = blank_screen(8, 4);
        s.load_mode_snapshot(
            &dump,
            0,
            1,
            Some(CopySelection {
                rect: true,
                x0: 2,
                y0: 1,
                x1: 4,
                y1: 2,
            }),
        );
        let attrs = s.render().1;
        assert_eq!(&attrs[1][2..=4], "iii");
        assert_eq!(&attrs[2][2..=4], "iii");
        assert_eq!(attrs[1].chars().filter(|&c| c == 'i').count(), 3);
    }

    #[test]
    fn load_mode_snapshot_selection_follows_window() {
        let dump: String = (0..10).map(|i| format!("L{i}xxxx\n")).collect();
        let mut s = blank_screen(8, 4);
        s.load_mode_snapshot(
            &dump,
            0,
            9,
            Some(CopySelection {
                rect: false,
                x0: 0,
                y0: 7,
                x1: 2,
                y1: 8,
            }),
        );
        let attrs = s.render().1;
        assert!(attrs[1].starts_with("iii"));
        assert_eq!(&attrs[2][..3], "iii");
        assert!(attrs[0].chars().all(|c| c == '.'));
    }

    #[test]
    fn cursor_from_inverse_finds_first_sgr7_cell() {
        let mut s = blank_screen(8, 4);
        s.load_snapshot("aaaa\nbb\x1b[7mX\x1b[0mcc\n\n");
        s.cursor_from_inverse();
        assert_eq!((s.cursor_x, s.cursor_y), (2, 1));
    }

    #[test]
    fn snapshot_is_blank_ignores_newlines() {
        assert!(Screen::snapshot_is_blank("\n\n\n\n"));
        assert!(Screen::snapshot_is_blank("   \n  \n"));
        assert!(Screen::snapshot_is_blank(""));
        assert!(!Screen::snapshot_is_blank("prompt\n\n\n"));
        assert!(!Screen::snapshot_is_blank("\n\x1b[1mhi"));
        assert!(Screen::snapshot_is_blank("\n\x1b[0m\n\x1b[?2004h\n"));
    }

    #[test]
    fn load_snapshot_tall_pane_keeps_prompt_at_top() {
        // Kindle model is 80x24; the desktop tmux client sizes new windows
        // to 183x50. A 50-row dump of a fresh shell is the prompt plus
        // trailing blanks — feeding that as VT scrolled the prompt away.
        let mut s = blank_screen(8, 4);
        let mut dump = String::from("prompt\n>\n");
        for _ in 0..48 {
            dump.push('\n');
        }
        s.load_snapshot(&dump);
        let rows = s.render().0;
        assert_eq!(rows[0].trim_end(), "prompt");
        assert_eq!(rows[1].trim_end(), ">");
        assert_eq!(rows[2].trim_end(), "");
        assert!(s.scrollback.is_empty());
    }

    #[test]
    fn load_snapshot_clips_wide_lines() {
        let mut s = blank_screen(4, 2);
        s.load_snapshot("abcdefgh\n");
        assert_eq!(s.render().0[0], "abcd");
        assert_eq!(s.cursor_y, 0);
    }

    #[test]
    fn load_snapshot_overflow_goes_to_scrollback() {
        let mut s = blank_screen(8, 2);
        s.load_snapshot("old1\nold2\nlive1\nlive2");
        let rows = s.render().0;
        assert_eq!(rows[0].trim_end(), "live1");
        assert_eq!(rows[1].trim_end(), "live2");
        assert_eq!(s.scrollback[0].trim_end(), "old2");
        assert_eq!(s.scrollback[1].trim_end(), "old1");
    }

    #[test]
    fn load_snapshot_history_prompt_stays_off_the_live_view() {
        // capture-pane -S -N on a 4-row pane: a previous prompt in history,
        // then the live rows (blank, prompt, glyph, blank). Stripping
        // trailing blanks first used to collapse that to two prompts.
        let mut s = blank_screen(16, 4);
        s.load_snapshot("\nprompt\n>\n\nprompt\n>\n\n");
        let rows = s.render().0;
        assert_eq!(rows[0].trim_end(), "");
        assert_eq!(rows[1].trim_end(), "prompt");
        assert_eq!(rows[2].trim_end(), ">");
        assert_eq!(rows[3].trim_end(), "");
        assert_eq!(
            rows.iter().filter(|r| r.trim_end() == "prompt").count(),
            1,
            "history prompt leaked onto the live view: {:?}",
            rows
        );
        assert_eq!(s.scrollback[0].trim_end(), ">");
        assert_eq!(s.scrollback[1].trim_end(), "prompt");
    }

    #[test]
    fn utf8_scalar_split_across_feeds_is_one_char() {
        // Grok's prompt footer is a run of U+2500 `─` (e2 94 80). tmux
        // %output can split that sequence across chunks; a fresh Parser
        // per feed turned one dash into two U+FFFD cells.
        let mut s = blank_screen(10, 1);
        s.feed(&[0xe2, 0x94]);
        s.feed(&[0x80]);
        let row = &s.render().0[0];
        assert_eq!(row.chars().next(), Some('─'));
        assert!(!row.contains('\u{FFFD}'));
        assert_eq!(row.chars().filter(|&c| c == '─').count(), 1);
    }
}
