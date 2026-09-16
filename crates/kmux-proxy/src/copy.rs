//! A frozen copy buffer gives both backends identical selection semantics.
use kmuxd::screen::{CopySelection, Screen};

pub struct CopyView {
    pub lines: Vec<String>,
    pub x: usize,
    pub y: usize,
    anchor: Option<(usize, usize)>,
    rect: bool,
    repeat: usize,
}
impl CopyView {
    pub fn new(text: &str) -> Self {
        let lines: Vec<String> = text
            .lines()
            .map(|line| {
                let mut s = Screen::new(4096, 1);
                s.load_snapshot(line);
                s.render()
                    .0
                    .first()
                    .cloned()
                    .unwrap_or_default()
                    .trim_end()
                    .to_string()
            })
            .collect();
        Self {
            y: lines.len().saturating_sub(1),
            lines,
            x: 0,
            anchor: None,
            rect: false,
            repeat: 0,
        }
    }
    /// Returns Some(text) when yanking, Some(empty) when cancelling.
    pub fn key(&mut self, key: &str) -> Result<Option<String>, String> {
        if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() && (key != "0" || self.repeat > 0) {
            self.repeat = (self.repeat * 10 + key.parse::<usize>().unwrap()).min(9999);
            return Ok(None);
        }
        let n = std::mem::take(&mut self.repeat).max(1);
        match key {
            "q" | "Escape" | "Esc" | "C-c" | "ScrollBottom" => return Ok(Some(String::new())),
            "y" | "Enter" | "C-j" => return Ok(Some(self.selected())),
            "k" | "Up" => self.y = self.y.saturating_sub(n),
            "j" | "Down" => self.y = self.y.saturating_add(n),
            "h" | "Left" => self.x = self.x.saturating_sub(n),
            "l" | "Right" => self.x = self.x.saturating_add(n),
            "PageUp" | "ScrollUp" | "C-b" => self.y = self.y.saturating_sub(24 * n),
            "PageDown" | "ScrollDown" | "C-f" => self.y = self.y.saturating_add(24 * n),
            "C-u" => self.y = self.y.saturating_sub(12 * n),
            "C-d" => self.y = self.y.saturating_add(12 * n),
            "g" => self.y = 0,
            "G" => self.y = self.lines.len().saturating_sub(1),
            "0" | "Home" => self.x = 0,
            "^" => {
                self.x = self
                    .lines
                    .get(self.y)
                    .map(|l| l.chars().take_while(|c| c.is_whitespace()).count())
                    .unwrap_or(0)
            }
            "$" | "End" => self.x = self.line_len().saturating_sub(1),
            "v" | " " | "Space" => self.anchor = Some((self.x, self.y)),
            "V" => {
                self.anchor = Some((0, self.y));
                self.x = self.line_len().saturating_sub(1);
            }
            "C-v" => self.rect = !self.rect,
            "w" | "W" | "C-Right" => {
                for _ in 0..n {
                    self.word(true, false);
                }
            }
            "b" | "B" | "C-Left" => {
                for _ in 0..n {
                    self.word(false, false);
                }
            }
            "e" => {
                for _ in 0..n {
                    self.word(true, true);
                }
            }
            _ => return Err(format!("unsupported copy key: {key}")),
        }
        self.y = self.y.min(self.lines.len().saturating_sub(1));
        self.x = self.x.min(self.line_len().saturating_sub(1));
        Ok(None)
    }
    fn line_len(&self) -> usize {
        self.lines
            .get(self.y)
            .map(|l| l.chars().count())
            .unwrap_or(0)
    }
    fn word(&mut self, forward: bool, end: bool) {
        let chars: Vec<char> = self
            .lines
            .get(self.y)
            .map(|l| l.chars().collect())
            .unwrap_or_default();
        if chars.is_empty() {
            return;
        }
        if forward {
            let mut x = (self.x + 1).min(chars.len());
            if !end {
                while x < chars.len() && !chars[x].is_whitespace() {
                    x += 1;
                }
            }
            while x < chars.len() && chars[x].is_whitespace() {
                x += 1;
            }
            if end {
                while x + 1 < chars.len() && !chars[x + 1].is_whitespace() {
                    x += 1;
                }
            }
            self.x = x.min(chars.len() - 1);
        } else {
            let mut x = self.x.saturating_sub(1).min(chars.len() - 1);
            while x > 0 && chars[x].is_whitespace() {
                x -= 1;
            }
            while x > 0 && !chars[x - 1].is_whitespace() {
                x -= 1;
            }
            self.x = x;
        }
    }
    pub fn search(&mut self, query: &str, backwards: bool) -> Result<(), String> {
        if query.is_empty() {
            return Err("search text required".into());
        }
        for step in 1..=self.lines.len() {
            let y = if backwards {
                (self.y + self.lines.len() - step) % self.lines.len()
            } else {
                (self.y + step) % self.lines.len()
            };
            if let Some(x) = self.lines[y].find(query) {
                self.y = y;
                self.x = self.lines[y][..x].chars().count();
                return Ok(());
            }
        }
        Err("no copy search match".into())
    }
    fn selected(&self) -> String {
        let Some((ax, ay)) = self.anchor else {
            return String::new();
        };
        let ((y0, x0), (y1, x1)) = if (ay, ax) <= (self.y, self.x) {
            ((ay, ax), (self.y, self.x))
        } else {
            ((self.y, self.x), (ay, ax))
        };
        (y0..=y1)
            .filter_map(|y| {
                self.lines.get(y).map(|l| {
                    let start = if self.rect {
                        ax.min(self.x)
                    } else if y == y0 {
                        x0
                    } else {
                        0
                    };
                    let end = if self.rect {
                        ax.max(self.x) + 1
                    } else if y == y1 {
                        x1 + 1
                    } else {
                        l.chars().count()
                    };
                    l.chars()
                        .skip(start)
                        .take(end.saturating_sub(start))
                        .collect::<String>()
                })
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    pub fn screen(&self) -> Screen {
        let mut screen = Screen::new(80, 24);
        let selection = self.anchor.map(|(x, y)| CopySelection {
            rect: self.rect,
            x0: x,
            y0: y as isize,
            x1: self.x,
            y1: self.y as isize,
        });
        screen.load_mode_snapshot(&self.lines.join("\n"), self.x, self.y, selection);
        screen
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reversed_selection_and_unicode_yank() {
        let mut c = CopyView::new("one\nλtwo");
        c.key("$").unwrap();
        c.key("v").unwrap();
        c.key("0").unwrap();
        assert_eq!(c.key("y").unwrap(), Some("λtwo".into()));
    }
    #[test]
    fn frozen_search_and_paging() {
        let text = (0..80)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut c = CopyView::new(&text);
        c.key("ScrollUp").unwrap();
        assert_eq!(c.y, 55);
        c.search("line 10", true).unwrap();
        assert_eq!(c.y, 10);
        assert!(c.key("unsupported").is_err());
    }
}
