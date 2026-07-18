//! Port of pyte `pyte/streams.py`.
//!
//! A [`Stream`] is a state machine that parses a stream of characters and
//! dispatches events to a [`Listener`]. pyte implements the parser as a Python
//! generator-coroutine (`_parser_fsm`); here it is reified into an explicit
//! [`State`] enum stepped one character at a time by [`Stream::advance`]. Every
//! branch corresponds 1:1 to a branch of the Python FSM.
//!
//! [`Stream`] also folds in pyte's `ByteStream`: [`Stream::feed_bytes`] decodes
//! bytes as UTF-8 (incrementally, with U+FFFD replacement) or Latin-1 depending
//! on `use_utf8`, then feeds the text through the same parser.

use crate::control as ctrl;
use crate::escape as esc;

/// Sink for events produced by [`Stream`]. [`crate::screens::Screen`] executes
/// them; a recorder can capture them for disassembly.
pub trait Listener {
    /// Plain text to display at the cursor.
    fn draw(&mut self, data: &str);
    /// A named control event (basic / escape / sharp / CSI) with integer
    /// parameters and the private (`?`) flag. Unknown ops arrive as `"debug"`.
    fn dispatch(&mut self, name: &str, args: &[i64], private: bool);
    /// `ESC ( c` / `ESC ) c` — define G0/G1 charset (only in non-UTF-8 mode).
    fn define_charset(&mut self, code: char, mode: char);
    /// OSC `1` — set icon name.
    fn set_icon_name(&mut self, name: &str);
    /// OSC `2` — set window title.
    fn set_title(&mut self, title: &str);
}

/// Parser state. Mirrors the control flow of pyte's `_parser_fsm` generator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Ground,
    Escape,
    EscapeSharp,
    EscapePercent,
    EscapeCharset,
    Csi,
    CsiDollar,
    OscCode,
    OscString,
}

/// A VTXXX escape/CSI/OSC parser. Faithful port of `pyte.streams.Stream`
/// (with `ByteStream` folded in).
pub struct Stream {
    state: State,
    // CSI accumulation.
    csi_params: Vec<i64>,
    csi_current: String,
    csi_private: bool,
    // ESC ( / ESC ) charset selection mode.
    esc_charset_mode: char,
    // OSC accumulation.
    osc_code: char,
    osc_param: String,
    osc_pending_esc: bool,
    // Byte decoding (ByteStream behavior).
    bytes: bool,
    use_utf8: bool,
    pending_bytes: Vec<u8>,
}

impl Default for Stream {
    fn default() -> Self {
        Self::new()
    }
}

impl Stream {
    /// A text stream (`feed_str`).
    pub fn new() -> Self {
        Stream {
            state: State::Ground,
            csi_params: Vec::new(),
            csi_current: String::new(),
            csi_private: false,
            esc_charset_mode: '(',
            osc_code: '\0',
            osc_param: String::new(),
            osc_pending_esc: false,
            bytes: false,
            use_utf8: true,
            pending_bytes: Vec::new(),
        }
    }

    /// A byte stream (`feed_bytes`), decoding UTF-8 with U+FFFD replacement by
    /// default. Mirrors `pyte.streams.ByteStream`.
    pub fn new_bytes() -> Self {
        let mut s = Self::new();
        s.bytes = true;
        s
    }

    /// Whether the parser is back at ground state (safe to take plain text).
    fn taking_plain_text(&self) -> bool {
        self.state == State::Ground
    }

    /// Feed text. Mirrors `Stream.feed`: plain-text runs are drawn in one call;
    /// special characters step the parser.
    pub fn feed_str(&mut self, listener: &mut dyn Listener, data: &str) {
        let chars: Vec<char> = data.chars().collect();
        let len = chars.len();
        let mut offset = 0;
        while offset < len {
            if self.taking_plain_text() {
                let start = offset;
                while offset < len && !is_special(chars[offset]) {
                    offset += 1;
                }
                if offset > start {
                    let chunk: String = chars[start..offset].iter().collect();
                    listener.draw(&chunk);
                } else {
                    self.advance(listener, chars[offset]);
                    offset += 1;
                }
            } else {
                self.advance(listener, chars[offset]);
                offset += 1;
            }
        }
    }

    /// Feed raw bytes, decoding per `use_utf8`. Mirrors `ByteStream.feed`.
    pub fn feed_bytes(&mut self, listener: &mut dyn Listener, data: &[u8]) {
        let text = if self.use_utf8 {
            decode_utf8_incremental(&mut self.pending_bytes, data)
        } else {
            data.iter().map(|&b| b as char).collect()
        };
        self.feed_str(listener, &text);
    }

    /// `ESC % C` — select control-character set. Only meaningful for a byte
    /// stream (toggles UTF-8); a no-op for text. Mirrors
    /// `ByteStream.select_other_charset`.
    fn select_other_charset(&mut self, code: char) {
        if !self.bytes {
            return;
        }
        match code {
            '@' => {
                self.use_utf8 = false;
                self.pending_bytes.clear();
            }
            'G' | '8' => self.use_utf8 = true,
            _ => {}
        }
    }

    fn enter_csi(&mut self) {
        self.state = State::Csi;
        self.csi_params.clear();
        self.csi_current.clear();
        self.csi_private = false;
    }

    fn finish_osc(&mut self, listener: &mut dyn Listener) {
        // Drop the leading ';'.
        let param: String = self.osc_param.chars().skip(1).collect();
        if self.osc_code == '0' || self.osc_code == '1' {
            listener.set_icon_name(&param);
        }
        if self.osc_code == '0' || self.osc_code == '2' {
            listener.set_title(&param);
        }
    }

    /// Step the parser by one character. Mirrors one iteration through pyte's
    /// FSM generator.
    fn advance(&mut self, listener: &mut dyn Listener, ch: char) {
        match self.state {
            State::Ground => self.advance_ground(listener, ch),
            State::Escape => self.advance_escape(listener, ch),
            State::EscapeSharp => {
                // `sharp_dispatch[char]()`
                listener.dispatch(sharp_name(ch), &[], false);
                self.state = State::Ground;
            }
            State::EscapePercent => {
                self.select_other_charset(ch);
                self.state = State::Ground;
            }
            State::EscapeCharset => {
                let mode = self.esc_charset_mode;
                if !self.use_utf8 {
                    listener.define_charset(ch, mode);
                }
                self.state = State::Ground;
            }
            State::Csi => self.advance_csi(listener, ch),
            State::CsiDollar => {
                // `yield None; break` — consume one char, back to ground.
                self.state = State::Ground;
            }
            State::OscCode => {
                // `code = yield None`
                if ch == 'R' || ch == 'P' {
                    // Reset/Set palette — not implemented.
                    self.state = State::Ground;
                } else {
                    self.osc_code = ch;
                    self.osc_param.clear();
                    self.osc_pending_esc = false;
                    self.state = State::OscString;
                }
            }
            State::OscString => self.advance_osc_string(listener, ch),
        }
    }

    fn advance_ground(&mut self, listener: &mut dyn Listener, ch: char) {
        if ch == ctrl::ESC {
            self.state = State::Escape;
        } else if let Some(name) = basic_name(ch) {
            // Ignore shifts in UTF-8 mode.
            if (ch == ctrl::SI || ch == ctrl::SO) && self.use_utf8 {
                return;
            }
            listener.dispatch(name, &[], false);
        } else if ch == ctrl::CSI_C1 {
            self.enter_csi();
        } else if ch == ctrl::OSC_C1 {
            self.state = State::OscCode;
        } else if ch != ctrl::NUL && ch != ctrl::DEL {
            let mut buf = [0u8; 4];
            listener.draw(ch.encode_utf8(&mut buf));
        }
    }

    fn advance_escape(&mut self, listener: &mut dyn Listener, ch: char) {
        match ch {
            '[' => self.enter_csi(),
            ']' => self.state = State::OscCode,
            '#' => self.state = State::EscapeSharp,
            '%' => self.state = State::EscapePercent,
            '(' | ')' => {
                self.esc_charset_mode = ch;
                self.state = State::EscapeCharset;
            }
            _ => {
                listener.dispatch(escape_name(ch), &[], false);
                self.state = State::Ground;
            }
        }
    }

    fn advance_csi(&mut self, listener: &mut dyn Listener, ch: char) {
        if ch == '?' {
            self.csi_private = true;
        } else if is_allowed_in_csi(ch) {
            // A control char mid-CSI executes immediately; the CSI continues.
            if let Some(name) = basic_name(ch) {
                listener.dispatch(name, &[], false);
            }
        } else if ch == ctrl::SP || ch == '>' {
            // Secondary DA / intermediate — ignored.
        } else if ch == ctrl::CAN || ch == ctrl::SUB {
            // Abort: draw the substitute character.
            let mut buf = [0u8; 4];
            listener.draw(ch.encode_utf8(&mut buf));
            self.state = State::Ground;
        } else if ch.is_ascii_digit() {
            self.csi_current.push(ch);
        } else if ch == '$' {
            // XTerm `$`-suffixed sequences: consume one more char, then abort.
            self.state = State::CsiDollar;
        } else {
            let n = if self.csi_current.is_empty() {
                0
            } else {
                self.csi_current.parse::<i64>().unwrap_or(0)
            };
            self.csi_params.push(n.min(9999));
            if ch == ';' {
                self.csi_current.clear();
            } else {
                let name = csi_name(ch);
                let params = std::mem::take(&mut self.csi_params);
                listener.dispatch(name, &params, self.csi_private);
                self.state = State::Ground;
            }
        }
    }

    fn advance_osc_string(&mut self, listener: &mut dyn Listener, ch: char) {
        if self.osc_pending_esc {
            // Previous char was ESC; this completes a 2-char terminator check.
            self.osc_pending_esc = false;
            if ch == '\\' {
                // ESC \ == ST_C0 terminator.
                self.finish_osc(listener);
                self.state = State::Ground;
            } else {
                self.osc_param.push(ctrl::ESC);
                self.osc_param.push(ch);
            }
        } else if ch == ctrl::ESC {
            self.osc_pending_esc = true;
        } else if ch == ctrl::ST_C1 || ch == ctrl::BEL {
            self.finish_osc(listener);
            self.state = State::Ground;
        } else {
            self.osc_param.push(ch);
        }
    }
}

// ── plain-text / special classification ──────────────────────────────────

/// Whether a character breaks a plain-text run. Mirrors pyte's `_special` set:
/// `{ESC, CSI_C1, NUL, DEL, OSC_C1}` plus every `basic` control key.
fn is_special(c: char) -> bool {
    matches!(
        c,
        ctrl::ESC | ctrl::CSI_C1 | ctrl::NUL | ctrl::DEL | ctrl::OSC_C1
    ) || basic_name(c).is_some()
}

/// Control characters permitted (and executed) inside a CSI sequence.
fn is_allowed_in_csi(c: char) -> bool {
    matches!(
        c,
        ctrl::BEL | ctrl::BS | ctrl::HT | ctrl::LF | ctrl::VT | ctrl::FF | ctrl::CR
    )
}

// ── event-name maps (mirror the dicts in pyte.streams.Stream) ─────────────

/// `Stream.basic`: control chars requiring no arguments.
fn basic_name(c: char) -> Option<&'static str> {
    Some(match c {
        ctrl::BEL => "bell",
        ctrl::BS => "backspace",
        ctrl::HT => "tab",
        ctrl::LF | ctrl::VT | ctrl::FF => "linefeed",
        ctrl::CR => "carriage_return",
        ctrl::SO => "shift_out",
        ctrl::SI => "shift_in",
        _ => return None,
    })
}

/// `Stream.escape`: non-CSI escape sequences. Unknown ops fall through to
/// `"debug"`.
fn escape_name(c: char) -> &'static str {
    match c {
        esc::RIS => "reset",
        esc::IND => "index",
        esc::NEL => "linefeed",
        esc::RI => "reverse_index",
        esc::HTS => "set_tab_stop",
        esc::DECSC => "save_cursor",
        esc::DECRC => "restore_cursor",
        _ => "debug",
    }
}

/// `Stream.sharp`: `ESC # N` sequences. Unknown ops fall through to `"debug"`.
fn sharp_name(c: char) -> &'static str {
    match c {
        esc::DECALN => "alignment_display",
        _ => "debug",
    }
}

/// `Stream.csi`: CSI final bytes. Unknown ops fall through to `"debug"`.
fn csi_name(c: char) -> &'static str {
    match c {
        esc::ICH => "insert_characters",
        esc::CUU => "cursor_up",
        esc::CUD => "cursor_down",
        esc::CUF => "cursor_forward",
        esc::CUB => "cursor_back",
        esc::CNL => "cursor_down1",
        esc::CPL => "cursor_up1",
        esc::CHA => "cursor_to_column",
        esc::CUP => "cursor_position",
        esc::ED => "erase_in_display",
        esc::EL => "erase_in_line",
        esc::IL => "insert_lines",
        esc::DL => "delete_lines",
        esc::DCH => "delete_characters",
        esc::ECH => "erase_characters",
        esc::HPR => "cursor_forward",
        esc::DA => "report_device_attributes",
        esc::VPA => "cursor_to_line",
        esc::VPR => "cursor_down",
        esc::HVP => "cursor_position",
        esc::TBC => "clear_tab_stop",
        esc::SM => "set_mode",
        esc::RM => "reset_mode",
        esc::SGR => "select_graphic_rendition",
        esc::DSR => "report_device_status",
        esc::DECSTBM => "set_margins",
        esc::HPA => "cursor_to_column",
        _ => "debug",
    }
}

// ── incremental UTF-8 decoding (ByteStream.utf8_decoder) ──────────────────

/// Decode `new_bytes` (prepended with any `pending` incomplete tail) as UTF-8,
/// emitting U+FFFD for malformed bytes and holding a trailing incomplete
/// sequence in `pending` for the next call. Approximates Python's
/// `codecs.getincrementaldecoder("utf-8")("replace")`.
fn decode_utf8_incremental(pending: &mut Vec<u8>, new_bytes: &[u8]) -> String {
    pending.extend_from_slice(new_bytes);
    let mut out = String::new();
    loop {
        match std::str::from_utf8(pending) {
            Ok(s) => {
                out.push_str(s);
                pending.clear();
                break;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                // SAFETY: `valid_up_to` guarantees this prefix is valid UTF-8.
                out.push_str(unsafe { std::str::from_utf8_unchecked(&pending[..valid]) });
                match e.error_len() {
                    Some(bad) => {
                        // A malformed sequence of `bad` bytes → one replacement.
                        out.push('\u{FFFD}');
                        pending.drain(..valid + bad);
                    }
                    None => {
                        // Incomplete trailing sequence — keep it for next feed.
                        pending.drain(..valid);
                        break;
                    }
                }
            }
        }
    }
    out
}
