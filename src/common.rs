//! Process-global registry of terminal sessions.
//!
//! A stryke script creates a session with `Terminal::new`, feeds bytes across
//! many calls, and reads the rendered screen — so each session's [`Screen`] +
//! [`Stream`] must persist in-process between FFI calls, exactly like
//! stryke-selenium keeps WebDriver handles alive. Sessions are keyed by a
//! monotonic `u64` id.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use once_cell::sync::OnceCell;

use crate::screens::Screen;
use crate::streams::Stream;

/// One terminal session: a screen model and the parser feeding it.
pub struct Session {
    pub screen: Screen,
    pub stream: Stream,
}

impl Session {
    /// Feed raw bytes through the parser into the screen (ByteStream path).
    pub fn feed_bytes(&mut self, data: &[u8]) {
        self.stream.feed_bytes(&mut self.screen, data);
    }

    /// Feed text through the parser into the screen (Stream path).
    pub fn feed_str(&mut self, data: &str) {
        self.stream.feed_str(&mut self.screen, data);
    }
}

fn registry() -> &'static Mutex<HashMap<u64, Session>> {
    static REGISTRY: OnceCell<Mutex<HashMap<u64, Session>>> = OnceCell::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Insert a new session and return its id.
pub fn insert(session: Session) -> u64 {
    let id = next_id();
    registry().lock().unwrap().insert(id, session);
    id
}

/// Remove a session, returning whether it existed.
pub fn remove(id: u64) -> bool {
    registry().lock().unwrap().remove(&id).is_some()
}

/// Sorted list of live session ids.
pub fn session_ids() -> Vec<u64> {
    let mut ids: Vec<u64> = registry().lock().unwrap().keys().copied().collect();
    ids.sort_unstable();
    ids
}

/// Run `f` against a session by id, returning its result, or an error if the id
/// is unknown.
pub fn with_session<R>(id: u64, f: impl FnOnce(&mut Session) -> R) -> anyhow::Result<R> {
    let mut guard = registry().lock().unwrap();
    let session = guard
        .get_mut(&id)
        .ok_or_else(|| anyhow::anyhow!("unknown terminal session {id}"))?;
    Ok(f(session))
}
