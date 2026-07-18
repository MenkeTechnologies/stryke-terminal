#!/usr/bin/env python3
"""Generate golden fixtures from the reference pyte emulator.

Feeds a battery of escape/CSI/OSC inputs to pyte's ByteStream + Screen /
HistoryScreen and records the resulting display, cursor, title, icon name, and
full styled buffer. The Rust port (`tests/golden.rs`) replays the identical
inputs and asserts byte-for-byte parity, so `golden.json` is committed and the
test needs no Python at CI time.

Run: python3 tests/gen_golden.py  (regenerates tests/golden.json)
"""
import base64
import json
import os

import pyte

FIELDS = ("data", "fg", "bg", "bold", "italics", "underscore",
          "strikethrough", "reverse", "blink")


def dump_cell(cell):
    return {f: getattr(cell, f) for f in FIELDS}


def dump_buffer(screen):
    rows = []
    for y in range(screen.lines):
        row = [dump_cell(screen.buffer[y][x]) for x in range(screen.columns)]
        rows.append(row)
    return rows


def run(cols, lines, feeds, kind="screen", history=100, ratio=0.5, pages=None):
    if kind == "history":
        screen = pyte.HistoryScreen(cols, lines, history=history, ratio=ratio)
    else:
        screen = pyte.Screen(cols, lines)
    # Capture device reports (write_process_input) so the Rust `take_input`
    # path can be checked; pyte's default is a no-op that records nothing.
    outgoing = []
    screen.write_process_input = lambda data: outgoing.append(data)
    stream = pyte.ByteStream(screen)
    for chunk in feeds:
        stream.feed(chunk)
    for op in (pages or []):
        getattr(screen, op)()
    return {
        "outgoing": "".join(outgoing),
        "cols": cols,
        "lines": lines,
        "kind": kind,
        "history": history,
        "ratio": ratio,
        "feeds": [base64.b64encode(c).decode("ascii") for c in feeds],
        "pages": pages or [],
        "display": list(screen.display),
        "cursor": {"x": screen.cursor.x, "y": screen.cursor.y,
                   "hidden": screen.cursor.hidden},
        "title": screen.title,
        "icon_name": screen.icon_name,
        "buffer": dump_buffer(screen),
    }


CASES = {
    # plain text + wrapping
    "plain": run(20, 3, [b"hello world"]),
    "wrap": run(5, 3, [b"abcdefghij"]),
    "no_autowrap": run(5, 2, [b"\x1b[?7l", b"abcdefgh"]),
    "newlines": run(10, 4, [b"a\r\nb\r\nc"]),
    "tab": run(20, 2, [b"a\tb\tc"]),
    "backspace": run(10, 2, [b"abc\x08\x08X"]),

    # cursor movement
    "cup": run(10, 5, [b"\x1b[3;4Hx"]),
    "cursor_moves": run(10, 5, [b"\x1b[5B\x1b[3C\x1b[2A\x1b[1Dz"]),
    "cha_vpa": run(10, 5, [b"\x1b[4Ghi\x1b[2dY"]),

    # erasing
    "erase_line_0": run(10, 2, [b"abcdefg\x1b[4D\x1b[K"]),
    "erase_line_2": run(10, 2, [b"abcdefg\x1b[2K"]),
    "erase_display_0": run(6, 3, [b"aaaaaa\r\nbbbbbb\r\ncccccc\x1b[2;3H\x1b[J"]),
    "erase_display_2": run(6, 3, [b"aaaaaa\r\nbbbbbb\r\ncccccc\x1b[2J"]),
    "erase_chars": run(10, 2, [b"abcdefgh\x1b[6D\x1b[3X"]),

    # SGR colors + attributes
    "sgr_basic": run(12, 2, [b"\x1b[1;31mBOLD\x1b[0m plain"]),
    "sgr_bg_ansi": run(12, 2, [b"\x1b[42;34mgreen"]),
    "sgr_256_fg": run(12, 2, [b"\x1b[38;5;196mred256"]),
    "sgr_truecolor": run(12, 2, [b"\x1b[38;2;10;20;30mtc"]),
    "sgr_aixterm": run(12, 2, [b"\x1b[92mbright"]),
    "sgr_attrs": run(20, 2, [b"\x1b[4munder\x1b[24m\x1b[3mital"]),

    # scrolling / index / margins
    "index_scroll": run(6, 3, [b"L1\r\nL2\r\nL3\x1b[3;1H\n\nX"]),
    "reverse_index": run(6, 3, [b"L1\r\nL2\r\nL3\x1b[HX\x1bMY"]),
    "margins_scroll": run(6, 5, [b"\x1b[2;4r\x1b[2;1H"
                                 b"a\r\nb\r\nc\r\nd\r\ne"]),
    "insert_lines": run(6, 4, [b"aaa\r\nbbb\r\nccc\x1b[1;1H\x1b[2L"]),
    "delete_lines": run(6, 4, [b"aaa\r\nbbb\r\nccc\r\nddd\x1b[1;1H\x1b[2M"]),
    "insert_chars": run(10, 2, [b"abcdef\x1b[4D\x1b[3@"]),
    "delete_chars": run(10, 2, [b"abcdef\x1b[6D\x1b[2P"]),
    "irm_insert": run(10, 2, [b"abcdef\x1b[4D\x1b[4hXY"]),

    # charsets, alignment, wide + combining
    "vt100_charset": run(8, 2, [b"\x1b(0lqk\x1b(B done"]),
    # ESC % @ disables UTF-8, so ESC ( 0 actually maps G0 to the VT100 line
    # drawing set and "lqk" renders as box glyphs.
    "vt100_charset_8bit": run(8, 2, [b"\x1b%@\x1b(0lqxk\x1b(Bz"]),
    "alignment": run(4, 2, [b"\x1b#8"]),
    "wide_cjk": run(8, 2, ["a你好b".encode("utf-8")]),
    "combining": run(6, 2, ["éx".encode("utf-8")]),

    # titles / OSC
    "osc_title": run(10, 2, [b"\x1b]2;My Title\x07hi"]),
    "osc_icon": run(10, 2, [b"\x1b]1;IconName\x07"]),
    "osc_both": run(10, 2, [b"\x1b]0;Both\x1b\\text"]),

    # device status report (cursor position) -> captured write_process_input
    "dsr_cursor": run(10, 3, [b"\x1b[2;5H\x1b[6n"]),
    "dsr_status": run(10, 3, [b"\x1b[5n"]),
    "device_attributes": run(10, 3, [b"\x1b[c"]),

    # history: scroll a bunch, page up then down
    "history_prev": run(4, 3, [b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8"],
                        kind="history", history=10, ratio=0.5,
                        pages=["prev_page"]),
    "history_prev_next": run(4, 3, [b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n8"],
                            kind="history", history=10, ratio=0.5,
                            pages=["prev_page", "next_page"]),
}

# Disassembler fixtures: (input_str, expected DebugEvent list) via pyte.dis.
import io  # noqa: E402
from pyte.screens import DebugScreen  # noqa: E402


def dis(s):
    with io.StringIO() as buf:
        pyte.ByteStream(DebugScreen(to=buf)).feed(s.encode("utf-8"))
        return [json.loads(line) for line in buf.getvalue().splitlines() if line]


DIS_CASES = {
    "dis_bell": dis("\x07"),
    "dis_sgr": dis("\x1b[20m"),
    "dis_cup": dis("\x1b[3;4H"),
    "dis_mixed": dis("\x1b[Jfoo"),
    "dis_private": dis("\x1b[?7h"),
    "dis_title": dis("\x1b]2;hi\x07"),
}


def main():
    out = {"cases": CASES, "dis": DIS_CASES}
    path = os.path.join(os.path.dirname(__file__), "golden.json")
    with open(path, "w") as f:
        json.dump(out, f, indent=1, ensure_ascii=False)
        f.write("\n")
    print(f"wrote {path}: {len(CASES)} screen cases, {len(DIS_CASES)} dis cases")


if __name__ == "__main__":
    main()
