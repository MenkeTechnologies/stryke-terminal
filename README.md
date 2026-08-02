```
 ███████╗████████╗██████╗ ██╗   ██╗██╗  ██╗███████╗
 ██╔════╝╚══██╔══╝██╔══██╗╚██╗ ██╔╝██║ ██╔╝██╔════╝
 ███████╗   ██║   ██████╔╝ ╚████╔╝ █████╔╝ █████╗
 ╚════██║   ██║   ██╔══██╗  ╚██╔╝  ██╔═██╗ ██╔══╝
 ███████║   ██║   ██║  ██║   ██║   ██║  ██╗███████╗
 ╚══════╝   ╚═╝   ╚═╝  ╚═╝   ╚═╝   ╚═╝  ╚═╝╚══════╝
                [ t e r m i n a l ]
```

[![CI](https://github.com/MenkeTechnologies/stryke-terminal/actions/workflows/ci.yml/badge.svg)](https://github.com/MenkeTechnologies/stryke-terminal/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![stryke](https://img.shields.io/badge/stryke-package-cyan.svg)](https://github.com/MenkeTechnologies/strykelang)

### `[HEADLESS TERMINAL EMULATOR FOR STRYKE // VT100 + VT220 + LINUX SCREEN MODEL]`

> *"pyte, one stryke pipe away."*

Headless VTXXX terminal emulator for stryke — a faithful port of
[pyte](https://github.com/selectel/pyte). Feed it the raw byte stream a program
writes (colors, cursor moves, erases, scroll regions, insert/delete, charsets,
titles) and it maintains a full VT100 / VT220 / `TERM=linux` **screen model**:
the character grid, cursor position, per-cell colors and attributes, terminal
modes, scroll margins, tab stops, and scrollback history. Then read the
*rendered* screen — `Terminal::display`, per-cell attributes — instead of
escape-laden bytes. Shipped as a precompiled cdylib that stryke dlopens
in-process on first `use Terminal`; emulator sessions persist across calls.

### Why this exists

strykelang already ships Tcl/Expect-style PTY automation as **built-in**
functions — `pty_spawn`, `pty_read`, `pty_send`, `pty_expect`, `pty_close`,
`pty_strip_ansi`. Those hand you the raw bytes a program writes, with only a
dumb `pty_strip_ansi` to make sense of them — no cursor movement, no erase, no
scroll, no insert/delete, no color/attribute state, no scrollback.

`stryke-terminal` is the missing piece: the **screen model** those bytes render
into. Drive `htop` / `vim` / `less` headlessly through the `pty_*` builtins,
pump their output into a `Terminal`, and read the exact screen a human would
see. It does **not** re-implement PTY spawning — it consumes `pty_read` output.

### [`strykelang`](https://github.com/MenkeTechnologies/strykelang) &middot; [`stryke-selenium`](https://github.com/MenkeTechnologies/stryke-selenium) &middot; [`stryke-scrape`](https://github.com/MenkeTechnologies/stryke-scrape)

### [`Read the Docs`](https://menketechnologies.github.io/stryke-terminal/) &middot; [`Engineering Report`](https://menketechnologies.github.io/stryke-terminal/report.html)

---

## Table of Contents

- [\[0x00\] How this loads](#0x00-how-this-loads)
- [\[0x01\] Install](#0x01-install)
- [\[0x02\] Quick start](#0x02-quick-start)
- [\[0x03\] Driving a real program headless](#0x03-driving-a-real-program-headless)
- [\[0x04\] API reference](#0x04-api-reference)
- [\[0x05\] Fidelity](#0x05-fidelity)
- [\[0x06\] License](#0x06-license)

---

## [0x00] How this loads

`stryke-terminal` is a precompiled Rust **cdylib**. On the first `use Terminal`,
stryke's FFI loader dlopens `libstryke_terminal.{dylib,so}` and registers every
`terminal__*` export as a stryke-callable function. Each call passes a
JSON-encoded args dict and receives JSON back.

Emulator sessions — a screen plus its escape-sequence parser — live in a
process-global registry inside the cdylib, so a stryke script creates a terminal
once and feeds it output across many `Terminal::*` calls without rebuilding
state.

```
 pty_spawn("htop")  ──bytes──▶  Terminal::feed  ──▶  Screen model  ──▶  Terminal::display
   (strykelang builtin)          (this package)      (grid+cursor+SGR)     (rendered lines)
```

## [0x01] Install

```sh
s pkg install -g github.com/MenkeTechnologies/stryke-terminal
```

Or build from a local checkout:

```sh
git clone https://github.com/MenkeTechnologies/stryke-terminal
cd stryke-terminal
make install          # cargo build --release + s pkg install -g .
```

Verify the whole stack end-to-end (permission-free — no PTY, no subprocess):

```sh
terminal-test
```

## [0x02] Quick start

Parse a stream of terminal output and read the rendered screen:

```perl
use Terminal

val $t = Terminal::new(columns => 80, lines => 24)

# Feed the kind of bytes a program emits: colored text + a cursor move.
Terminal::feed($t, "\x1b[1;32mBUILD OK\x1b[0m")
Terminal::feed($t, "\x1b[3;1Hline three")

for $line (Terminal::display($t)) {
    p $line                       # the rendered screen, one string per row
}

val $cell = Terminal::cell($t, 0, 0)
p "$cell->{data} fg=$cell->{fg} bold=$cell->{bold}"   # B fg=green bold=1

Terminal::destroy($t)
```

## [0x03] Driving a real program headless

Combine the strykelang `pty_*` builtins (which spawn and talk to the process)
with `Terminal` (which renders what it draws):

```perl
use Terminal

val $h = pty_spawn("vim -u NONE")     # strykelang builtin — allocates a PTY
val $t = Terminal::new(columns => 80, lines => 24)

Terminal::drain($t, $h, 1)            # pump pty_read → Terminal::feed until idle
pty_send($h, "ihello\x1b")            # type into vim
Terminal::drain($t, $h, 1)

p Terminal::display($t)               # what vim's screen now shows

# Some programs query the terminal (DA / DSR / cursor position). Forward the
# emulator's replies back to the child so they keep working:
pty_send($h, Terminal::take_input($t))

pty_send($h, ":q!\x0d")
pty_close($h)
```

`Terminal::drain` is the core loop of headless terminal automation: spawn,
drain, read the screen, act, repeat.

## [0x04] API reference

### Session lifecycle

| Function | Returns |
|---|---|
| `Terminal::new(columns => 80, lines => 24, kind => "screen"\|"history", history => 100, ratio => 0.5)` | session id |
| `Terminal::destroy($t)` | 1 |
| `Terminal::sessions()` | list of live ids |
| `Terminal::reset($t)` | 1 |
| `Terminal::resize($t, $lines, $columns)` | 1 |
| `Terminal::size($t)` | `{ columns, lines }` |

### Feeding

| Function | Notes |
|---|---|
| `Terminal::feed($t, $data)` | feed terminal output (text) |
| `Terminal::feed_bytes($t, $base64)` | feed raw bytes when output may not be valid UTF-8 |

### Reading the rendered screen

| Function | Returns |
|---|---|
| `Terminal::display($t)` | list of line strings |
| `Terminal::line($t, $y)` | one rendered line |
| `Terminal::cell($t, $x, $y)` | `{ data, fg, bg, bold, italics, underscore, strikethrough, reverse, blink }` |
| `Terminal::buffer($t)` | full styled matrix (rows of cells) |
| `Terminal::cursor($t)` | `{ x, y, hidden, attrs }` |
| `Terminal::dirty($t)` / `Terminal::clear_dirty($t)` | changed line numbers / clear the set |
| `Terminal::title($t)` / `Terminal::icon_name($t)` | OSC-set strings |
| `Terminal::mode($t)` | active mode numbers (private modes are pyte-shifted) |
| `Terminal::take_input($t)` | drain device-report replies to forward with `pty_send` |

### Scrollback (`kind => "history"`)

`Terminal::prev_page($t)`, `Terminal::next_page($t)`, `Terminal::history($t)` →
`{ position, size, top, bottom }`.

### PTY integration convenience

`Terminal::pump($t, $pty, $timeout)` — one `pty_read` + feed, returns the chunk
or undef at EOF. `Terminal::drain($t, $pty, $timeout)` — pump to EOF, return the
rendered display.

### Direct screen commands

Drive the screen without escape sequences — `Terminal::draw`,
`cursor_position`, `cursor_up/down/forward/back/up1/down1`, `cursor_to_column`,
`cursor_to_line`, `carriage_return`, `index`, `reverse_index`, `linefeed`,
`tab`, `backspace`, `save_cursor`, `restore_cursor`, `insert_lines`,
`delete_lines`, `insert_characters`, `delete_characters`, `erase_characters`,
`erase_in_line`, `erase_in_display`, `set_tab_stop`, `clear_tab_stop`,
`set_mode` / `set_mode_private`, `reset_mode` / `reset_mode_private`,
`select_graphic_rendition` (alias `sgr`), `define_charset`, `shift_in`,
`shift_out`, `set_margins`, `alignment_display`, `bell`,
`report_device_attributes`, `report_device_status`.

### Disassembler

`Terminal::dis($data)` decodes an escape/CSI/OSC string into a list of
`[name, [args], {kwargs}]` events — a stryke-native escape-sequence decoder:

```perl
p Terminal::dis("\x1b[3;4Hfoo")
# [ ["cursor_position", [3, 4], {}], ["draw", ["foo"], {}] ]
```

## [0x05] Fidelity

This is a faithful port of pyte 0.8.2 — VT100 / VT220 / `TERM=linux`. The test
suite generates golden fixtures by feeding the reference pyte the same inputs
(`tests/gen_golden.py`) and asserts the Rust port reproduces the display,
cursor, styled buffer, titles, device reports, and scrollback byte-for-byte
(`tests/golden.rs`). The committed `tests/golden.json` means CI needs no Python.

## [0x06] License

MIT. This is a port of pyte (LGPL upstream); the VT100/IBMPC charset tables and
the 256-color table are, per pyte's own notes, reproduced from the Linux kernel
and Pygments respectively, for emulation parity.
