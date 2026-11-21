//! A panic of the core is kept for the app's next start: a binary of its own, since the hook it
//! sets is the process's.

use flottille_mobile::{PANIC_FILE, keep_panics, last_panic};

#[test]
fn a_panic_is_kept_for_the_next_start() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    assert_eq!(last_panic(folder.clone()), None);

    keep_panics(folder.clone());
    let crashed = std::thread::spawn(|| panic!("the core broke on a frame")).join();
    assert!(crashed.is_err());
    assert!(dir.path().join(PANIC_FILE).exists());

    // At the next start, the app reads it once, with where it happened.
    let kept = last_panic(folder.clone()).expect("the panic kept");
    assert!(kept.contains("the core broke on a frame"), "{kept}");
    assert!(kept.contains("mobile_panic.rs"), "{kept}");
    assert_eq!(last_panic(folder), None);
}
