//! Character width + combining helpers matching pyte's use of the `wcwidth`
//! and `unicodedata` libraries in `pyte/screens.py`.
//!
//! pyte calls `wcwidth(char)` to classify each drawn character and
//! `unicodedata.combining(char)` to decide whether a zero-width character
//! combines with its predecessor. We map those onto the `unicode-width` and
//! `unicode-normalization` crates:
//!
//! * [`wcwidth`] returns `-1` for control characters, `0` for zero-width /
//!   combining marks, and `1` or `2` for narrow / wide characters — the same
//!   contract pyte relies on.
//! * [`combining`] mirrors `unicodedata.combining` (canonical combining class
//!   is non-zero).

use unicode_normalization::char::canonical_combining_class;
use unicode_width::UnicodeWidthChar;

/// Cell width of a character: `-1` control, `0` zero-width/combining, `1`
/// narrow, `2` wide. Mirrors the `wcwidth` library contract pyte depends on.
#[inline]
pub fn wcwidth(ch: char) -> i32 {
    match ch.width() {
        // `unicode-width` returns `None` for control characters (C0, C1, DEL).
        None => -1,
        Some(w) => w as i32,
    }
}

/// Whether `ch` has a non-zero canonical combining class, matching
/// `unicodedata.combining(ch)` used in [`crate::screens::Screen::draw`].
#[inline]
pub fn combining(ch: char) -> bool {
    canonical_combining_class(ch) != 0
}
