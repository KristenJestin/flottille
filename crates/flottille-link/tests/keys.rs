//! The keys of the user's devices (`docs/remote.md`, "Identity and keys"): the machine key a
//! daemon makes, the device key its clients make, in temporary folders and an in-memory keyring.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use flottille_link::{DeviceKey, Keyring, KeyringError, MachineKey};
use zeroize::Zeroizing;

/// A temporary `$XDG_STATE_HOME`: the key folder Flottille's would be, not yet made.
fn state() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("state/flottille/remote");
    (dir, folder)
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// An item's attributes, sorted.
type Attributes = Vec<(String, String)>;

/// The session's keyring, in memory: its items by their attributes, with their labels.
#[derive(Default)]
struct Memory {
    items: RefCell<HashMap<Attributes, (String, Vec<u8>)>>,
    /// Plays a keyring that is locked: every use fails, and nothing may go to a file instead.
    locked: bool,
}

fn attributes_of(attributes: &[(&str, &str)]) -> Attributes {
    let mut owned: Attributes = attributes
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    owned.sort();
    owned
}

impl Keyring for Memory {
    fn read(
        &self,
        attributes: &[(&str, &str)],
    ) -> Result<Option<Zeroizing<Vec<u8>>>, KeyringError> {
        if self.locked {
            return Err(KeyringError::Failed("the keyring is locked".into()));
        }
        Ok(self
            .items
            .borrow()
            .get(&attributes_of(attributes))
            .map(|(_, secret)| Zeroizing::new(secret.clone())))
    }

    fn store(
        &self,
        label: &str,
        attributes: &[(&str, &str)],
        secret: &[u8],
    ) -> Result<(), KeyringError> {
        if self.locked {
            return Err(KeyringError::Failed("the keyring is locked".into()));
        }
        self.items.borrow_mut().insert(
            attributes_of(attributes),
            (label.to_owned(), secret.to_vec()),
        );
        Ok(())
    }
}

/// A machine with no Secret Service: a headless server.
struct NoKeyring;

impl Keyring for NoKeyring {
    fn read(&self, _: &[(&str, &str)]) -> Result<Option<Zeroizing<Vec<u8>>>, KeyringError> {
        Err(KeyringError::Absent)
    }

    fn store(&self, _: &str, _: &[(&str, &str)], _: &[u8]) -> Result<(), KeyringError> {
        Err(KeyringError::Absent)
    }
}

#[test]
fn a_machine_key_is_made_once_then_read_back() {
    let (_dir, folder) = state();

    let made = MachineKey::load_or_make(&folder).unwrap();
    let file = folder.join("machine.key");
    let written = std::fs::metadata(&file).unwrap();
    let again = MachineKey::load_or_make(&folder).unwrap();

    assert_eq!(made.public(), again.public());
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(&folder), 0o700);
    assert_eq!(written.len(), 32);
    // Read back, never written again.
    let after = std::fs::metadata(&file).unwrap();
    assert_eq!(written.ino(), after.ino());
    assert_eq!(written.mtime_nsec(), after.mtime_nsec());
    // Nothing else is left in the folder: no temporary file.
    let names: Vec<_> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["machine.key"]);
}

#[test]
fn two_machine_keys_are_two_identities() {
    let (_one, first) = state();
    let (_other, second) = state();

    let one = MachineKey::load_or_make(&first).unwrap().public();
    let other = MachineKey::load_or_make(&second).unwrap().public();

    assert_ne!(one, other);
    assert_ne!(one.fingerprint(), other.fingerprint());
}

#[test]
fn a_fingerprint_is_short_and_printable() {
    let (_dir, folder) = state();
    let key = MachineKey::load_or_make(&folder).unwrap().public();

    let fingerprint = key.fingerprint();

    assert_eq!(fingerprint, key.fingerprint());
    assert_eq!(fingerprint.len(), 19, "{fingerprint}");
    assert!(
        fingerprint
            .chars()
            .all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "{fingerprint}"
    );
    assert_eq!(fingerprint.matches('-').count(), 3, "{fingerprint}");
}

#[test]
fn a_machine_key_open_to_others_is_refused_and_left_as_it_is() {
    for (wide, said) in [
        (0o644, "readable"),
        (0o640, "readable"),
        (0o620, "writable"),
    ] {
        let (_dir, folder) = state();
        MachineKey::load_or_make(&folder).unwrap();
        let file = folder.join("machine.key");
        let before = std::fs::read(&file).unwrap();
        std::fs::set_permissions(&file, PermissionsExt::from_mode(wide)).unwrap();

        let refused = MachineKey::load_or_make(&folder).unwrap_err().to_string();

        assert!(
            refused.contains(&format!("{} is {said} by others", file.display())),
            "{refused}"
        );
        assert!(
            refused.contains(&format!("chmod 600 {}", file.display())),
            "{refused}"
        );
        assert_eq!(
            std::fs::read(&file).unwrap(),
            before,
            "the key was replaced"
        );
        assert_eq!(mode(&file), wide);
    }
}

#[test]
fn a_key_folder_open_to_others_is_refused() {
    let (_dir, folder) = state();
    MachineKey::load_or_make(&folder).unwrap();
    std::fs::set_permissions(&folder, PermissionsExt::from_mode(0o755)).unwrap();

    let refused = MachineKey::load_or_make(&folder).unwrap_err().to_string();

    assert!(refused.contains("chmod 700"), "{refused}");
}

#[test]
fn a_machine_key_that_is_a_link_or_not_a_key_is_refused() {
    let (dir, folder) = state();
    MachineKey::load_or_make(&folder).unwrap();
    let file = folder.join("machine.key");
    // A link to a key elsewhere: whoever made the link chooses the machine's identity.
    let elsewhere = dir.path().join("elsewhere.key");
    std::fs::rename(&file, &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
    let refused = MachineKey::load_or_make(&folder).unwrap_err().to_string();
    assert!(refused.contains("link"), "{refused}");

    std::fs::remove_file(&file).unwrap();
    std::fs::write(&file, b"short").unwrap();
    std::fs::set_permissions(&file, PermissionsExt::from_mode(0o600)).unwrap();
    let refused = MachineKey::load_or_make(&folder).unwrap_err().to_string();
    assert!(refused.contains("not a key"), "{refused}");
    assert!(!refused.contains("short"), "{refused}");
}

#[test]
fn a_machine_key_that_is_a_pipe_is_refused_without_waiting() {
    let (_dir, folder) = state();
    MachineKey::load_or_make(&folder).unwrap();
    let file = folder.join("machine.key");
    std::fs::remove_file(&file).unwrap();
    let path = CString::new(file.as_os_str().as_bytes()).unwrap();
    // SAFETY: `path` is a valid C string that outlives the call.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);

    // An open that waits for a writer would never answer: the daemon would hang at its start.
    let (sent, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let loaded = MachineKey::load_or_make(&folder).map(|_| ());
        sent.send(loaded.map_err(|error| error.to_string()))
            .unwrap();
    });
    let refused = received
        .recv_timeout(Duration::from_secs(10))
        .expect("the load waited on the pipe")
        .unwrap_err();
    assert!(refused.contains("not a file"), "{refused}");
}

#[test]
fn a_temporary_name_a_crash_left_beside_a_key_is_removed() {
    let (_dir, folder) = state();
    MachineKey::load_or_make(&folder).unwrap();
    let file = folder.join("machine.key");
    // A crash between the link and the removal leaves the key a second name. No process has
    // this id: it is above any `pid_max`.
    let left = folder.join(".machine.key.2000000000.0.new");
    std::fs::hard_link(&file, &left).unwrap();
    // This process's own may be another thread's key being made: it is left.
    let making = folder.join(format!(".machine.key.{}.7.new", std::process::id()));
    std::fs::write(&making, b"being made").unwrap();
    let other = folder.join(".machine.key.notes");
    std::fs::write(&other, b"not a temporary name").unwrap();

    MachineKey::load_or_make(&folder).unwrap();

    assert!(!left.exists(), "the key's second name is still there");
    assert_eq!(std::fs::metadata(&file).unwrap().nlink(), 1);
    assert!(making.exists());
    assert!(other.exists());
}

#[test]
fn the_device_key_goes_to_the_keyring_or_to_a_private_file_without_one() {
    // With a keyring: an item `Flottille: device`, and nothing on the disk.
    let (_dir, folder) = state();
    let keyring = Memory::default();
    let made = DeviceKey::load_or_make(&keyring, &folder).unwrap();
    let again = DeviceKey::load_or_make(&keyring, &folder).unwrap();
    assert_eq!(made.public(), again.public());
    {
        let items = keyring.items.borrow();
        assert_eq!(items.len(), 1);
        let (attributes, (label, secret)) = items.iter().next().unwrap();
        assert_eq!(label, "Flottille: device");
        assert!(
            attributes.contains(&("application".to_owned(), "flottille".to_owned())),
            "{attributes:?}"
        );
        // Never a vault entry's attribute: a vault entry named `device` is another item.
        assert!(
            attributes.iter().all(|(name, _)| name != "flottille-entry"),
            "{attributes:?}"
        );
        assert_eq!(secret.len(), 32);
    }
    assert!(
        !folder.exists(),
        "the key folder was made beside the keyring"
    );

    // Without a Secret Service: `device.key`, mode 600, in a folder of mode 700.
    let (_dir, folder) = state();
    let made = DeviceKey::load_or_make(&NoKeyring, &folder).unwrap();
    let file = folder.join("device.key");
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(&folder), 0o700);
    assert_eq!(std::fs::metadata(&file).unwrap().len(), 32);
    let again = DeviceKey::load_or_make(&NoKeyring, &folder).unwrap();
    assert_eq!(made.public(), again.public());
    // Refused as the machine key is when others may read it.
    std::fs::set_permissions(&file, PermissionsExt::from_mode(0o644)).unwrap();
    let refused = DeviceKey::load_or_make(&NoKeyring, &folder)
        .unwrap_err()
        .to_string();
    assert!(refused.contains("chmod 600"), "{refused}");

    // A keyring that is there but fails (locked) is said, never worked around with a file: a
    // second identity of the same device would be made.
    let (_dir, folder) = state();
    let locked = Memory {
        locked: true,
        ..Memory::default()
    };
    let refused = DeviceKey::load_or_make(&locked, &folder)
        .unwrap_err()
        .to_string();
    assert!(refused.contains("locked"), "{refused}");
    assert!(!folder.exists());
}

#[test]
fn a_private_key_is_never_printed() {
    let (_dir, folder) = state();
    let machine = MachineKey::load_or_make(&folder).unwrap();
    let seed = std::fs::read(folder.join("machine.key")).unwrap();
    let keyring = Memory::default();
    let device = DeviceKey::load_or_make(&keyring, &folder).unwrap();
    let device_seed = keyring.items.borrow().values().next().unwrap().1.clone();

    for (printed, seed) in [
        (format!("{machine:?} {machine:#?}"), &seed),
        (format!("{device:?} {device:#?}"), &device_seed),
    ] {
        let hex: String = seed.iter().map(|byte| format!("{byte:02x}")).collect();
        let upper = hex.to_uppercase();
        let listed = format!("{seed:?}");
        let listed = &listed[1..listed.len() - 1];
        let first = seed[..4]
            .iter()
            .map(|byte| byte.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        for shown in [&hex[..8], &upper[..8], listed, &first] {
            assert!(!printed.contains(shown), "{printed} shows {shown}");
        }
        // What it does print: whose key it is.
        assert!(printed.contains("Key"), "{printed} does not say what it is");
    }
    assert!(format!("{machine:?}").contains(&machine.public().fingerprint()));
    assert!(format!("{device:?}").contains(&device.public().fingerprint()));
}
