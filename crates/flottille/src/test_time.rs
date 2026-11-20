//! The waits of the daemon that a test would otherwise sit through: a site that never answers, a
//! process that never stops. In a test build a test shortens one by a `FLOTTILLE_TEST_*_MS`
//! variable of the daemon it starts; a release build reads nothing and keeps the production
//! value, as every other `FLOTTILLE_TEST_*` hook.

use std::time::Duration;

/// `production`, or, in a test build, the milliseconds the variable `name` says.
pub fn tuned(name: &str, production: Duration) -> Duration {
    cfg!(debug_assertions)
        .then(|| std::env::var(name).ok())
        .flatten()
        .and_then(|ms| ms.parse().ok())
        .map_or(production, Duration::from_millis)
}
