//! Port of pyte `pyte/modes.py`.
//!
//! Terminal mode switches used by [`crate::screens::Screen`]. Two kinds:
//!
//! * *non-private* — set with `ESC [ N h`;
//! * *private* — set with `ESC [ ? N h`.
//!
//! Private modes are shifted left 5 bits so they stay distinct from the
//! non-private ones; e.g. Origin Mode ([`DECOM`]) is `192`, not `6`. This is a
//! faithful port — the numeric values match Python exactly.

/// *Line Feed / New Line Mode*: when enabled, a received `LF`, `FF`, or `VT`
/// moves the cursor to the first column of the next line.
pub const LNM: u32 = 20;

/// *Insert / Replace Mode*: when enabled, new display characters move old
/// characters to the right; otherwise they replace at the cursor position.
pub const IRM: u32 = 4;

// ── private modes ───────────────────────────────────────────────────────

/// *Text Cursor Enable Mode*: determines if the text cursor is visible.
pub const DECTCEM: u32 = 25 << 5;

/// *Screen Mode*: toggles screen-wide reverse-video mode.
pub const DECSCNM: u32 = 5 << 5;

/// *Origin Mode*: cursor addressing relative to a user-defined origin.
pub const DECOM: u32 = 6 << 5;

/// *Auto Wrap Mode*: selects where received graphic characters appear when the
/// cursor is at the right margin.
pub const DECAWM: u32 = 7 << 5;

/// *Column Mode*: selects the number of columns per line (80 or 132).
pub const DECCOLM: u32 = 3 << 5;
