//! Port of pyte `pyte/control.py`.
//!
//! Simple control sequences recognized by [`crate::streams::Stream`]. The set
//! here is for `TERM=linux`, a superset of VT102. Upstream pyte:
//! <https://github.com/selectel/pyte> (LGPL). Constant names and values match
//! the Python module 1:1.

/// *Space*: not surprisingly — `" "`.
pub const SP: char = ' ';

/// *Null*: does nothing.
pub const NUL: char = '\u{0}';

/// *Bell*: beeps.
pub const BEL: char = '\u{7}';

/// *Backspace*: backspace one column, but not past the beginning of the line.
pub const BS: char = '\u{8}';

/// *Horizontal tab*: move cursor to the next tab stop, or to the end of the
/// line if there is no earlier tab stop.
pub const HT: char = '\u{9}';

/// *Linefeed*: give a line feed, and, if [`crate::modes::LNM`] (new line mode)
/// is set, also a carriage return.
pub const LF: char = '\n';
/// *Vertical tab*: same as [`LF`].
pub const VT: char = '\u{b}';
/// *Form feed*: same as [`LF`].
pub const FF: char = '\u{c}';

/// *Carriage return*: move cursor to left margin on current line.
pub const CR: char = '\r';

/// *Shift out*: activate G1 character set.
pub const SO: char = '\u{e}';

/// *Shift in*: activate G0 character set.
pub const SI: char = '\u{f}';

/// *Cancel*: interrupt escape sequence. If received during an escape or control
/// sequence, cancels the sequence and displays substitution character.
pub const CAN: char = '\u{18}';
/// *Substitute*: same as [`CAN`].
pub const SUB: char = '\u{1a}';

/// *Escape*: starts an escape sequence.
pub const ESC: char = '\u{1b}';

/// *Delete*: is ignored.
pub const DEL: char = '\u{7f}';

/// *Control sequence introducer* (7-bit form, `ESC [`).
pub const CSI_C0: &str = "\u{1b}[";
/// *Control sequence introducer* (8-bit form).
pub const CSI_C1: char = '\u{9b}';
/// *Control sequence introducer*: alias for the 7-bit form.
pub const CSI: &str = CSI_C0;

/// *String terminator* (7-bit form, `ESC \`).
pub const ST_C0: &str = "\u{1b}\\";
/// *String terminator* (8-bit form).
pub const ST_C1: char = '\u{9c}';

/// *Operating system command* (7-bit form, `ESC ]`).
pub const OSC_C0: &str = "\u{1b}]";
/// *Operating system command* (8-bit form).
pub const OSC_C1: char = '\u{9d}';
