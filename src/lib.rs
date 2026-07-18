//! stryke-terminal — headless VTXXX terminal emulator cdylib, a faithful port
//! of pyte 0.8.2, loaded in-process by stryke via dlopen on first `use
//! Terminal`.
//!
//! Each `#[no_mangle] extern "C" fn terminal__*` is a JSON-string-in /
//! JSON-string-out wrapper around the [`screens`]/[`streams`] port. stryke's
//! FFI bridge (`rust_ffi.rs::load_cdylib`) resolves these symbols, registers
//! each as a stryke-callable function, and copies the returned JSON into a
//! stryke string; [`stryke_free_cstring`] frees the allocation.
//!
//! Role in the stack: strykelang already ships the PTY/Expect builtins
//! (`pty_spawn`, `pty_read`, `pty_send`, `pty_expect`, …). Those hand back the
//! *raw bytes* a program writes. stryke-terminal is the missing screen model —
//! feed it those bytes and read the rendered grid, cursor, colors, modes, and
//! scrollback that a human would see. It deliberately does **not** re-implement
//! any PTY spawning; it consumes `pty_read` output.

pub mod charsets;
pub mod common;
pub mod control;
pub mod escape;
pub mod graphics;
pub mod modes;
pub mod screens;
pub mod streams;
pub mod wcwidth;

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::AssertUnwindSafe;

use std::io::Write;

use anyhow::{anyhow, Result};
use base64::Engine;
use serde::Serialize;
use serde_json::ser::{CharEscape, Formatter, Serializer};
use serde_json::{json, Value};

use crate::common::{insert, remove, session_ids, with_session, Session};
use crate::screens::Screen;
use crate::streams::{Listener, Stream};

/// Run a handler that maps a parsed JSON `Value` to a JSON `Value`, converting
/// any error or panic into `{"error": "<msg>"}` so the stryke side can `die`.
/// Always returns a freshly allocated `CString`; the caller frees it via
/// [`stryke_free_cstring`].
fn ffi_call<F>(args: *const c_char, handler: F) -> *const c_char
where
    F: FnOnce(Value) -> Result<Value>,
{
    let input = if args.is_null() {
        Value::Null
    } else {
        // SAFETY: `args` comes from stryke's FFI bridge, which only passes
        // pointers into NUL-terminated CStrings it allocated for this call.
        let cs = unsafe { CStr::from_ptr(args) };
        serde_json::from_slice::<Value>(&sanitize_json_control_bytes(cs.to_bytes()))
            .unwrap_or(Value::Null)
    };
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| handler(input)));
    let out = match result {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => json!({ "error": e.to_string() }),
        Err(_) => json!({ "error": "stryke-terminal handler panicked" }),
    };
    let s = to_stryke_json(&out).unwrap_or_else(|_| String::from(r#"{"error":"serialize"}"#));
    match CString::new(s) {
        Ok(c) => c.into_raw() as *const c_char,
        Err(_) => std::ptr::null(),
    }
}

/// A `serde_json` formatter that writes control characters **raw** instead of
/// as `\uXXXX` / `\n` / `\t` escapes.
///
/// This is the output-side mirror of [`sanitize_json_control_bytes`]: stryke's
/// `from_json` does not decode `\uXXXX` (it keeps the six literal characters),
/// but it *does* read raw control bytes, because stryke's own `to_json` emits
/// them raw. Matching that convention makes device-report data from
/// [`terminal__take_input`] round-trip so a script can forward the exact bytes
/// back to the child with `pty_send`. NUL is the one exception: it is kept
/// escaped so the returned `CString` never contains an interior NUL (terminal
/// output never carries a meaningful NUL — pyte drops it).
struct RawControlFormatter;

impl Formatter for RawControlFormatter {
    fn write_char_escape<W>(
        &mut self,
        writer: &mut W,
        char_escape: CharEscape,
    ) -> std::io::Result<()>
    where
        W: ?Sized + Write,
    {
        match char_escape {
            CharEscape::Quote => writer.write_all(b"\\\""),
            CharEscape::ReverseSolidus => writer.write_all(b"\\\\"),
            CharEscape::Solidus => writer.write_all(b"/"),
            CharEscape::Backspace => writer.write_all(&[0x08]),
            CharEscape::FormFeed => writer.write_all(&[0x0c]),
            CharEscape::LineFeed => writer.write_all(&[0x0a]),
            CharEscape::CarriageReturn => writer.write_all(&[0x0d]),
            CharEscape::Tab => writer.write_all(&[0x09]),
            CharEscape::AsciiControl(0) => writer.write_all(b"\\u0000"),
            CharEscape::AsciiControl(b) => writer.write_all(&[b]),
        }
    }
}

/// Serialize a JSON value with control characters emitted raw, matching
/// stryke's JSON convention (see [`RawControlFormatter`]).
fn to_stryke_json(v: &Value) -> Result<String> {
    let mut buf = Vec::new();
    let mut ser = Serializer::with_formatter(&mut buf, RawControlFormatter);
    v.serialize(&mut ser)?;
    Ok(String::from_utf8(buf)?)
}

/// Escape raw control bytes (`< 0x20`) into their `\u00XX` JSON form.
///
/// Terminal data (the whole point of this package) is full of control bytes —
/// ESC, CR, LF, BEL. stryke's `to_json` embeds those bytes *raw* into the JSON
/// it hands us, which violates the JSON spec (control characters in strings
/// must be escaped) and makes a strict parser reject the entire payload. Raw
/// bytes `< 0x20` can only appear inside JSON string values, so rewriting each
/// one to `\u00XX` is lossless — it reconstructs the exact character the string
/// was meant to hold — while leaving structure, quoting, and already-escaped
/// sequences untouched. Multibyte UTF-8 bytes are all `>= 0x80`, so they pass
/// through unchanged.
fn sanitize_json_control_bytes(bytes: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        if b < 0x20 {
            out.extend_from_slice(b"\\u00");
            out.push(HEX[(b >> 4) as usize]);
            out.push(HEX[(b & 0x0f) as usize]);
        } else {
            out.push(b);
        }
    }
    out
}

/// Free a C string previously returned by any export from this cdylib.
///
/// # Safety
///
/// `p` must be a pointer returned by an export of this cdylib (a
/// `CString::into_raw` output) or null.
#[no_mangle]
pub unsafe extern "C" fn stryke_free_cstring(p: *mut c_char) {
    if p.is_null() {
        return;
    }
    drop(CString::from_raw(p));
}

// ── arg helpers ──────────────────────────────────────────────────────────

fn arg_session(v: &Value) -> Result<u64> {
    v.get("session")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("missing session id"))
}

fn arg_i64(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}

fn arg_str<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("missing string '{key}'"))
}

fn arg_char(v: &Value, key: &str) -> Result<char> {
    let s = arg_str(v, key)?;
    s.chars()
        .next()
        .ok_or_else(|| anyhow!("empty char for '{key}'"))
}

fn arg_i64_list(v: &Value, key: &str) -> Vec<i64> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default()
}

/// Run a screen command by name (wrapped in history before/after hooks) and
/// return `{"ok": true}`.
fn run_cmd<F: FnOnce(&mut Screen)>(v: &Value, name: &str, f: F) -> Result<Value> {
    let id = arg_session(v)?;
    with_session(id, |s| s.screen.event(name, f))?;
    Ok(json!({ "ok": true }))
}

// ── session lifecycle ────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn terminal__new(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let columns = arg_i64(&v, "columns").unwrap_or(80);
        let lines = arg_i64(&v, "lines").unwrap_or(24);
        let kind = v.get("kind").and_then(Value::as_str).unwrap_or("screen");
        let screen = if kind == "history" {
            let history = arg_i64(&v, "history").unwrap_or(100).max(0) as usize;
            let ratio = v.get("ratio").and_then(Value::as_f64).unwrap_or(0.5);
            Screen::with_history(columns, lines, history, ratio)
        } else {
            Screen::new(columns, lines)
        };
        let stream = Stream::new_bytes();
        let id = insert(Session { screen, stream });
        Ok(json!({ "session": id }))
    })
}

#[no_mangle]
pub extern "C" fn terminal__destroy(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        Ok(json!({ "destroyed": remove(id) }))
    })
}

#[no_mangle]
pub extern "C" fn terminal__sessions(args: *const c_char) -> *const c_char {
    ffi_call(args, |_v| Ok(json!(session_ids())))
}

#[no_mangle]
pub extern "C" fn terminal__reset(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| run_cmd(&v, "reset", |s| s.reset()))
}

#[no_mangle]
pub extern "C" fn terminal__resize(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let lines = arg_i64(&v, "lines");
        let columns = arg_i64(&v, "columns");
        with_session(id, |s| s.screen.resize(lines, columns))?;
        Ok(json!({ "ok": true }))
    })
}

// ── feeding ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn terminal__feed(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let data = arg_str(&v, "data")?.to_string();
        with_session(id, |s| s.feed_str(&data))?;
        Ok(json!({ "ok": true }))
    })
}

#[no_mangle]
pub extern "C" fn terminal__feed_bytes(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let b64 = arg_str(&v, "base64")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| anyhow!("invalid base64: {e}"))?;
        with_session(id, |s| s.feed_bytes(&bytes))?;
        Ok(json!({ "ok": true }))
    })
}

// ── reading ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn terminal__display(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let lines = with_session(id, |s| s.screen.display())?;
        Ok(json!(lines))
    })
}

#[no_mangle]
pub extern "C" fn terminal__line(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let y = arg_i64(&v, "y").unwrap_or(0);
        let lines = with_session(id, |s| s.screen.display())?;
        let line = lines.get(y as usize).cloned().unwrap_or_default();
        Ok(json!(line))
    })
}

#[no_mangle]
pub extern "C" fn terminal__cell(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let x = arg_i64(&v, "x").unwrap_or(0);
        let y = arg_i64(&v, "y").unwrap_or(0);
        let cell = with_session(id, |s| s.screen.cell(x, y))?;
        Ok(serde_json::to_value(cell)?)
    })
}

#[no_mangle]
pub extern "C" fn terminal__buffer(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let matrix = with_session(id, |s| {
            let mut rows = Vec::with_capacity(s.screen.lines as usize);
            for y in 0..s.screen.lines {
                let mut row = Vec::with_capacity(s.screen.columns as usize);
                for x in 0..s.screen.columns {
                    row.push(s.screen.cell(x, y));
                }
                rows.push(row);
            }
            rows
        })?;
        Ok(serde_json::to_value(matrix)?)
    })
}

#[no_mangle]
pub extern "C" fn terminal__cursor(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let cursor = with_session(id, |s| s.screen.cursor.clone())?;
        Ok(serde_json::to_value(cursor)?)
    })
}

#[no_mangle]
pub extern "C" fn terminal__dirty(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let dirty = with_session(id, |s| s.screen.dirty_sorted())?;
        Ok(json!(dirty))
    })
}

#[no_mangle]
pub extern "C" fn terminal__clear_dirty(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        with_session(id, |s| s.screen.dirty.clear())?;
        Ok(json!({ "ok": true }))
    })
}

#[no_mangle]
pub extern "C" fn terminal__title(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let title = with_session(id, |s| s.screen.title.clone())?;
        Ok(json!(title))
    })
}

#[no_mangle]
pub extern "C" fn terminal__icon_name(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let name = with_session(id, |s| s.screen.icon_name.clone())?;
        Ok(json!(name))
    })
}

#[no_mangle]
pub extern "C" fn terminal__mode(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let mode = with_session(id, |s| s.screen.mode_sorted())?;
        Ok(json!(mode))
    })
}

#[no_mangle]
pub extern "C" fn terminal__size(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let (columns, lines) = with_session(id, |s| (s.screen.columns, s.screen.lines))?;
        Ok(json!({ "columns": columns, "lines": lines }))
    })
}

#[no_mangle]
pub extern "C" fn terminal__take_input(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let out = with_session(id, |s| std::mem::take(&mut s.screen.outgoing))?;
        Ok(json!(out))
    })
}

// ── history ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn terminal__prev_page(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| run_cmd(&v, "prev_page", |s| s.prev_page()))
}

#[no_mangle]
pub extern "C" fn terminal__next_page(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| run_cmd(&v, "next_page", |s| s.next_page()))
}

#[no_mangle]
pub extern "C" fn terminal__history(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let id = arg_session(&v)?;
        let state = with_session(id, |s| s.screen.history_state())?;
        match state {
            Some((position, size, top, bottom)) => Ok(json!({
                "position": position,
                "size": size,
                "top": top,
                "bottom": bottom,
            })),
            None => Err(anyhow!("session has no history")),
        }
    })
}

// ── direct screen commands ───────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn terminal__draw(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let data = arg_str(&v, "data")?.to_string();
        run_cmd(&v, "draw", |s| s.draw(&data))
    })
}

macro_rules! count_cmd {
    ($fn_name:ident, $event:literal, $method:ident) => {
        #[no_mangle]
        pub extern "C" fn $fn_name(args: *const c_char) -> *const c_char {
            ffi_call(args, |v| {
                let count = arg_i64(&v, "count");
                run_cmd(&v, $event, |s| s.$method(count))
            })
        }
    };
}

count_cmd!(terminal__cursor_up, "cursor_up", cursor_up);
count_cmd!(terminal__cursor_down, "cursor_down", cursor_down);
count_cmd!(terminal__cursor_forward, "cursor_forward", cursor_forward);
count_cmd!(terminal__cursor_back, "cursor_back", cursor_back);
count_cmd!(terminal__cursor_up1, "cursor_up1", cursor_up1);
count_cmd!(terminal__cursor_down1, "cursor_down1", cursor_down1);
count_cmd!(terminal__insert_lines, "insert_lines", insert_lines);
count_cmd!(terminal__delete_lines, "delete_lines", delete_lines);
count_cmd!(
    terminal__insert_characters,
    "insert_characters",
    insert_characters
);
count_cmd!(
    terminal__delete_characters,
    "delete_characters",
    delete_characters
);
count_cmd!(
    terminal__erase_characters,
    "erase_characters",
    erase_characters
);

macro_rules! noarg_cmd {
    ($fn_name:ident, $event:literal, $method:ident) => {
        #[no_mangle]
        pub extern "C" fn $fn_name(args: *const c_char) -> *const c_char {
            ffi_call(args, |v| run_cmd(&v, $event, |s| s.$method()))
        }
    };
}

noarg_cmd!(
    terminal__carriage_return,
    "carriage_return",
    carriage_return
);
noarg_cmd!(terminal__index, "index", index);
noarg_cmd!(terminal__reverse_index, "reverse_index", reverse_index);
noarg_cmd!(terminal__linefeed, "linefeed", linefeed);
noarg_cmd!(terminal__tab, "tab", tab);
noarg_cmd!(terminal__backspace, "backspace", backspace);
noarg_cmd!(terminal__save_cursor, "save_cursor", save_cursor);
noarg_cmd!(terminal__restore_cursor, "restore_cursor", restore_cursor);
noarg_cmd!(terminal__set_tab_stop, "set_tab_stop", set_tab_stop);
noarg_cmd!(terminal__shift_in, "shift_in", shift_in);
noarg_cmd!(terminal__shift_out, "shift_out", shift_out);
noarg_cmd!(
    terminal__alignment_display,
    "alignment_display",
    alignment_display
);
noarg_cmd!(terminal__bell, "bell", bell);

#[no_mangle]
pub extern "C" fn terminal__cursor_position(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let line = arg_i64(&v, "line");
        let column = arg_i64(&v, "column");
        run_cmd(&v, "cursor_position", |s| s.cursor_position(line, column))
    })
}

#[no_mangle]
pub extern "C" fn terminal__cursor_to_column(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let column = arg_i64(&v, "column");
        run_cmd(&v, "cursor_to_column", |s| s.cursor_to_column(column))
    })
}

#[no_mangle]
pub extern "C" fn terminal__cursor_to_line(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let line = arg_i64(&v, "line");
        run_cmd(&v, "cursor_to_line", |s| s.cursor_to_line(line))
    })
}

#[no_mangle]
pub extern "C" fn terminal__erase_in_line(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let how = arg_i64(&v, "how").unwrap_or(0);
        run_cmd(&v, "erase_in_line", |s| s.erase_in_line(how))
    })
}

#[no_mangle]
pub extern "C" fn terminal__erase_in_display(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let how = arg_i64(&v, "how");
        run_cmd(&v, "erase_in_display", |s| s.erase_in_display(how, false))
    })
}

#[no_mangle]
pub extern "C" fn terminal__clear_tab_stop(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let how = arg_i64(&v, "how").unwrap_or(0);
        run_cmd(&v, "clear_tab_stop", |s| s.clear_tab_stop(how))
    })
}

#[no_mangle]
pub extern "C" fn terminal__set_mode(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let modes = arg_i64_list(&v, "modes");
        let private = v.get("private").and_then(Value::as_bool).unwrap_or(false);
        run_cmd(&v, "set_mode", |s| s.set_mode(&modes, private))
    })
}

#[no_mangle]
pub extern "C" fn terminal__reset_mode(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let modes = arg_i64_list(&v, "modes");
        let private = v.get("private").and_then(Value::as_bool).unwrap_or(false);
        run_cmd(&v, "reset_mode", |s| s.reset_mode(&modes, private))
    })
}

#[no_mangle]
pub extern "C" fn terminal__select_graphic_rendition(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let attrs = arg_i64_list(&v, "attrs");
        run_cmd(&v, "select_graphic_rendition", |s| {
            s.select_graphic_rendition(&attrs)
        })
    })
}

#[no_mangle]
pub extern "C" fn terminal__define_charset(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let code = arg_char(&v, "code")?;
        let mode = arg_char(&v, "mode")?;
        run_cmd(&v, "define_charset", |s| s.define_charset(code, mode))
    })
}

#[no_mangle]
pub extern "C" fn terminal__set_margins(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let top = arg_i64(&v, "top");
        let bottom = arg_i64(&v, "bottom");
        run_cmd(&v, "set_margins", |s| s.set_margins(top, bottom))
    })
}

#[no_mangle]
pub extern "C" fn terminal__report_device_attributes(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let mode = arg_i64(&v, "mode").unwrap_or(0);
        let private = v.get("private").and_then(Value::as_bool).unwrap_or(false);
        run_cmd(&v, "report_device_attributes", |s| {
            s.report_device_attributes(mode, private)
        })
    })
}

#[no_mangle]
pub extern "C" fn terminal__report_device_status(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let mode = arg_i64(&v, "mode").unwrap_or(0);
        run_cmd(&v, "report_device_status", |s| s.report_device_status(mode))
    })
}

// ── disassembler (pyte __main__ / DebugScreen) ───────────────────────────

/// Records parsed events as `[name, [args], {kwargs}]` triples, mirroring
/// pyte's `DebugEvent` JSON format.
struct Recorder {
    events: Vec<Value>,
}

impl Listener for Recorder {
    fn draw(&mut self, data: &str) {
        self.events.push(json!(["draw", [data], {}]));
    }
    fn dispatch(&mut self, name: &str, args: &[i64], private: bool) {
        let kwargs = if private {
            json!({ "private": true })
        } else {
            json!({})
        };
        self.events.push(json!([name, args, kwargs]));
    }
    fn define_charset(&mut self, code: char, mode: char) {
        self.events
            .push(json!(["define_charset", [code.to_string()], { "mode": mode.to_string() }]));
    }
    fn set_icon_name(&mut self, name: &str) {
        self.events.push(json!(["set_icon_name", [name], {}]));
    }
    fn set_title(&mut self, title: &str) {
        self.events.push(json!(["set_title", [title], {}]));
    }
}

#[no_mangle]
pub extern "C" fn terminal__dis(args: *const c_char) -> *const c_char {
    ffi_call(args, |v| {
        let data = arg_str(&v, "data")?;
        let mut stream = Stream::new_bytes();
        let mut rec = Recorder { events: Vec::new() };
        stream.feed_bytes(&mut rec, data.as_bytes());
        Ok(json!(rec.events))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// stryke embeds raw control bytes in the JSON it hands us; the sanitizer
    /// must escape them so a strict parser accepts the payload, and the parsed
    /// value must reconstruct the exact original bytes.
    #[test]
    fn sanitize_makes_raw_control_bytes_parseable() {
        // `{"data":"\x1b[2;5HX"}` with a *raw* ESC byte — invalid JSON.
        let raw = b"{\"data\":\"\x1b[2;5HX\"}";
        // Strict parse of the raw payload fails.
        assert!(serde_json::from_slice::<Value>(raw).is_err());
        // After sanitizing it parses, and the ESC round-trips.
        let fixed = sanitize_json_control_bytes(raw);
        let v: Value = serde_json::from_slice(&fixed).unwrap();
        assert_eq!(v["data"].as_str().unwrap(), "\u{1b}[2;5HX");
    }

    /// Non-control bytes (including multibyte UTF-8) pass through untouched.
    #[test]
    fn sanitize_leaves_normal_bytes_alone() {
        let input = "{\"data\":\"héllo 你\"}".as_bytes();
        assert_eq!(sanitize_json_control_bytes(input), input);
    }

    /// The output formatter emits control characters raw (matching stryke's
    /// `from_json`), not as `\uXXXX`, so device-report bytes forward verbatim.
    #[test]
    fn output_emits_raw_control_bytes() {
        let v = json!({ "reply": "\u{1b}[2;5R" });
        let s = to_stryke_json(&v).unwrap();
        // The ESC byte is present raw, not as the six chars ``.
        assert!(s.contains('\u{1b}'));
        assert!(!s.contains("\\u001b"));
        // Quote and backslash stay structurally escaped.
        let q = to_stryke_json(&json!({ "s": "a\"b\\c" })).unwrap();
        assert!(q.contains("\\\""));
        assert!(q.contains("\\\\"));
    }

    /// A full round-trip through both halves: sanitize-in then raw-out yields a
    /// payload stryke can read back, preserving the control bytes.
    #[test]
    fn control_bytes_round_trip_both_directions() {
        let raw_in = b"{\"data\":\"\x1b]2;t\x07\"}";
        let v: Value = serde_json::from_slice(&sanitize_json_control_bytes(raw_in)).unwrap();
        let out = to_stryke_json(&v).unwrap();
        assert!(out.contains('\u{1b}'));
        assert!(out.contains('\u{7}'));
    }
}
