//! The desktop entry and the icon (#302): `install.sh` and `flottille update` write them into
//! the user's XDG data folder. A local server stands for the releases, in a throwaway home and
//! data folder: nothing here touches the real ones, nor the network. (`release.rs` has its own
//! server and is not run on a developer's machine; this file is small enough to be.)

mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use common::User;

/// The repository's own files.
fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

/// The files of a local server standing for GitHub, by path.
struct Releases {
    base: String,
    files: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>>,
}

impl Releases {
    fn serve() -> Releases {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let files: Arc<Mutex<std::collections::HashMap<String, Vec<u8>>>> = Arc::default();
        let served = files.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                        break;
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let body = served.lock().unwrap().get(&path).cloned();
                let _ = match body {
                    Some(body) => write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .and_then(|()| stream.write_all(&body)),
                    None => write!(
                        stream,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    ),
                };
            }
        });
        Releases { base, files }
    }

    fn api(&self) -> String {
        format!("{}/repos/o/r", self.base)
    }

    fn put(&self, path: &str, body: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_owned(), body);
    }

    /// A stable release at `version`, its archive made by `tools/package.sh` from a program that
    /// says that version, so that it holds what a real one holds.
    fn stable(&self, version: &str) {
        let programs = tempfile::tempdir().unwrap();
        let program = programs.path().join("flottille");
        std::fs::write(
            &program,
            format!("#!/bin/sh\necho \"flottille {version} (test, 000000000000)\"\n"),
        )
        .unwrap();
        std::fs::set_permissions(
            &program,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        let out = tempfile::tempdir().unwrap();
        let made = Command::new("bash")
            .arg(repo("tools/package.sh"))
            .args([version])
            .arg(&program)
            .arg(out.path())
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let name = format!("flottille-{version}-x86_64-linux.tar.gz");
        let tag = format!("v{version}");
        self.put(
            &format!("/download/{tag}/{name}"),
            std::fs::read(out.path().join(&name)).unwrap(),
        );
        self.put(
            &format!("/download/{tag}/{name}.sha256"),
            std::fs::read(out.path().join(format!("{name}.sha256"))).unwrap(),
        );
        let url = format!("{}/download/{tag}/{name}", self.base);
        self.put(
            "/repos/o/r/releases/latest",
            serde_json::json!({"tag_name": tag, "assets": [{"browser_download_url": url}]})
                .to_string()
                .into_bytes(),
        );
    }
}

/// `install.sh` for a throwaway user: `home`, and `data` as `XDG_DATA_HOME` when given.
fn install(home: &Path, data: Option<&Path>, releases: &Releases) -> Output {
    let mut command = Command::new("sh");
    command
        .arg(repo("install.sh"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", home)
        .env("FLOTTILLE_RELEASES", releases.api());
    if let Some(data) = data {
        command.env("XDG_DATA_HOME", data);
    }
    command.output().unwrap()
}

/// The icon files a package holds, relative to the data folder.
const ICONS: [&str; 4] = [
    "icons/hicolor/scalable/apps/flottille.svg",
    "icons/hicolor/48x48/apps/flottille.png",
    "icons/hicolor/128x128/apps/flottille.png",
    "icons/hicolor/256x256/apps/flottille.png",
];

fn assert_icons(data: &Path) {
    for icon in ICONS {
        assert_eq!(
            std::fs::read(data.join(icon)).unwrap_or_else(|e| panic!("{icon}: {e}")),
            std::fs::read(repo("packaging/linux").join(icon)).unwrap(),
            "{icon}"
        );
    }
}

/// `desktop-file-validate` accepts the entry, when that program is installed.
fn assert_valid(entry: &Path) {
    match Command::new("desktop-file-validate").arg(entry).output() {
        Ok(output) => assert!(output.status.success(), "{output:?}"),
        Err(_) => eprintln!("desktop-file-validate is not installed: not checked"),
    }
}

fn exec_of(entry: &Path) -> String {
    std::fs::read_to_string(entry)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Exec=").map(str::to_owned))
        .unwrap()
}

#[test]
fn install_sh_writes_the_desktop_entry_and_the_icon() {
    let releases = Releases::serve();
    releases.stable("0.4.0");
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();

    let output = install(home.path(), Some(data.path()), &releases);

    assert!(output.status.success(), "{output:?}");
    let entry = data.path().join("applications/flottille.desktop");
    assert_eq!(
        exec_of(&entry),
        format!(
            "{} window",
            home.path().join(".local/bin/flottille").display()
        )
    );
    let text = std::fs::read_to_string(&entry).unwrap();
    for line in [
        "Type=Application",
        "Name=Flottille",
        "Icon=flottille",
        "Terminal=false",
        "Categories=Development;",
        "StartupWMClass=flottille",
    ] {
        assert!(text.lines().any(|l| l == line), "{line} is missing: {text}");
    }
    assert_valid(&entry);
    assert_icons(data.path());
    // Nothing of it went to the home's own data folder.
    assert!(!home.path().join(".local/share").exists());
}

#[test]
fn install_sh_writes_them_under_the_home_when_xdg_data_home_is_not_set() {
    let releases = Releases::serve();
    releases.stable("0.4.0");
    let home = tempfile::tempdir().unwrap();

    let output = install(home.path(), None, &releases);

    assert!(output.status.success(), "{output:?}");
    let data = home.path().join(".local/share");
    assert!(data.join("applications/flottille.desktop").is_file());
    assert_icons(&data);
}

#[test]
fn install_sh_quotes_a_binary_path_that_holds_a_space() {
    let releases = Releases::serve();
    releases.stable("0.4.0");
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("my home");
    std::fs::create_dir(&home).unwrap();
    let data = root.path().join("data");

    let output = install(&home, Some(&data), &releases);

    assert!(output.status.success(), "{output:?}");
    let entry = data.join("applications/flottille.desktop");
    assert_eq!(
        exec_of(&entry),
        format!("\"{}\" window", home.join(".local/bin/flottille").display())
    );
    assert_valid(&entry);
}

#[test]
fn install_sh_leaves_the_install_alone_when_the_entry_cannot_name_the_binary() {
    // A `$` in the path has no way to be said in an Exec line: the binary is installed all the
    // same, and no entry is written with a path the launcher would read wrong.
    let releases = Releases::serve();
    releases.stable("0.4.0");
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home$USER");
    std::fs::create_dir(&home).unwrap();
    let data = root.path().join("data");

    let output = install(&home, Some(&data), &releases);

    assert!(output.status.success(), "{output:?}");
    assert!(home.join(".local/bin/flottille").is_file());
    assert!(!data.join("applications/flottille.desktop").exists());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("desktop entry"),
        "{output:?}"
    );
}

/// The version `flottille --version` says for this build.
fn own_version() -> String {
    let said = Command::new(env!("CARGO_BIN_EXE_flottille"))
        .arg("--version")
        .output()
        .unwrap();
    String::from_utf8_lossy(&said.stdout)
        .split_whitespace()
        .nth(1)
        .unwrap()
        .to_owned()
}

/// The binary `update` replaces, copied into a folder of its own under cargo's scratch folder.
fn installed() -> (tempfile::TempDir, PathBuf) {
    let folder = tempfile::Builder::new()
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let path = folder.path().join("flottille");
    put_binary(&path);
    (folder, path)
}

/// The real binary at `path`: linked to it rather than copied (it weighs a gigabyte in a debug
/// build), and `update` renames the new one over the link, leaving the real one as it is.
fn put_binary(path: &Path) {
    let _ = std::fs::remove_file(path);
    if std::fs::hard_link(env!("CARGO_BIN_EXE_flottille"), path).is_err() {
        let copied = Command::new("cp")
            .arg(env!("CARGO_BIN_EXE_flottille"))
            .arg(path)
            .status()
            .unwrap();
        assert!(copied.success());
    }
}

/// An entry as `install.sh` of an older install left it: its binary somewhere else.
fn old_entry(user: &User) -> PathBuf {
    let entry = user.path("data/applications/flottille.desktop");
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(
        &entry,
        "[Desktop Entry]\nType=Application\nName=Flottille\nExec=/old/place/flottille window\n\
         Icon=flottille\n",
    )
    .unwrap();
    entry
}

#[test]
fn update_rewrites_the_desktop_entry_when_the_binary_moved() {
    let releases = Releases::serve();
    releases.stable("99.0.0");
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let entry = old_entry(&user);
    let (_folder, binary) = installed();

    let output = user
        .command_through(&binary, &["update", "--channel", "stable"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let now = binary.canonicalize().unwrap();
    assert_eq!(exec_of(&entry), format!("{} window", now.display()));
    assert_valid(&entry);
    assert_icons(&user.path("data"));
}

#[test]
fn update_writes_the_entry_for_an_install_that_had_none() {
    let releases = Releases::serve();
    releases.stable("99.0.0");
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let (_folder, binary) = installed();

    let output = user
        .command_through(&binary, &["update", "--channel", "stable"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let entry = user.path("data/applications/flottille.desktop");
    assert_eq!(
        exec_of(&entry),
        format!("{} window", binary.canonicalize().unwrap().display())
    );
    assert_icons(&user.path("data"));
}

#[test]
fn update_points_the_entry_at_the_binary_even_when_there_is_nothing_to_update() {
    let releases = Releases::serve();
    releases.stable(&own_version());
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let entry = old_entry(&user);
    let (_folder, binary) = installed();

    let output = user
        .command_through(&binary, &["update", "--channel", "stable"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("already"));
    assert_eq!(
        exec_of(&entry),
        format!("{} window", binary.canonicalize().unwrap().display())
    );
}

#[test]
fn update_leaves_an_entry_that_is_right_untouched() {
    let releases = Releases::serve();
    releases.stable("99.0.0");
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let (_folder, binary) = installed();
    // What `update` leaves at the path is the release's program: the real binary is put back.
    let update = || {
        put_binary(&binary);
        let output = user
            .command_through(&binary, &["update", "--channel", "stable"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    };
    update();
    let entry = user.path("data/applications/flottille.desktop");
    let icon = user.path("data/icons/hicolor/48x48/apps/flottille.png");
    let before = (
        std::fs::metadata(&entry).unwrap().modified().unwrap(),
        std::fs::metadata(&icon).unwrap().modified().unwrap(),
    );
    std::thread::sleep(std::time::Duration::from_millis(50));

    update();

    let after = (
        std::fs::metadata(&entry).unwrap().modified().unwrap(),
        std::fs::metadata(&icon).unwrap().modified().unwrap(),
    );
    assert_eq!(before, after);
}

#[test]
fn update_from_an_older_package_without_the_files_writes_nothing() {
    // A release made before the entry existed holds only the binary and the licences.
    let releases = Releases::serve();
    releases.stable("99.0.0");
    let name = "flottille-99.0.0-x86_64-linux.tar.gz";
    let work = tempfile::tempdir().unwrap();
    let archive = work.path().join(name);
    let folder = work.path().join("flottille-99.0.0");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("flottille"), "#!/bin/sh\necho old\n").unwrap();
    let packed = Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(work.path())
        .arg("flottille-99.0.0")
        .status()
        .unwrap();
    assert!(packed.success());
    let bytes = std::fs::read(&archive).unwrap();
    let sum = {
        use sha2::{Digest, Sha256};
        let hex: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("{hex}  {name}\n")
    };
    releases.put(&format!("/download/v99.0.0/{name}"), bytes);
    releases.put(
        &format!("/download/v99.0.0/{name}.sha256"),
        sum.into_bytes(),
    );
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let (_folder, binary) = installed();

    let output = user
        .command_through(&binary, &["update", "--channel", "stable"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(!user.path("data/applications").exists());
    assert!(!user.path("data/icons").exists());
}

/// The icon theme folder as a desktop left it, its size folders there (a file written in one
/// does not date it) and its own date long ago, so that a later touch of it shows.
fn long_ago(folder: &Path) {
    for size in ["scalable", "48x48", "128x128", "256x256"] {
        std::fs::create_dir_all(folder.join(size).join("apps")).unwrap();
    }
    std::fs::File::open(folder)
        .unwrap()
        .set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000))
        .unwrap();
}

fn modified(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

fn is_root() -> bool {
    // SAFETY: getuid has no precondition and cannot fail.
    unsafe { libc::getuid() == 0 }
}

#[test]
fn install_sh_still_succeeds_when_the_launcher_folder_cannot_be_written() {
    use std::os::unix::fs::PermissionsExt as _;

    if is_root() {
        eprintln!("root writes anywhere: not checked");
        return;
    }
    let releases = Releases::serve();
    releases.stable("0.4.0");
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::set_permissions(data.path(), std::fs::Permissions::from_mode(0o500)).unwrap();

    let output = install(home.path(), Some(data.path()), &releases);

    std::fs::set_permissions(data.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(home.path().join(".local/bin/flottille").is_file());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("the launcher entry was not written"),
        "{output:?}"
    );
    assert!(!data.path().join("applications/flottille.desktop").exists());
}

#[test]
fn install_sh_names_the_binary_by_its_real_path_as_update_does() {
    // A home reached through a link: `update` sees the binary's real path, so `install.sh` writes
    // that one, or the two would rewrite the entry in turn.
    let releases = Releases::serve();
    releases.stable("0.4.0");
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let linked = root.path().join("linked");
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    let data = root.path().join("data");

    let output = install(&linked, Some(&data), &releases);

    assert!(output.status.success(), "{output:?}");
    let binary = linked.join(".local/bin/flottille").canonicalize().unwrap();
    assert!(binary.starts_with(root.path().canonicalize().unwrap().join("real")));
    assert_eq!(
        exec_of(&data.join("applications/flottille.desktop")),
        format!("{} window", binary.display())
    );
}

#[test]
fn install_sh_writes_no_entry_for_a_path_with_a_newline() {
    let releases = Releases::serve();
    releases.stable("0.4.0");
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("two\nlines");
    std::fs::create_dir(&home).unwrap();
    let data = root.path().join("data");

    let output = install(&home, Some(&data), &releases);

    assert!(output.status.success(), "{output:?}");
    assert!(home.join(".local/bin/flottille").is_file());
    assert!(!data.join("applications/flottille.desktop").exists());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("desktop entry"),
        "{output:?}"
    );
}

#[test]
fn install_sh_dates_the_icon_folder_so_a_stale_cache_is_read_again() {
    let releases = Releases::serve();
    releases.stable("0.4.0");
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let hicolor = data.path().join("icons/hicolor");
    long_ago(&hicolor);
    let before = modified(&hicolor);

    let output = install(home.path(), Some(data.path()), &releases);

    assert!(output.status.success(), "{output:?}");
    assert!(modified(&hicolor) > before);
}

#[test]
fn update_dates_the_icon_folder_when_it_writes_an_icon_and_only_then() {
    let releases = Releases::serve();
    releases.stable("99.0.0");
    let user = User::new().with_env("FLOTTILLE_RELEASES", &releases.api());
    let hicolor = user.path("data/icons/hicolor");
    long_ago(&hicolor);
    let before = modified(&hicolor);
    let (_folder, binary) = installed();
    let update = || {
        put_binary(&binary);
        let output = user
            .command_through(&binary, &["update", "--channel", "stable"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    };

    update();
    let written = modified(&hicolor);
    assert!(written > before);

    // Nothing changed: the folder is left as it is.
    std::thread::sleep(std::time::Duration::from_millis(50));
    update();
    assert_eq!(modified(&hicolor), written);
}
