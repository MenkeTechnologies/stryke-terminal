//! Port of pyte `pyte/graphics.py`.
//!
//! Graphic-related constants (SGR), mostly from `console_codes(4)` and
//! <http://pueblo.sourceforge.net/doc/manual/ansi_color_codes.html>. Mappings
//! are expressed as `match` lookups returning the same values pyte's dicts do.

use once_cell::sync::Lazy;

/// A mapping of ANSI text style codes to `"+field"` / `"-field"` names. `"+"`
/// means the attribute is set, `"-"` reset — e.g. `1 -> "+bold"`,
/// `9 -> "+strikethrough"`. Mirrors `pyte.graphics.TEXT`.
pub fn text(attr: i64) -> Option<&'static str> {
    Some(match attr {
        1 => "+bold",
        3 => "+italics",
        4 => "+underscore",
        5 => "+blink",
        7 => "+reverse",
        9 => "+strikethrough",
        22 => "-bold",
        23 => "-italics",
        24 => "-underscore",
        25 => "-blink",
        27 => "-reverse",
        29 => "-strikethrough",
        _ => return None,
    })
}

/// A mapping of ANSI foreground color codes to color names.
/// Mirrors `pyte.graphics.FG_ANSI` (`39 -> "default"`).
pub fn fg_ansi(attr: i64) -> Option<&'static str> {
    Some(match attr {
        30 => "black",
        31 => "red",
        32 => "green",
        33 => "brown",
        34 => "blue",
        35 => "magenta",
        36 => "cyan",
        37 => "white",
        39 => "default", // white.
        _ => return None,
    })
}

/// A mapping of non-standard `aixterm` foreground color codes (high intensity).
/// Mirrors `pyte.graphics.FG_AIXTERM`.
pub fn fg_aixterm(attr: i64) -> Option<&'static str> {
    Some(match attr {
        90 => "brightblack",
        91 => "brightred",
        92 => "brightgreen",
        93 => "brightbrown",
        94 => "brightblue",
        95 => "brightmagenta",
        96 => "brightcyan",
        97 => "brightwhite",
        _ => return None,
    })
}

/// A mapping of ANSI background color codes to color names.
/// Mirrors `pyte.graphics.BG_ANSI` (`49 -> "default"`).
pub fn bg_ansi(attr: i64) -> Option<&'static str> {
    Some(match attr {
        40 => "black",
        41 => "red",
        42 => "green",
        43 => "brown",
        44 => "blue",
        45 => "magenta",
        46 => "cyan",
        47 => "white",
        49 => "default", // black.
        _ => return None,
    })
}

/// A mapping of non-standard `aixterm` background color codes (high intensity).
/// Mirrors `pyte.graphics.BG_AIXTERM`.
///
/// NOTE: `105 -> "bfightmagenta"` reproduces an upstream pyte typo verbatim
/// (should read `"brightmagenta"`), preserved for byte-for-byte parity with the
/// reference emulator's SGR output.
pub fn bg_aixterm(attr: i64) -> Option<&'static str> {
    Some(match attr {
        100 => "brightblack",
        101 => "brightred",
        102 => "brightgreen",
        103 => "brightbrown",
        104 => "brightblue",
        105 => "bfightmagenta",
        106 => "brightcyan",
        107 => "brightwhite",
        _ => return None,
    })
}

/// SGR code for foreground in 256 or True color mode.
pub const FG_256: i64 = 38;

/// SGR code for background in 256 or True color mode.
pub const BG_256: i64 = 48;

/// A table of 256 foreground/background colors as `"rrggbb"` hex strings.
///
/// The first 16 entries are the standard palette (part of the Pygments
/// project, BSD licensed); 16..231 are the 6×6×6 color cube; 232..255 are
/// grayscale. Mirrors `pyte.graphics.FG_BG_256`.
pub static FG_BG_256: Lazy<Vec<String>> = Lazy::new(|| {
    let mut table: Vec<(u8, u8, u8)> = vec![
        (0x00, 0x00, 0x00), // 0
        (0xcd, 0x00, 0x00), // 1
        (0x00, 0xcd, 0x00), // 2
        (0xcd, 0xcd, 0x00), // 3
        (0x00, 0x00, 0xee), // 4
        (0xcd, 0x00, 0xcd), // 5
        (0x00, 0xcd, 0xcd), // 6
        (0xe5, 0xe5, 0xe5), // 7
        (0x7f, 0x7f, 0x7f), // 8
        (0xff, 0x00, 0x00), // 9
        (0x00, 0xff, 0x00), // 10
        (0xff, 0xff, 0x00), // 11
        (0x5c, 0x5c, 0xff), // 12
        (0xff, 0x00, 0xff), // 13
        (0x00, 0xff, 0xff), // 14
        (0xff, 0xff, 0xff), // 15
    ];

    // colors 16..231: the 6x6x6 color cube
    let valuerange: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
    for i in 0..216usize {
        let r = valuerange[(i / 36) % 6];
        let g = valuerange[(i / 6) % 6];
        let b = valuerange[i % 6];
        table.push((r, g, b));
    }

    // colors 232..255: grayscale
    for i in 0..24u8 {
        let v = 8 + i * 10;
        table.push((v, v, v));
    }

    table
        .into_iter()
        .map(|(r, g, b)| format!("{:02x}{:02x}{:02x}", r, g, b))
        .collect()
});
