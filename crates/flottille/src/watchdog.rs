//! The daemon's watchdog (`docs/reference/daemon.md`): a thread of its own, off the runtime,
//! that notices when the daemon stopped answering and writes to its log why it can see: the
//! locks held and waited for (`locks`), how long the runtime's thread has not run, the requests
//! that wait. Once for each time it is stuck, then again once it answers.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::locks;

/// How long the daemon may answer nothing, or its runtime not run, before it is said stuck.
const LIMIT: Duration = Duration::from_secs(30);
/// How often the runtime says it runs.
const BEAT: Duration = Duration::from_millis(500);

/// What the runtime and the connections tell the watchdog, read from its thread.
pub struct Watch {
    started: Instant,
    /// When the runtime last ran, in ms after `started`.
    beat: AtomicU64,
    /// The requests read and not answered yet.
    waiting: AtomicUsize,
    /// When a request was last answered, or the first of those waiting came, in ms after
    /// `started`.
    progress: AtomicU64,
}

impl Default for Watch {
    fn default() -> Watch {
        Watch {
            started: Instant::now(),
            beat: AtomicU64::new(0),
            waiting: AtomicUsize::new(0),
            progress: AtomicU64::new(0),
        }
    }
}

/// A request read and not answered yet: answered when it goes.
pub struct Asked(Arc<Watch>);

impl Drop for Asked {
    fn drop(&mut self) {
        self.0.waiting.fetch_sub(1, Ordering::SeqCst);
        self.0.progress.store(self.0.now(), Ordering::SeqCst);
    }
}

impl Watch {
    fn now(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// A request was read; it is answered when the guard goes.
    pub fn asked(self: &Arc<Watch>) -> Asked {
        if self.waiting.fetch_add(1, Ordering::SeqCst) == 0 {
            self.progress.store(self.now(), Ordering::SeqCst);
        }
        Asked(self.clone())
    }

    /// Why the daemon is stuck now, past `limit`; `None` while it answers.
    fn stuck(&self, limit: Duration) -> Option<String> {
        let now = self.now();
        let limit_ms = u64::try_from(limit.as_millis()).unwrap_or(u64::MAX);
        let quiet = now.saturating_sub(self.beat.load(Ordering::SeqCst));
        let waiting = self.waiting.load(Ordering::SeqCst);
        let unanswered = now.saturating_sub(self.progress.load(Ordering::SeqCst));
        let ms = |ms: u64| locks::seconds(Duration::from_millis(ms));
        if quiet > limit_ms {
            Some(format!(
                "the runtime's thread has not run for {}, {waiting} request(s) wait",
                ms(quiet)
            ))
        } else if waiting > 0 && unanswered > limit_ms {
            Some(format!(
                "{waiting} request(s) wait and none was answered for {}; the runtime's thread \
                 ran {} ago",
                ms(unanswered),
                ms(quiet)
            ))
        } else {
            None
        }
    }
}

fn limit() -> Duration {
    std::env::var("FLOTTILLE_WATCHDOG_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .map_or(LIMIT, Duration::from_millis)
}

/// Starts the beat on the runtime and the watchdog's thread, for the daemon's whole life.
pub fn start(watch: &Arc<Watch>) -> std::io::Result<()> {
    let limit = limit();
    let beating = watch.clone();
    tokio::spawn(async move {
        let mut beat = tokio::time::interval(BEAT.min(limit / 4));
        loop {
            beat.tick().await;
            beating.beat.store(beating.now(), Ordering::SeqCst);
        }
    });
    let runtime = tokio::runtime::Handle::current();
    let watch = watch.clone();
    std::thread::Builder::new()
        .name("flottille-watchdog".into())
        .spawn(move || {
            let mut said = false;
            loop {
                std::thread::sleep(limit / 4);
                match watch.stuck(limit) {
                    Some(why) if !said => {
                        said = true;
                        eprint!("{}", report(&why, &runtime));
                    }
                    Some(_) => {}
                    None if said => {
                        said = false;
                        eprintln!("flottille daemon: watchdog: the daemon answers again");
                    }
                    None => {}
                }
            }
        })
        .map(drop)
}

fn report(why: &str, runtime: &tokio::runtime::Handle) -> String {
    let pid = std::process::id();
    let mut text = format!("flottille daemon: watchdog: the daemon is stuck: {why}\n");
    text.push_str(&format!(
        "flottille daemon: watchdog: the runtime has {} task(s) alive\n",
        runtime.metrics().num_alive_tasks()
    ));
    for line in locks::report() {
        text.push_str(&format!("flottille daemon: watchdog: {line}\n"));
    }
    text.push_str(&format!(
        "flottille daemon: watchdog: before ending it, its threads' stacks: `sudo eu-stack -p {pid}` \
         or `sudo gdb -p {pid} -batch -ex \"thread apply all bt\"`\n"
    ));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daemon_that_answers_is_not_stuck_and_one_that_waits_past_the_limit_is() {
        let watch = Arc::new(Watch::default());
        let limit = Duration::from_millis(50);
        watch.beat.store(watch.now(), Ordering::SeqCst);
        assert_eq!(watch.stuck(limit), None);

        let asked = watch.asked();
        std::thread::sleep(limit * 2);
        watch.beat.store(watch.now(), Ordering::SeqCst);
        let why = watch.stuck(limit).expect("a request waits past the limit");
        assert!(why.starts_with("1 request(s) wait"), "{why}");

        drop(asked);
        assert_eq!(watch.stuck(limit), None);

        std::thread::sleep(limit * 2);
        let why = watch.stuck(limit).expect("the runtime did not run");
        assert!(why.starts_with("the runtime's thread has not run"), "{why}");
    }
}
