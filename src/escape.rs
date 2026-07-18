//! Port of pyte `pyte/escape.py`.
//!
//! Both CSI and non-CSI escape sequences recognized by [`crate::streams::Stream`].
//! Values are the single trailing byte of each sequence and match Python 1:1.

// ── non-CSI escape sequences ────────────────────────────────────────────

/// *Reset*.
pub const RIS: char = 'c';

/// *Index*: move cursor down one line in same column. At the bottom margin the
/// screen scrolls up.
pub const IND: char = 'D';

/// *Next line*: same as [`crate::control::LF`].
pub const NEL: char = 'E';

/// *Tabulation set*: set a horizontal tab stop at cursor position.
pub const HTS: char = 'H';

/// *Reverse index*: move cursor up one line in same column. At the top margin
/// the screen scrolls down.
pub const RI: char = 'M';

/// *Save cursor*: save cursor position, character attribute, character set, and
/// origin mode selection (see [`DECRC`]).
pub const DECSC: char = '7';

/// *Restore cursor*: restore previously saved cursor state. If none were saved,
/// move cursor to home position.
pub const DECRC: char = '8';

// ── "Sharp" escape sequences ────────────────────────────────────────────

/// *Alignment display*: fill screen with uppercase E's for testing screen focus
/// and alignment.
pub const DECALN: char = '8';

// ── ECMA-48 CSI sequences ───────────────────────────────────────────────

/// *Insert character*: insert the indicated # of blank characters.
pub const ICH: char = '@';

/// *Cursor up*: move cursor up the indicated # of lines in same column. Cursor
/// stops at top margin.
pub const CUU: char = 'A';

/// *Cursor down*: move cursor down the indicated # of lines in same column.
/// Cursor stops at bottom margin.
pub const CUD: char = 'B';

/// *Cursor forward*: move cursor right the indicated # of columns. Cursor stops
/// at right margin.
pub const CUF: char = 'C';

/// *Cursor back*: move cursor left the indicated # of columns. Cursor stops at
/// left margin.
pub const CUB: char = 'D';

/// *Cursor next line*: move cursor down the indicated # of lines to column 1.
pub const CNL: char = 'E';

/// *Cursor previous line*: move cursor up the indicated # of lines to column 1.
pub const CPL: char = 'F';

/// *Cursor horizontal align*: move cursor to the indicated column in current
/// line.
pub const CHA: char = 'G';

/// *Cursor position*: move cursor to the indicated line, column (origin at
/// `1, 1`).
pub const CUP: char = 'H';

/// *Erase data* (default: from cursor to end of line).
pub const ED: char = 'J';

/// *Erase in line* (default: from cursor to end of line).
pub const EL: char = 'K';

/// *Insert line*: insert the indicated # of blank lines, starting from the
/// current line. Lines below cursor move down; lines past the bottom margin are
/// lost.
pub const IL: char = 'L';

/// *Delete line*: delete the indicated # of lines, starting from the current
/// line. Lines below cursor move up.
pub const DL: char = 'M';

/// *Delete character*: delete the indicated # of characters on the current
/// line. Characters to the right of cursor move left.
pub const DCH: char = 'P';

/// *Erase character*: erase the indicated # of characters on the current line.
pub const ECH: char = 'X';

/// *Horizontal position relative*: same as [`CUF`].
pub const HPR: char = 'a';

/// *Device Attributes*.
pub const DA: char = 'c';

/// *Vertical position adjust*: move cursor to the indicated line, current
/// column.
pub const VPA: char = 'd';

/// *Vertical position relative*: same as [`CUD`].
pub const VPR: char = 'e';

/// *Horizontal / Vertical position*: same as [`CUP`].
pub const HVP: char = 'f';

/// *Tabulation clear*: clears a horizontal tab stop at cursor position.
pub const TBC: char = 'g';

/// *Set mode*.
pub const SM: char = 'h';

/// *Reset mode*.
pub const RM: char = 'l';

/// *Select graphics rendition*: change character display attributes without
/// changing the character (see [`crate::graphics`]).
pub const SGR: char = 'm';

/// *Device status report*.
pub const DSR: char = 'n';

/// *Select top and bottom margins*: define the scrolling region.
pub const DECSTBM: char = 'r';

/// *Horizontal position adjust*: same as [`CHA`].
pub const HPA: char = '\'';
