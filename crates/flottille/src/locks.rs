//! The daemon's locks, named: a `std::sync::Mutex` that knows, while it is held, which thread
//! holds it, from which line and since when, and who waits for it. The watchdog says it when the
//! daemon stops answering (`watchdog`), which no stack taken after the fact tells as plainly.
//!
//! The daemon's runtime has one thread: a lock taken again by the thread that holds it, by the
//! same task or by another one, waits for ever. That is said in the log the moment it happens.

use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};
use std::panic::Location;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LockResult, Mutex, MutexGuard, PoisonError, TryLockError};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// A `Mutex` that says who holds it. `lock` answers as the standard one does. One made by
/// `Default` has no name: its holders' lines say which it is.
#[derive(Default)]
pub struct Lock<T> {
    name: &'static str,
    inner: Mutex<T>,
}

/// A lock held, or waited for: where and since when.
#[derive(Clone)]
struct Taker {
    lock: &'static str,
    thread: Thread,
    at: &'static Location<'static>,
    since: Instant,
}

/// What is held and waited for now, by lock (its address) and by ticket. Held only to read or
/// write it, never while anything waits.
struct Table {
    held: BTreeMap<usize, Taker>,
    waiting: BTreeMap<u64, (usize, Taker)>,
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    held: BTreeMap::new(),
    waiting: BTreeMap::new(),
});
static TICKETS: AtomicU64 = AtomicU64::new(0);

fn table() -> MutexGuard<'static, Table> {
    TABLE.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<T> Lock<T> {
    pub const fn named(name: &'static str, value: T) -> Lock<T> {
        Lock {
            name,
            inner: Mutex::new(value),
        }
    }

    fn key(&self) -> usize {
        std::ptr::from_ref(self).addr()
    }

    #[track_caller]
    pub fn lock(&self) -> LockResult<Guard<'_, T>> {
        let me = Taker {
            lock: self.name,
            thread: std::thread::current(),
            at: Location::caller(),
            since: Instant::now(),
        };
        let taken = match self.inner.try_lock() {
            Ok(guard) => Ok(guard),
            Err(TryLockError::Poisoned(poisoned)) => Err(poisoned),
            Err(TryLockError::WouldBlock) => {
                let ticket = self.wait(&me);
                let taken = self.inner.lock();
                table().waiting.remove(&ticket);
                taken
            }
        };
        let held = |inner| {
            table().held.insert(self.key(), me.clone());
            Guard { lock: self, inner }
        };
        match taken {
            Ok(inner) => Ok(held(inner)),
            Err(poisoned) => Err(PoisonError::new(held(poisoned.into_inner()))),
        }
    }

    /// Says that `me` waits, and, when its own thread holds the lock, that it waits for ever.
    fn wait(&self, me: &Taker) -> u64 {
        let ticket = TICKETS.fetch_add(1, Ordering::Relaxed);
        let mut table = table();
        let itself = table
            .held
            .get(&self.key())
            .filter(|holder| holder.thread.id() == me.thread.id())
            .map(|holder| holder.at);
        table.waiting.insert(ticket, (self.key(), me.clone()));
        drop(table);
        if let Some(held_at) = itself {
            eprintln!(
                "flottille daemon: the lock {} is taken at {} by the thread that holds it since \
                 {}: it waits for itself, and the daemon with it",
                shown(me.lock),
                me.at,
                held_at
            );
        }
        ticket
    }
}

/// A held `Lock`: what the standard guard is, and its holder forgotten when it goes.
pub struct Guard<'a, T> {
    lock: &'a Lock<T>,
    inner: MutexGuard<'a, T>,
}

impl<T> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        table().held.remove(&self.lock.key());
    }
}

impl<T> Deref for Guard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

fn shown(name: &str) -> &str {
    if name.is_empty() { "(unnamed)" } else { name }
}

fn line(what: &str, taker: &Taker, now: Instant) -> String {
    let thread = taker.thread.name().map_or_else(
        || format!("{:?}", taker.thread.id()),
        |name| format!("{name} ({:?})", taker.thread.id()),
    );
    format!(
        "{what} {} at {} by the thread {thread}, for {}",
        shown(taker.lock),
        taker.at,
        seconds(now.saturating_duration_since(taker.since))
    )
}

pub fn seconds(duration: Duration) -> String {
    format!("{:.1} s", duration.as_secs_f64())
}

/// Every lock held and waited for now, the longest first, one per line.
pub fn report() -> Vec<String> {
    let now = Instant::now();
    let table = table();
    let mut held: Vec<&Taker> = table.held.values().collect();
    held.sort_by_key(|taker| taker.since);
    let mut waiting: Vec<&Taker> = table.waiting.values().map(|(_, taker)| taker).collect();
    waiting.sort_by_key(|taker| taker.since);
    let mut lines: Vec<String> = held.iter().map(|t| line("held:", t, now)).collect();
    lines.extend(waiting.iter().map(|t| line("waited for:", t, now)));
    if lines.is_empty() {
        lines.push("no lock of the daemon is held".to_owned());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mine(lines: &[String], name: &str) -> Vec<String> {
        lines
            .iter()
            .filter(|line| line.contains(&format!(" {name} at ")))
            .cloned()
            .collect()
    }

    #[test]
    fn a_held_lock_says_where_and_by_which_thread_and_forgets_it_once_let_go() {
        let lock = Lock::named("test-held", 1);
        let guard = lock.lock().unwrap();
        let held = mine(&report(), "test-held");
        assert_eq!(held.len(), 1, "{held:?}");
        assert!(held[0].starts_with("held: test-held at "), "{}", held[0]);
        assert!(held[0].contains("locks.rs:"), "{}", held[0]);
        assert!(held[0].contains(&format!("{:?}", std::thread::current().id())));
        drop(guard);
        assert!(mine(&report(), "test-held").is_empty());
    }

    #[test]
    fn a_waiter_is_said_while_it_waits() {
        let lock = std::sync::Arc::new(Lock::named("test-waited", ()));
        let guard = lock.lock().unwrap();
        let other = lock.clone();
        let waiter = std::thread::spawn(move || drop(other.lock().unwrap()));
        let deadline = Instant::now() + Duration::from_secs(10);
        while mine(&report(), "test-waited").len() < 2 {
            assert!(Instant::now() < deadline, "the waiter is never said");
            std::thread::sleep(Duration::from_millis(5));
        }
        let lines = mine(&report(), "test-waited");
        assert!(lines[0].starts_with("held: "), "{lines:?}");
        assert!(lines[1].starts_with("waited for: "), "{lines:?}");
        drop(guard);
        waiter.join().unwrap();
        assert!(mine(&report(), "test-waited").is_empty());
    }

    #[test]
    fn a_poisoned_lock_answers_as_the_standard_one_and_is_forgotten_too() {
        let lock = std::sync::Arc::new(Lock::named("test-poisoned", 0));
        let other = lock.clone();
        let _ = std::thread::spawn(move || {
            let _held = other.lock().unwrap();
            panic!("poisons the lock");
        })
        .join();
        let mut guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        *guard += 1;
        drop(guard);
        assert!(mine(&report(), "test-poisoned").is_empty());
    }
}
