//! What the daemon reads of a forge or a tracker for a call: how many command lines ran and how
//! long they took (the developer log says it, `docs/developer.md`), and the short cache that
//! gives the next call what was just read (`docs/forges.md`, "How fast it is read").

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flottille_core::store::now_ms;
use serde_json::{Value, json};

/// How long what was read is given again: a pull request or a ticket is read again after it.
pub const KEPT_MS: u64 = 30_000;

#[derive(Default)]
struct Counted {
    ran: u64,
    total_ms: u64,
    slowest_ms: u64,
    cached: bool,
}

tokio::task_local! {
    static CURRENT: Reads;
}

/// The reads of one call. Its tool scopes it (`counting`), and the command lines `gh`, `bkt` and
/// `acli` note themselves in the one of the task they run in: reads run together by `join!` are
/// in that task, and a read nobody scoped is not counted.
#[derive(Clone, Default)]
pub struct Reads(Arc<Mutex<Counted>>);

impl Reads {
    /// Runs `work` with this as the reads it counts.
    pub async fn counting<F: Future>(&self, work: F) -> F::Output {
        CURRENT.scope(self.clone(), work).await
    }

    /// One command line that took `time`, noted in the reads of the call it belongs to.
    pub fn ran(time: Duration) {
        let _ = CURRENT.try_with(|reads| {
            let mut counted = reads.0.lock().unwrap();
            let ms = time.as_millis() as u64;
            counted.ran += 1;
            counted.total_ms += ms;
            counted.slowest_ms = counted.slowest_ms.max(ms);
        });
    }

    /// The call was given what an earlier one read.
    pub fn from_cache() {
        let _ = CURRENT.try_with(|reads| reads.0.lock().unwrap().cached = true);
    }

    /// What the developer log says: how many reads ran, the sum of their durations and the
    /// slowest (the call's own duration is the sum when they ran one after the other, the slowest
    /// when they ran together), and whether the cache answered. None for a call that read nothing.
    pub fn to_json(&self) -> Option<Value> {
        let counted = self.0.lock().unwrap();
        (counted.ran > 0 || counted.cached).then(|| {
            json!({"ran": counted.ran, "total_ms": counted.total_ms,
                "slowest_ms": counted.slowest_ms, "cached": counted.cached})
        })
    }
}

/// What was read, by key, for `KEPT_MS`. A write drops it all; a read that began before that drop
/// does not put back what it read (`generation`).
pub struct Kept<T> {
    inner: Mutex<Inner<T>>,
}

struct Inner<T> {
    generation: u64,
    entries: HashMap<String, (u64, T)>,
}

impl<T> Default for Kept<T> {
    fn default() -> Self {
        Kept {
            inner: Mutex::new(Inner {
                generation: 0,
                entries: HashMap::new(),
            }),
        }
    }
}

impl<T: Clone> Kept<T> {
    /// What was read for `key` less than `KEPT_MS` ago, and how many milliseconds ago.
    pub fn get(&self, key: &str) -> Option<(T, u64)> {
        let now = now_ms();
        let mut inner = self.inner.lock().unwrap();
        inner
            .entries
            .retain(|_, (at, _)| now.saturating_sub(*at) < KEPT_MS);
        inner
            .entries
            .get(key)
            .map(|(at, kept)| (kept.clone(), now.saturating_sub(*at)))
    }

    /// The generation to hand to `put`, taken before the read begins.
    pub fn generation(&self) -> u64 {
        self.inner.lock().unwrap().generation
    }

    /// Keeps `value` for `key`, unless something was dropped since `generation`.
    pub fn put(&self, key: String, value: T, generation: u64) {
        let mut inner = self.inner.lock().unwrap();
        if inner.generation == generation {
            inner.entries.insert(key, (now_ms(), value));
        }
    }

    /// Drops everything kept, and keeps what the reads that are running find: they began before
    /// what changed.
    pub fn drop_all(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.generation += 1;
        inner.entries.clear();
    }
}

/// The age of a cached read, as the result says it.
pub fn age_text(age_ms: u64) -> String {
    format!("as read {} s ago", age_ms / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drop_stops_the_read_that_began_before_it_from_being_kept() {
        let kept: Kept<u8> = Kept::default();
        kept.put("acme/api#12".into(), 1, kept.generation());
        let began = kept.generation();
        kept.drop_all();

        kept.put("acme/api#12".into(), 1, began);

        assert!(kept.get("acme/api#12").is_none());
        kept.put("acme/api#12".into(), 2, kept.generation());
        assert_eq!(kept.get("acme/api#12").map(|(value, _)| value), Some(2));
    }
}
