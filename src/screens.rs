//! Port of pyte `pyte/screens.py`.
//!
//! In-memory matrix of styled characters representing a terminal display. This
//! is a faithful port of pyte's `Screen` (plus `HistoryScreen`, folded in as an
//! optional `history` component rather than a subclass — pyte's own docstring
//! notes the subclass split "is not obvious how to do" and would be nicer as a
//! mixin). Every handler maps 1:1 to the Python method of the same name.
//!
//! Coordinates and counts are `i64` to match Python integer arithmetic, which
//! transiently goes negative before [`Screen::ensure_hbounds`] /
//! [`Screen::ensure_vbounds`] clamp it.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::charsets as cs;
use crate::graphics as g;
use crate::modes as mo;
use crate::streams::Listener;
use crate::wcwidth::{combining, wcwidth};

/// A single styled on-screen character. Mirrors `pyte.screens.Char`.
///
/// Invariant on `data`: normally one Unicode scalar wide; `""` marks the stub
/// slot after a full-width character; combining marks are folded into the
/// preceding cell via NFC normalization, so it can hold a short grapheme.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Char {
    pub data: String,
    pub fg: String,
    pub bg: String,
    pub bold: bool,
    pub italics: bool,
    pub underscore: bool,
    pub strikethrough: bool,
    pub reverse: bool,
    pub blink: bool,
}

impl Char {
    /// Construct a `Char` with pyte's field defaults (`fg`/`bg` = `"default"`,
    /// all flags off).
    pub fn new(data: impl Into<String>) -> Self {
        Char {
            data: data.into(),
            fg: "default".to_string(),
            bg: "default".to_string(),
            bold: false,
            italics: false,
            underscore: false,
            strikethrough: false,
            reverse: false,
            blink: false,
        }
    }
}

/// Screen cursor. Mirrors `pyte.screens.Cursor`.
#[derive(Clone, Debug, Serialize)]
pub struct Cursor {
    pub x: i64,
    pub y: i64,
    pub attrs: Char,
    pub hidden: bool,
}

impl Cursor {
    fn new(x: i64, y: i64) -> Self {
        Cursor {
            x,
            y,
            attrs: Char::new(" "),
            hidden: false,
        }
    }
}

/// Scroll margins (`top`, `bottom`). Mirrors `pyte.screens.Margins`.
#[derive(Clone, Copy, Debug)]
struct Margins {
    top: i64,
    bottom: i64,
}

/// Saved cursor state for `DECSC` / `DECRC`. Mirrors `pyte.screens.Savepoint`.
#[derive(Clone)]
struct Savepoint {
    cursor: Cursor,
    g0_charset: &'static [u32; 256],
    g1_charset: &'static [u32; 256],
    charset: i64,
    origin: bool,
    wrap: bool,
}

/// A sparse screen line: a map of column → [`Char`] with a static default,
/// mirroring pyte's `StaticDefaultDict`. Reading a missing column returns the
/// default *without* storing it.
#[derive(Clone)]
struct Line {
    cells: HashMap<i64, Char>,
    default: Char,
}

impl Line {
    fn new(default: Char) -> Self {
        Line {
            cells: HashMap::new(),
            default,
        }
    }

    fn get(&self, x: i64) -> Char {
        self.cells
            .get(&x)
            .cloned()
            .unwrap_or_else(|| self.default.clone())
    }

    fn set(&mut self, x: i64, c: Char) {
        self.cells.insert(x, c);
    }

    fn pop(&mut self, x: i64) -> Option<Char> {
        self.cells.remove(&x)
    }
}

/// Scrollback history for a screen, mirroring `pyte.screens.History`. Folded
/// into [`Screen`] as an optional component.
struct History {
    top: VecDeque<Line>,
    bottom: VecDeque<Line>,
    ratio: f64,
    size: usize,
    position: usize,
    maxlen: usize,
}

impl History {
    fn push_top(&mut self, line: Line) {
        self.top.push_back(line);
        while self.top.len() > self.maxlen {
            self.top.pop_front();
        }
    }
    fn push_bottom_back(&mut self, line: Line) {
        self.bottom.push_back(line);
        while self.bottom.len() > self.maxlen {
            self.bottom.pop_front();
        }
    }
    fn push_bottom_front(&mut self, line: Line) {
        self.bottom.push_front(line);
        while self.bottom.len() > self.maxlen {
            self.bottom.pop_back();
        }
    }
}

/// A terminal screen: an in-memory matrix of characters plus cursor, modes,
/// margins, tab stops, charsets, and optional scrollback. Faithful port of
/// `pyte.screens.Screen` (+ `HistoryScreen`).
pub struct Screen {
    savepoints: Vec<Savepoint>,
    pub columns: i64,
    pub lines: i64,
    buffer: HashMap<i64, Line>,
    pub dirty: HashSet<i64>,
    pub mode: HashSet<u32>,
    margins: Option<Margins>,
    pub title: String,
    pub icon_name: String,
    charset: i64,
    g0_charset: &'static [u32; 256],
    g1_charset: &'static [u32; 256],
    tabstops: HashSet<i64>,
    pub cursor: Cursor,
    saved_columns: Option<i64>,
    /// Captured [`Screen::write_process_input`] output (device reports). Drained
    /// by the FFI layer so a script can forward replies back to the child.
    pub outgoing: String,
    history: Option<History>,
}

/// Default modes on power-up / reset: `DECAWM` (auto-wrap) + `DECTCEM` (cursor
/// visible). Mirrors `pyte.screens._DEFAULT_MODE`.
fn default_mode() -> HashSet<u32> {
    let mut m = HashSet::new();
    m.insert(mo::DECAWM);
    m.insert(mo::DECTCEM);
    m
}

impl Screen {
    /// Create a plain screen of `columns` × `lines`.
    pub fn new(columns: i64, lines: i64) -> Self {
        Self::build(columns, lines, None)
    }

    /// Create a screen with scrollback `history` lines (split top/bottom) and a
    /// paging `ratio`. Mirrors `HistoryScreen.__init__`.
    pub fn with_history(columns: i64, lines: i64, history: usize, ratio: f64) -> Self {
        let hist = History {
            top: VecDeque::new(),
            bottom: VecDeque::new(),
            ratio,
            size: history,
            position: history,
            maxlen: history,
        };
        Self::build(columns, lines, Some(hist))
    }

    fn build(columns: i64, lines: i64, history: Option<History>) -> Self {
        let mut s = Screen {
            savepoints: Vec::new(),
            columns,
            lines,
            buffer: HashMap::new(),
            dirty: HashSet::new(),
            mode: default_mode(),
            margins: None,
            title: String::new(),
            icon_name: String::new(),
            charset: 0,
            g0_charset: &cs::LAT1_MAP,
            g1_charset: &cs::VT100_MAP,
            tabstops: HashSet::new(),
            cursor: Cursor::new(0, 0),
            saved_columns: None,
            outgoing: String::new(),
            history,
        };
        s.reset();
        s.mode = default_mode();
        s.margins = None;
        s
    }

    /// Whether this screen keeps scrollback history.
    pub fn has_history(&self) -> bool {
        self.history.is_some()
    }

    /// An empty character with default colors; `reverse` reflects `DECSCNM`.
    /// Mirrors `Screen.default_char`.
    fn default_char(&self) -> Char {
        let reverse = self.mode.contains(&mo::DECSCNM);
        let mut c = Char::new(" ");
        c.reverse = reverse;
        c
    }

    fn margins_or_default(&self) -> Margins {
        self.margins.unwrap_or(Margins {
            top: 0,
            bottom: self.lines - 1,
        })
    }

    fn line_mut(&mut self, y: i64) -> &mut Line {
        let d = self.default_char();
        self.buffer.entry(y).or_insert_with(|| Line::new(d))
    }

    /// The character at `(x, y)`, returning the default for empty cells.
    pub fn cell(&self, x: i64, y: i64) -> Char {
        match self.buffer.get(&y) {
            Some(l) => l.get(x),
            None => self.default_char(),
        }
    }

    /// Screen contents as one string per line. Mirrors `Screen.display`.
    pub fn display(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.lines as usize);
        for y in 0..self.lines {
            let mut line = String::new();
            let mut is_wide_char = false;
            for x in 0..self.columns {
                if is_wide_char {
                    is_wide_char = false;
                    continue;
                }
                let ch = self.cell(x, y).data;
                let first = ch.chars().next().unwrap_or(' ');
                is_wide_char = wcwidth(first) == 2;
                if ch.is_empty() {
                    // Stub slot after a wide char: render nothing (the wide
                    // glyph already occupied this visual column). pyte yields
                    // the empty string here.
                } else {
                    line.push_str(&ch);
                }
            }
            out.push(line);
        }
        out
    }

    // ── reset / resize / margins ────────────────────────────────────────

    /// Reset the terminal to its initial state. Mirrors `Screen.reset`.
    pub fn reset(&mut self) {
        self.dirty.extend(0..self.lines);
        self.buffer.clear();
        self.margins = None;
        self.mode = default_mode();
        self.title.clear();
        self.icon_name.clear();
        self.charset = 0;
        self.g0_charset = &cs::LAT1_MAP;
        self.g1_charset = &cs::VT100_MAP;
        self.tabstops = (8..self.columns).step_by(8).collect();
        self.cursor = Cursor::new(0, 0);
        self.cursor_position(None, None);
        self.saved_columns = None;
        if self.history.is_some() {
            self.reset_history();
        }
    }

    /// Resize the screen. Mirrors `Screen.resize`.
    pub fn resize(&mut self, lines: Option<i64>, columns: Option<i64>) {
        let lines = lines.filter(|&l| l != 0).unwrap_or(self.lines);
        let columns = columns.filter(|&c| c != 0).unwrap_or(self.columns);

        if lines == self.lines && columns == self.columns {
            return;
        }

        self.dirty.extend(0..lines);

        if lines < self.lines {
            self.save_cursor();
            self.cursor_position(Some(0), Some(0));
            self.delete_lines(Some(self.lines - lines));
            self.restore_cursor();
        }

        if columns < self.columns {
            let old_columns = self.columns;
            for line in self.buffer.values_mut() {
                for x in columns..old_columns {
                    line.pop(x);
                }
            }
        }

        self.lines = lines;
        self.columns = columns;
        self.set_margins(None, None);
    }

    /// Select top and bottom scrolling margins (DECSTBM). Mirrors
    /// `Screen.set_margins`.
    pub fn set_margins(&mut self, top: Option<i64>, bottom: Option<i64>) {
        // 0 corresponds to CSI with no parameters.
        if (top.is_none() || top == Some(0)) && bottom.is_none() {
            self.margins = None;
            return;
        }

        let base = self.margins.unwrap_or(Margins {
            top: 0,
            bottom: self.lines - 1,
        });

        let top = match top {
            None => base.top,
            Some(t) => 0.max((t - 1).min(self.lines - 1)),
        };
        let bottom = match bottom {
            None => base.bottom,
            Some(b) => 0.max((b - 1).min(self.lines - 1)),
        };

        if bottom - top >= 1 {
            self.margins = Some(Margins { top, bottom });
            self.cursor_position(None, None);
        }
    }

    // ── modes ───────────────────────────────────────────────────────────

    /// Set (enable) modes. Mirrors `Screen.set_mode`.
    pub fn set_mode(&mut self, modes: &[i64], private: bool) {
        let mode_list: Vec<u32> = if private {
            modes.iter().map(|m| (*m as u32) << 5).collect()
        } else {
            modes.iter().map(|m| *m as u32).collect()
        };
        if private && mode_list.contains(&mo::DECSCNM) {
            self.dirty.extend(0..self.lines);
        }

        self.mode.extend(mode_list.iter().copied());

        if mode_list.contains(&mo::DECCOLM) {
            self.saved_columns = Some(self.columns);
            self.resize(None, Some(132));
            self.erase_in_display(Some(2), false);
            self.cursor_position(None, None);
        }

        if mode_list.contains(&mo::DECOM) {
            self.cursor_position(None, None);
        }

        if mode_list.contains(&mo::DECSCNM) {
            let dc = self.default_char();
            for line in self.buffer.values_mut() {
                line.default = dc.clone();
                let keys: Vec<i64> = line.cells.keys().copied().collect();
                for x in keys {
                    let mut c = line.get(x);
                    c.reverse = true;
                    line.set(x, c);
                }
            }
            self.select_graphic_rendition(&[7]); // +reverse.
        }

        if mode_list.contains(&mo::DECTCEM) {
            self.cursor.hidden = false;
        }
    }

    /// Reset (disable) modes. Mirrors `Screen.reset_mode`.
    pub fn reset_mode(&mut self, modes: &[i64], private: bool) {
        let mode_list: Vec<u32> = if private {
            modes.iter().map(|m| (*m as u32) << 5).collect()
        } else {
            modes.iter().map(|m| *m as u32).collect()
        };
        if private && mode_list.contains(&mo::DECSCNM) {
            self.dirty.extend(0..self.lines);
        }

        for m in &mode_list {
            self.mode.remove(m);
        }

        if mode_list.contains(&mo::DECCOLM) {
            if self.columns == 132 && self.saved_columns.is_some() {
                self.resize(None, self.saved_columns);
                self.saved_columns = None;
            }
            self.erase_in_display(Some(2), false);
            self.cursor_position(None, None);
        }

        if mode_list.contains(&mo::DECOM) {
            self.cursor_position(None, None);
        }

        if mode_list.contains(&mo::DECSCNM) {
            let dc = self.default_char();
            for line in self.buffer.values_mut() {
                line.default = dc.clone();
                let keys: Vec<i64> = line.cells.keys().copied().collect();
                for x in keys {
                    let mut c = line.get(x);
                    c.reverse = false;
                    line.set(x, c);
                }
            }
            self.select_graphic_rendition(&[27]); // -reverse.
        }

        if mode_list.contains(&mo::DECTCEM) {
            self.cursor.hidden = true;
        }
    }

    // ── charsets ────────────────────────────────────────────────────────

    /// Define `G0`/`G1` charset. Mirrors `Screen.define_charset`.
    pub fn define_charset(&mut self, code: char, mode: char) {
        if let Some(map) = cs::maps(code) {
            match mode {
                '(' => self.g0_charset = map,
                ')' => self.g1_charset = map,
                _ => {}
            }
        }
    }

    /// Select `G0` charset (SI). Mirrors `Screen.shift_in`.
    pub fn shift_in(&mut self) {
        self.charset = 0;
    }

    /// Select `G1` charset (SO). Mirrors `Screen.shift_out`.
    pub fn shift_out(&mut self) {
        self.charset = 1;
    }

    // ── drawing ─────────────────────────────────────────────────────────

    /// Display characters at the cursor, advancing per `DECAWM`. Mirrors
    /// `Screen.draw`.
    pub fn draw(&mut self, data: &str) {
        let map = if self.charset != 0 {
            self.g1_charset
        } else {
            self.g0_charset
        };

        for raw in data.chars() {
            let ch = cs::translate(map, raw);
            let char_width = wcwidth(ch);

            if self.cursor.x == self.columns {
                if self.mode.contains(&mo::DECAWM) {
                    self.dirty.insert(self.cursor.y);
                    self.carriage_return();
                    self.linefeed();
                } else if char_width > 0 {
                    self.cursor.x -= char_width as i64;
                }
            }

            if self.mode.contains(&mo::IRM) && char_width > 0 {
                self.insert_characters(Some(char_width as i64));
            }

            let cx = self.cursor.x;
            let cy = self.cursor.y;
            let attrs = self.cursor.attrs.clone();

            if char_width == 1 {
                let mut c = attrs;
                c.data = ch.to_string();
                self.line_mut(cy).set(cx, c);
            } else if char_width == 2 {
                let mut c = attrs.clone();
                c.data = ch.to_string();
                self.line_mut(cy).set(cx, c);
                if cx + 1 < self.columns {
                    let mut stub = attrs;
                    stub.data = String::new();
                    self.line_mut(cy).set(cx + 1, stub);
                }
            } else if char_width == 0 && combining(ch) {
                // Combine with the previous character on this or the prior line.
                if cx != 0 {
                    let last = self.line_mut(cy).get(cx - 1);
                    let mut normalized = last.clone();
                    normalized.data = nfc(&format!("{}{}", last.data, ch));
                    self.line_mut(cy).set(cx - 1, normalized);
                } else if cy != 0 {
                    let cols = self.columns;
                    let last = self.line_mut(cy - 1).get(cols - 1);
                    let mut normalized = last.clone();
                    normalized.data = nfc(&format!("{}{}", last.data, ch));
                    self.line_mut(cy - 1).set(cols - 1, normalized);
                }
            } else {
                break; // Unprintable / doesn't advance the cursor.
            }

            if char_width > 0 {
                self.cursor.x = (self.cursor.x + char_width as i64).min(self.columns);
            }
        }

        self.dirty.insert(self.cursor.y);
    }

    // ── titles ──────────────────────────────────────────────────────────

    /// Set terminal title (XTerm/linux). Mirrors `Screen.set_title`.
    pub fn set_title(&mut self, param: &str) {
        self.title = param.to_string();
    }

    /// Set icon name (XTerm/linux). Mirrors `Screen.set_icon_name`.
    pub fn set_icon_name(&mut self, param: &str) {
        self.icon_name = param.to_string();
    }

    // ── cursor movement / scrolling ─────────────────────────────────────

    /// Move the cursor to column 0. Mirrors `Screen.carriage_return`.
    pub fn carriage_return(&mut self) {
        self.cursor.x = 0;
    }

    /// Move down one line, scrolling at the bottom margin. Mirrors
    /// `Screen.index`.
    pub fn index(&mut self) {
        let Margins { top, bottom } = self.margins_or_default();

        // HistoryScreen.index: push the scrolled-off top line to history.
        if self.history.is_some() && self.cursor.y == bottom {
            let line = self.line_mut(top).clone();
            if let Some(h) = self.history.as_mut() {
                h.push_top(line);
            }
        }

        if self.cursor.y == bottom {
            self.dirty.extend(0..self.lines);
            for y in top..bottom {
                if let Some(l) = self.buffer.remove(&(y + 1)) {
                    self.buffer.insert(y, l);
                } else {
                    self.buffer.remove(&y);
                }
            }
            self.buffer.remove(&bottom);
        } else {
            self.cursor_down(None);
        }
    }

    /// Move up one line, scrolling at the top margin. Mirrors
    /// `Screen.reverse_index`.
    pub fn reverse_index(&mut self) {
        let Margins { top, bottom } = self.margins_or_default();

        // HistoryScreen.reverse_index: push the scrolled-off bottom line.
        if self.history.is_some() && self.cursor.y == top {
            let line = self.line_mut(bottom).clone();
            if let Some(h) = self.history.as_mut() {
                h.push_bottom_back(line);
            }
        }

        if self.cursor.y == top {
            self.dirty.extend(0..self.lines);
            for y in (top + 1..=bottom).rev() {
                if let Some(l) = self.buffer.remove(&(y - 1)) {
                    self.buffer.insert(y, l);
                } else {
                    self.buffer.remove(&y);
                }
            }
            self.buffer.remove(&top);
        } else {
            self.cursor_up(None);
        }
    }

    /// Index + optional carriage return (`LNM`). Mirrors `Screen.linefeed`.
    pub fn linefeed(&mut self) {
        self.index();
        if self.mode.contains(&mo::LNM) {
            self.carriage_return();
        }
    }

    /// Move to the next tab stop. Mirrors `Screen.tab`.
    pub fn tab(&mut self) {
        let mut stops: Vec<i64> = self.tabstops.iter().copied().collect();
        stops.sort_unstable();
        let mut column = self.columns - 1;
        for stop in stops {
            if self.cursor.x < stop {
                column = stop;
                break;
            }
        }
        self.cursor.x = column;
    }

    /// Backspace one column. Mirrors `Screen.backspace`.
    pub fn backspace(&mut self) {
        self.cursor_back(None);
    }

    /// Push cursor state (DECSC). Mirrors `Screen.save_cursor`.
    pub fn save_cursor(&mut self) {
        self.savepoints.push(Savepoint {
            cursor: self.cursor.clone(),
            g0_charset: self.g0_charset,
            g1_charset: self.g1_charset,
            charset: self.charset,
            origin: self.mode.contains(&mo::DECOM),
            wrap: self.mode.contains(&mo::DECAWM),
        });
    }

    /// Pop cursor state (DECRC). Mirrors `Screen.restore_cursor`.
    pub fn restore_cursor(&mut self) {
        if let Some(sp) = self.savepoints.pop() {
            self.g0_charset = sp.g0_charset;
            self.g1_charset = sp.g1_charset;
            self.charset = sp.charset;
            if sp.origin {
                self.set_mode(&[(mo::DECOM >> 5) as i64], true);
            }
            if sp.wrap {
                self.set_mode(&[(mo::DECAWM >> 5) as i64], true);
            }
            self.cursor = sp.cursor;
            self.ensure_hbounds();
            self.ensure_vbounds(Some(true));
        } else {
            self.reset_mode(&[(mo::DECOM >> 5) as i64], true);
            self.cursor_position(None, None);
        }
    }

    /// Insert blank lines at the cursor. Mirrors `Screen.insert_lines`.
    pub fn insert_lines(&mut self, count: Option<i64>) {
        let count = count.filter(|&c| c != 0).unwrap_or(1);
        let Margins { top, bottom } = self.margins_or_default();

        if top <= self.cursor.y && self.cursor.y <= bottom {
            self.dirty.extend(self.cursor.y..self.lines);
            for y in (self.cursor.y..=bottom).rev() {
                if y + count <= bottom && self.buffer.contains_key(&y) {
                    if let Some(l) = self.buffer.remove(&y) {
                        self.buffer.insert(y + count, l);
                    }
                } else {
                    self.buffer.remove(&y);
                }
            }
            self.carriage_return();
        }
    }

    /// Delete lines at the cursor. Mirrors `Screen.delete_lines`.
    pub fn delete_lines(&mut self, count: Option<i64>) {
        let count = count.filter(|&c| c != 0).unwrap_or(1);
        let Margins { top, bottom } = self.margins_or_default();

        if top <= self.cursor.y && self.cursor.y <= bottom {
            self.dirty.extend(self.cursor.y..self.lines);
            for y in self.cursor.y..=bottom {
                if y + count <= bottom {
                    if let Some(l) = self.buffer.remove(&(y + count)) {
                        self.buffer.insert(y, l);
                    }
                } else {
                    self.buffer.remove(&y);
                }
            }
            self.carriage_return();
        }
    }

    /// Insert blank characters at the cursor. Mirrors `Screen.insert_characters`.
    pub fn insert_characters(&mut self, count: Option<i64>) {
        self.dirty.insert(self.cursor.y);
        let count = count.filter(|&c| c != 0).unwrap_or(1);
        let cx = self.cursor.x;
        let columns = self.columns;
        let line = self.line_mut(self.cursor.y);
        for x in (cx..=columns).rev() {
            if x + count <= columns {
                let c = line.get(x);
                line.set(x + count, c);
            }
            line.pop(x);
        }
    }

    /// Delete characters at the cursor. Mirrors `Screen.delete_characters`.
    pub fn delete_characters(&mut self, count: Option<i64>) {
        self.dirty.insert(self.cursor.y);
        let count = count.filter(|&c| c != 0).unwrap_or(1);
        let cx = self.cursor.x;
        let columns = self.columns;
        let dc = self.default_char();
        let line = self.line_mut(self.cursor.y);
        for x in cx..columns {
            if x + count <= columns {
                let c = line.pop(x + count).unwrap_or_else(|| dc.clone());
                line.set(x, c);
            } else {
                line.pop(x);
            }
        }
    }

    /// Erase characters at the cursor using cursor attrs. Mirrors
    /// `Screen.erase_characters`.
    pub fn erase_characters(&mut self, count: Option<i64>) {
        self.dirty.insert(self.cursor.y);
        let count = count.filter(|&c| c != 0).unwrap_or(1);
        let cx = self.cursor.x;
        let end = (self.cursor.x + count).min(self.columns);
        let attrs = self.cursor.attrs.clone();
        let line = self.line_mut(self.cursor.y);
        for x in cx..end {
            line.set(x, attrs.clone());
        }
    }

    /// Erase within the current line. Mirrors `Screen.erase_in_line`.
    pub fn erase_in_line(&mut self, how: i64) {
        self.dirty.insert(self.cursor.y);
        let (start, end) = match how {
            0 => (self.cursor.x, self.columns),
            1 => (0, self.cursor.x + 1),
            2 => (0, self.columns),
            _ => return,
        };
        let attrs = self.cursor.attrs.clone();
        let line = self.line_mut(self.cursor.y);
        for x in start..end {
            line.set(x, attrs.clone());
        }
    }

    /// Erase within the display. Mirrors `Screen.erase_in_display`.
    pub fn erase_in_display(&mut self, how: Option<i64>, _private: bool) {
        let how = how.unwrap_or(0);
        let interval: Vec<i64> = match how {
            0 => (self.cursor.y + 1..self.lines).collect(),
            1 => (0..self.cursor.y).collect(),
            2 | 3 => (0..self.lines).collect(),
            _ => Vec::new(),
        };

        self.dirty.extend(interval.iter().copied());
        let attrs = self.cursor.attrs.clone();
        for y in &interval {
            let line = self.line_mut(*y);
            let keys: Vec<i64> = line.cells.keys().copied().collect();
            for x in keys {
                line.set(x, attrs.clone());
            }
        }

        if how == 0 || how == 1 {
            self.erase_in_line(how);
        }

        // HistoryScreen.erase_in_display: how == 3 resets history.
        if how == 3 && self.history.is_some() {
            self.reset_history();
        }
    }

    /// Set a tab stop at the cursor. Mirrors `Screen.set_tab_stop`.
    pub fn set_tab_stop(&mut self) {
        self.tabstops.insert(self.cursor.x);
    }

    /// Clear tab stop(s). Mirrors `Screen.clear_tab_stop`.
    pub fn clear_tab_stop(&mut self, how: i64) {
        if how == 0 {
            self.tabstops.remove(&self.cursor.x);
        } else if how == 3 {
            self.tabstops.clear();
        }
    }

    /// Clamp the cursor within horizontal bounds. Mirrors `Screen.ensure_hbounds`.
    pub fn ensure_hbounds(&mut self) {
        self.cursor.x = 0.max(self.cursor.x).min(self.columns - 1);
    }

    /// Clamp the cursor within vertical bounds. Mirrors `Screen.ensure_vbounds`.
    pub fn ensure_vbounds(&mut self, use_margins: Option<bool>) {
        let (top, bottom) = match self.margins {
            Some(m) if use_margins == Some(true) || self.mode.contains(&mo::DECOM) => {
                (m.top, m.bottom)
            }
            _ => (0, self.lines - 1),
        };
        self.cursor.y = top.max(self.cursor.y).min(bottom);
    }

    /// Move cursor up. Mirrors `Screen.cursor_up`.
    pub fn cursor_up(&mut self, count: Option<i64>) {
        let top = self.margins_or_default().top;
        self.cursor.y = (self.cursor.y - count.filter(|&c| c != 0).unwrap_or(1)).max(top);
    }

    /// Move cursor up to column 1. Mirrors `Screen.cursor_up1`.
    pub fn cursor_up1(&mut self, count: Option<i64>) {
        self.cursor_up(count);
        self.carriage_return();
    }

    /// Move cursor down. Mirrors `Screen.cursor_down`.
    pub fn cursor_down(&mut self, count: Option<i64>) {
        let bottom = self.margins_or_default().bottom;
        self.cursor.y = (self.cursor.y + count.filter(|&c| c != 0).unwrap_or(1)).min(bottom);
    }

    /// Move cursor down to column 1. Mirrors `Screen.cursor_down1`.
    pub fn cursor_down1(&mut self, count: Option<i64>) {
        self.cursor_down(count);
        self.carriage_return();
    }

    /// Move cursor left. Mirrors `Screen.cursor_back`.
    pub fn cursor_back(&mut self, count: Option<i64>) {
        if self.cursor.x == self.columns {
            self.cursor.x -= 1;
        }
        self.cursor.x -= count.filter(|&c| c != 0).unwrap_or(1);
        self.ensure_hbounds();
    }

    /// Move cursor right. Mirrors `Screen.cursor_forward`.
    pub fn cursor_forward(&mut self, count: Option<i64>) {
        self.cursor.x += count.filter(|&c| c != 0).unwrap_or(1);
        self.ensure_hbounds();
    }

    /// Move cursor to `(line, column)`. Mirrors `Screen.cursor_position`.
    pub fn cursor_position(&mut self, line: Option<i64>, column: Option<i64>) {
        let column = column.filter(|&c| c != 0).unwrap_or(1) - 1;
        let mut line = line.filter(|&l| l != 0).unwrap_or(1) - 1;

        if let Some(m) = self.margins {
            if self.mode.contains(&mo::DECOM) {
                line += m.top;
                if !(m.top <= line && line <= m.bottom) {
                    return;
                }
            }
        }

        self.cursor.x = column;
        self.cursor.y = line;
        self.ensure_hbounds();
        self.ensure_vbounds(None);
    }

    /// Move cursor to a column. Mirrors `Screen.cursor_to_column`.
    pub fn cursor_to_column(&mut self, column: Option<i64>) {
        self.cursor.x = column.filter(|&c| c != 0).unwrap_or(1) - 1;
        self.ensure_hbounds();
    }

    /// Move cursor to a line. Mirrors `Screen.cursor_to_line`.
    pub fn cursor_to_line(&mut self, line: Option<i64>) {
        self.cursor.y = line.filter(|&l| l != 0).unwrap_or(1) - 1;
        if self.mode.contains(&mo::DECOM) {
            if let Some(m) = self.margins {
                self.cursor.y += m.top;
            }
        }
        self.ensure_vbounds(None);
    }

    /// Bell stub. Mirrors `Screen.bell`.
    pub fn bell(&mut self) {}

    /// Fill screen with `E`s (DECALN). Mirrors `Screen.alignment_display`.
    pub fn alignment_display(&mut self) {
        self.dirty.extend(0..self.lines);
        for y in 0..self.lines {
            for x in 0..self.columns {
                let mut c = self.cell(x, y);
                c.data = "E".to_string();
                self.line_mut(y).set(x, c);
            }
        }
    }

    /// Set display attributes (SGR). Mirrors `Screen.select_graphic_rendition`.
    pub fn select_graphic_rendition(&mut self, attrs: &[i64]) {
        // Fast path: reset everything.
        if attrs.is_empty() || attrs == [0] {
            self.cursor.attrs = self.default_char();
            return;
        }

        let mut work = self.cursor.attrs.clone();
        let mut i = 0usize;
        while i < attrs.len() {
            let attr = attrs[i];
            i += 1;
            if attr == 0 {
                work = self.default_char();
            } else if let Some(name) = g::fg_ansi(attr) {
                work.fg = name.to_string();
            } else if let Some(name) = g::bg_ansi(attr) {
                work.bg = name.to_string();
            } else if let Some(t) = g::text(attr) {
                let set = t.starts_with('+');
                apply_text_flag(&mut work, &t[1..], set);
            } else if let Some(name) = g::fg_aixterm(attr) {
                work.fg = name.to_string();
            } else if let Some(name) = g::bg_aixterm(attr) {
                work.bg = name.to_string();
            } else if attr == g::FG_256 || attr == g::BG_256 {
                let is_fg = attr == g::FG_256;
                if i >= attrs.len() {
                    continue;
                }
                let n = attrs[i];
                i += 1;
                if n == 5 {
                    if i >= attrs.len() {
                        continue;
                    }
                    let m = attrs[i];
                    i += 1;
                    if let Some(hex) = g::FG_BG_256.get(m as usize) {
                        if is_fg {
                            work.fg = hex.clone();
                        } else {
                            work.bg = hex.clone();
                        }
                    }
                } else if n == 2 {
                    if i + 2 >= attrs.len() {
                        continue;
                    }
                    let r = attrs[i];
                    let gg = attrs[i + 1];
                    let b = attrs[i + 2];
                    i += 3;
                    let hex = format!("{:02x}{:02x}{:02x}", r, gg, b);
                    if is_fg {
                        work.fg = hex;
                    } else {
                        work.bg = hex;
                    }
                }
            }
        }

        self.cursor.attrs = work;
    }

    /// Report terminal identity (primary DA). Mirrors
    /// `Screen.report_device_attributes`.
    pub fn report_device_attributes(&mut self, mode: i64, private: bool) {
        if mode == 0 && !private {
            let msg = format!("{}?6c", crate::control::CSI);
            self.write_process_input(&msg);
        }
    }

    /// Report terminal status or cursor position. Mirrors
    /// `Screen.report_device_status`.
    pub fn report_device_status(&mut self, mode: i64) {
        if mode == 5 {
            let msg = format!("{}0n", crate::control::CSI);
            self.write_process_input(&msg);
        } else if mode == 6 {
            let x = self.cursor.x + 1;
            let mut y = self.cursor.y + 1;
            if self.mode.contains(&mo::DECOM) {
                if let Some(m) = self.margins {
                    y -= m.top;
                }
            }
            let msg = format!("{}{};{}R", crate::control::CSI, y, x);
            self.write_process_input(&msg);
        }
    }

    /// Capture data destined for the child process (device reports). pyte's
    /// default is a no-op; we buffer it in [`Screen::outgoing`] so the FFI layer
    /// can hand it back to `pty_send`.
    pub fn write_process_input(&mut self, data: &str) {
        self.outgoing.push_str(data);
    }

    // ── history (HistoryScreen) ─────────────────────────────────────────

    fn reset_history(&mut self) {
        if let Some(h) = self.history.as_mut() {
            h.top.clear();
            h.bottom.clear();
            h.position = h.size;
        }
    }

    /// Ensure the screen is at the bottom of the history buffer before a
    /// non-paging event. Mirrors `HistoryScreen.before_event`.
    pub fn before_event(&mut self, event: &str) {
        if self.history.is_none() || event == "prev_page" || event == "next_page" {
            return;
        }
        loop {
            let (pos, size) = {
                let h = self.history.as_ref().unwrap();
                (h.position, h.size)
            };
            if pos < size {
                // pyte wraps every event method, so the internal next_page()
                // calls run their own after_event (which normalizes line widths
                // and cursor visibility). before_event("next_page") is a no-op.
                self.next_page();
                self.after_event("next_page");
            } else {
                break;
            }
        }
    }

    /// Run `f` as an event named `name`, wrapping it with history
    /// before/after hooks. Used by the FFI layer for direct screen commands so
    /// history screens behave the same whether driven by a stream or directly.
    pub fn event<F: FnOnce(&mut Screen)>(&mut self, name: &str, f: F) {
        self.before_event(name);
        f(self);
        self.after_event(name);
    }

    /// Normalize line widths + cursor visibility after an event. Mirrors
    /// `HistoryScreen.after_event`.
    pub fn after_event(&mut self, event: &str) {
        if self.history.is_none() {
            return;
        }
        if event == "prev_page" || event == "next_page" {
            let columns = self.columns;
            for line in self.buffer.values_mut() {
                let keys: Vec<i64> = line.cells.keys().copied().collect();
                for x in keys {
                    if x > columns {
                        line.pop(x);
                    }
                }
            }
        }
        let h = self.history.as_ref().unwrap();
        let at_bottom = h.position == h.size;
        self.cursor.hidden = !(at_bottom && self.mode.contains(&mo::DECTCEM));
    }

    /// Page up through history. Mirrors `HistoryScreen.prev_page`.
    pub fn prev_page(&mut self) {
        let (position, top_len, ratio) = match self.history.as_ref() {
            Some(h) => (h.position, h.top.len(), h.ratio),
            None => return,
        };
        if position > self.lines as usize && top_len > 0 {
            let mid = top_len.min((self.lines as f64 * ratio).ceil() as usize);

            for y in ((self.lines - mid as i64)..self.lines).rev() {
                let line = self.line_mut(y).clone();
                self.history.as_mut().unwrap().push_bottom_front(line);
            }
            {
                let h = self.history.as_mut().unwrap();
                h.position -= mid;
            }

            for y in (mid as i64..self.lines).rev() {
                if let Some(l) = self.buffer.remove(&(y - mid as i64)) {
                    self.buffer.insert(y, l);
                } else {
                    self.buffer.remove(&y);
                }
            }
            for y in (0..mid as i64).rev() {
                if let Some(l) = self.history.as_mut().unwrap().top.pop_back() {
                    self.buffer.insert(y, l);
                }
            }

            self.dirty = (0..self.lines).collect();
        }
    }

    /// Page down through history. Mirrors `HistoryScreen.next_page`.
    pub fn next_page(&mut self) {
        let (position, size, bottom_len, ratio) = match self.history.as_ref() {
            Some(h) => (h.position, h.size, h.bottom.len(), h.ratio),
            None => return,
        };
        if position < size && bottom_len > 0 {
            let mid = bottom_len.min((self.lines as f64 * ratio).ceil() as usize);

            for y in 0..mid as i64 {
                let line = self.line_mut(y).clone();
                self.history.as_mut().unwrap().push_top(line);
            }
            {
                let h = self.history.as_mut().unwrap();
                h.position += mid;
            }

            for y in 0..(self.lines - mid as i64) {
                if let Some(l) = self.buffer.remove(&(y + mid as i64)) {
                    self.buffer.insert(y, l);
                } else {
                    self.buffer.remove(&y);
                }
            }
            for y in (self.lines - mid as i64)..self.lines {
                if let Some(l) = self.history.as_mut().unwrap().bottom.pop_front() {
                    self.buffer.insert(y, l);
                }
            }

            self.dirty = (0..self.lines).collect();
        }
    }

    /// History state summary (position/size/queue lengths) for the FFI layer.
    pub fn history_state(&self) -> Option<(usize, usize, usize, usize)> {
        self.history
            .as_ref()
            .map(|h| (h.position, h.size, h.top.len(), h.bottom.len()))
    }

    /// Sorted list of dirty line numbers.
    pub fn dirty_sorted(&self) -> Vec<i64> {
        let mut v: Vec<i64> = self.dirty.iter().copied().collect();
        v.sort_unstable();
        v
    }

    /// Sorted list of active mode numbers.
    pub fn mode_sorted(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.mode.iter().copied().collect();
        v.sort_unstable();
        v
    }
}

/// Apply a `+field`/`-field` text attribute flag by name.
fn apply_text_flag(c: &mut Char, field: &str, set: bool) {
    match field {
        "bold" => c.bold = set,
        "italics" => c.italics = set,
        "underscore" => c.underscore = set,
        "blink" => c.blink = set,
        "reverse" => c.reverse = set,
        "strikethrough" => c.strikethrough = set,
        _ => {}
    }
}

/// NFC-normalize a short string, matching `unicodedata.normalize("NFC", ...)`.
fn nfc(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfc().collect()
}

// ── Listener impl: drive a Screen from a parsed stream ───────────────────

impl Listener for Screen {
    fn draw(&mut self, data: &str) {
        self.before_event("draw");
        Screen::draw(self, data);
        self.after_event("draw");
    }

    fn define_charset(&mut self, code: char, mode: char) {
        self.before_event("define_charset");
        Screen::define_charset(self, code, mode);
        self.after_event("define_charset");
    }

    fn set_icon_name(&mut self, name: &str) {
        self.before_event("set_icon_name");
        Screen::set_icon_name(self, name);
        self.after_event("set_icon_name");
    }

    fn set_title(&mut self, title: &str) {
        self.before_event("set_title");
        Screen::set_title(self, title);
        self.after_event("set_title");
    }

    fn dispatch(&mut self, name: &str, args: &[i64], private: bool) {
        self.before_event(name);
        self.dispatch_inner(name, args, private);
        self.after_event(name);
    }
}

impl Screen {
    fn dispatch_inner(&mut self, name: &str, args: &[i64], private: bool) {
        let a0 = args.first().copied();
        let a1 = args.get(1).copied();
        match name {
            "bell" => self.bell(),
            "backspace" => self.backspace(),
            "tab" => self.tab(),
            "linefeed" => self.linefeed(),
            "carriage_return" => self.carriage_return(),
            "shift_out" => self.shift_out(),
            "shift_in" => self.shift_in(),
            "reset" => self.reset(),
            "index" => self.index(),
            "reverse_index" => self.reverse_index(),
            "set_tab_stop" => self.set_tab_stop(),
            "save_cursor" => self.save_cursor(),
            "restore_cursor" => self.restore_cursor(),
            "alignment_display" => self.alignment_display(),
            "insert_characters" => self.insert_characters(a0),
            "cursor_up" => self.cursor_up(a0),
            "cursor_down" => self.cursor_down(a0),
            "cursor_forward" => self.cursor_forward(a0),
            "cursor_back" => self.cursor_back(a0),
            "cursor_down1" => self.cursor_down1(a0),
            "cursor_up1" => self.cursor_up1(a0),
            "cursor_to_column" => self.cursor_to_column(a0),
            "cursor_position" => self.cursor_position(a0, a1),
            "erase_in_display" => self.erase_in_display(a0, private),
            "erase_in_line" => self.erase_in_line(a0.unwrap_or(0)),
            "insert_lines" => self.insert_lines(a0),
            "delete_lines" => self.delete_lines(a0),
            "delete_characters" => self.delete_characters(a0),
            "erase_characters" => self.erase_characters(a0),
            "report_device_attributes" => self.report_device_attributes(a0.unwrap_or(0), private),
            "cursor_to_line" => self.cursor_to_line(a0),
            "clear_tab_stop" => self.clear_tab_stop(a0.unwrap_or(0)),
            "set_mode" => self.set_mode(args, private),
            "reset_mode" => self.reset_mode(args, private),
            "select_graphic_rendition" => self.select_graphic_rendition(args),
            "report_device_status" => self.report_device_status(a0.unwrap_or(0)),
            "set_margins" => self.set_margins(a0, a1),
            "prev_page" => self.prev_page(),
            "next_page" => self.next_page(),
            // "debug" and any unrecognized event are no-ops on a real screen.
            _ => {}
        }
    }
}
