//! The desktop of a test: a session bus of its own, a notification server on it that records
//! what it is told and says what the user does (`docs/design.md`, §11), and the sounds the
//! daemon plays recorded instead of heard. Never the machine's bus nor its audio device: the
//! harness points the daemon at `runtime/bus` of the user's own folder, and removes the sound
//! server's variables.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

use super::User;

/// What a test that needs a bus says when the machine has no `dbus-daemon`: it is skipped, as the
/// tests that need a kernel feature are.
pub fn skipped(why: &str) {
    eprintln!("skipped: {why}");
}

/// A private session bus: the `dbus-daemon` of the test, listening where the user's daemon looks
/// for the session bus, stopped when it is dropped.
pub struct Bus {
    child: Child,
    address: String,
}

impl Bus {
    /// The bus of `user`, or `None` (said once on the error output) when this machine has no
    /// `dbus-daemon`: the test is skipped, and CI has the package.
    pub fn start(user: &User) -> Option<Bus> {
        let socket = user.path("runtime/bus");
        let config = user.path("bus.conf");
        std::fs::write(
            &config,
            format!(
                "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\" \
                 \"http://www.freedesktop.org/standards/dtds/busconfig.dtd\">\n\
                 <busconfig><type>session</type><listen>unix:path={}</listen>\
                 <auth>EXTERNAL</auth><policy context=\"default\"><allow send_destination=\"*\" \
                 eavesdrop=\"true\"/><allow eavesdrop=\"true\"/><allow own=\"*\"/></policy>\
                 </busconfig>\n",
                socket.display()
            ),
        )
        .unwrap();
        let child = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .arg("--nofork")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let child = match child {
            Ok(child) => child,
            Err(error) => {
                skipped(&format!("dbus-daemon cannot be started here: {error}"));
                return None;
            }
        };
        let bus = Bus {
            child,
            address: format!("unix:path={}", socket.display()),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !std::fs::metadata(&socket).is_ok_and(|m| m.file_type().is_socket()) {
            assert!(Instant::now() < deadline, "the private bus never listened");
            std::thread::sleep(Duration::from_millis(20));
        }
        Some(bus)
    }

    pub fn address(&self) -> &str {
        &self.address
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One call of `Notify`, as the server heard it.
#[derive(Clone, Debug)]
pub struct Notified {
    pub id: u32,
    pub app_name: String,
    pub replaces_id: u32,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<String>,
    /// The names of the hints, and the ones that are text, as text.
    pub hints: HashMap<String, Option<String>>,
    pub expire_timeout: i32,
}

impl Notified {
    /// The label of the action `key`.
    pub fn label_of(&self, key: &str) -> Option<&str> {
        self.actions
            .chunks(2)
            .find(|pair| pair[0] == key)
            .map(|pair| pair[1].as_str())
    }
}

#[derive(Default)]
struct State {
    notified: Vec<Notified>,
    closed: Vec<u32>,
    capabilities: Vec<String>,
    next: u32,
}

struct Server(Arc<Mutex<State>>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<String> {
        self.0.lock().unwrap().capabilities.clone()
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let mut state = self.0.lock().unwrap();
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            state.next += 1;
            state.next
        };
        let hints = hints
            .into_iter()
            .map(|(name, value)| {
                let text = <&str>::try_from(&value).ok().map(str::to_owned);
                (name, text)
            })
            .collect();
        state.notified.push(Notified {
            id,
            app_name,
            replaces_id,
            app_icon,
            summary,
            body,
            actions,
            hints,
            expire_timeout,
        });
        id
    }

    fn close_notification(&self, id: u32) {
        self.0.lock().unwrap().closed.push(id);
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "fake-server".into(),
            "flottille-tests".into(),
            "1".into(),
            "1.2".into(),
        )
    }

    #[zbus(signal)]
    async fn action_invoked(emitter: &SignalEmitter<'_>, id: u32, key: String) -> zbus::Result<()>;
}

/// A notification server on the private bus: what it is told is kept for the test to read.
pub struct Notifications {
    state: Arc<Mutex<State>>,
    connection: zbus::blocking::Connection,
}

impl Notifications {
    /// A server that says it supports `capabilities` (`["actions", "body"]` is a common one).
    pub fn start(bus: &Bus, capabilities: &[&str]) -> Notifications {
        let state = Arc::new(Mutex::new(State {
            capabilities: capabilities.iter().map(|c| (*c).to_owned()).collect(),
            ..State::default()
        }));
        let connection = zbus::blocking::connection::Builder::address(bus.address())
            .unwrap()
            .name("org.freedesktop.Notifications")
            .unwrap()
            .serve_at("/org/freedesktop/Notifications", Server(state.clone()))
            .unwrap()
            .build()
            .unwrap();
        Notifications { state, connection }
    }

    pub fn notified(&self) -> Vec<Notified> {
        self.state.lock().unwrap().notified.clone()
    }

    /// The numbers `CloseNotification` was called with.
    pub fn closed(&self) -> Vec<u32> {
        self.state.lock().unwrap().closed.clone()
    }

    /// Waits for `count` calls of `Notify` (at least), and answers them.
    pub fn wait_for(&self, count: usize) -> Vec<Notified> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let notified = self.notified();
            if notified.len() >= count {
                return notified;
            }
            assert!(
                Instant::now() < deadline,
                "the desktop was told {} notifications, not {count}: {notified:?}",
                notified.len()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Waits until `CloseNotification` was called with `id`.
    pub fn wait_closed(&self, id: u32) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.closed().contains(&id) {
            assert!(
                Instant::now() < deadline,
                "notification {id} was never closed: {:?}",
                self.closed()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// What the user does with a notification: `ActionInvoked`, as the desktop's server says it.
    pub fn invoke(&self, id: u32, key: &str) {
        self.connection
            .emit_signal(
                None::<&str>,
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
                "ActionInvoked",
                &(id, key),
            )
            .unwrap();
    }

    /// Lets the test hold it: a server that leaves the bus.
    pub fn stop(self) {
        drop(self);
    }
}

/// The sounds this user's daemon plays are written to a file, one name a line, rather than
/// played: `needs-you` or `tell`. The daemon's configuration says so, in a test build only.
pub fn with_fake_audio(user: &User) -> std::path::PathBuf {
    let played = user.path("played.txt");
    std::fs::write(
        user.path("config/flottille/audio.toml"),
        format!("backend = \"fake\"\nfile = \"{}\"\n", played.display()),
    )
    .unwrap();
    played
}

/// The sounds played so far, in order.
pub fn played(file: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The daemon's own log, what it says once.
pub fn daemon_log(user: &User) -> String {
    std::fs::read_to_string(user.path("state/flottille/daemon.log")).unwrap_or_default()
}

/// A window, for the daemon: a connection that keeps its presence for as long as it is open.
pub struct Window {
    stream: UnixStream,
}

impl Window {
    /// Connects as a window does and says it is `focused`, showing `session` when one is.
    pub fn open(user: &User, focused: bool, session: Option<&str>) -> Window {
        user.status();
        let stream = UnixStream::connect(user.path("runtime/flottille/daemon.sock")).unwrap();
        let mut window = Window { stream };
        window.tell(focused, session);
        window
    }

    /// Says its presence again.
    pub fn tell(&mut self, focused: bool, session: Option<&str>) {
        let request = json!({"id": "presence", "method": "presence.set",
            "params": {"focused": focused, "session": session}});
        writeln!(self.stream, "{request}").unwrap();
        // The answer: said before the test goes on, so that what follows is after it.
        let mut line = String::new();
        let mut reader = std::io::BufReader::new(self.stream.try_clone().unwrap());
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        let answer: Value = serde_json::from_str(&line).unwrap();
        assert!(answer.get("error").is_none(), "presence.set: {answer}");
    }
}
