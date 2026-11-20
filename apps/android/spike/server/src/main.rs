//! The server side of the Android push spike (#823). Throwaway: it proves one thing, that a
//! phone answers a request from its notification with the app killed and the phone in Doze.
//!
//! It runs, in temporary folders of its own (never the user's configuration, keyring or home):
//!
//! - a hub (`flottille-hub`, served in this process) on `--hub` (127.0.0.1:7448 by default, which
//!   the phone reaches through `adb reverse`);
//! - a daemon (`flottille daemon`) with remote on, set up as the hub's first member `pc`, whose
//!   agent is the fake ACP agent; its project `app` and one session;
//! - the **gate**, on `--listen` (0.0.0.0:7470 by default), the address the phone knows. It
//!   stands for the link the phone will open with the mobile core (#824): the phone sends it
//!   requests signed with its device key, in the very format a link carries
//!   (`flottille_link::Request`), the gate checks them as a daemon does (`Requests::check`,
//!   against the member list the daemon checked) and asks its daemon;
//! - the sender of the push: an encrypted Web Push (RFC 8291, aes128gcm, with a VAPID signature)
//!   that holds a kind and an id only, sent to the endpoint the phone registered;
//! - a stand-in push server for the emulator's stub distributor (`/push/<token>`, `/poll`).
//!
//! The gate speaks plain HTTP/1.1, one request per connection:
//!
//! ```text
//! GET  /invitation        a fresh invitation's code; the join request it brings is accepted
//!                         as `phone`, a controller, and its four words kept for /state
//! POST /register          a signed push.register {endpoint, p256dh, auth}
//! POST /hold              the fake agent asks for a command; once it is held, the push is sent
//! POST /call              a signed request (approval.list, approval.approve, approval.deny)
//! POST /report            the phone's own times for a try
//! GET  /state             everything above, as it happened, with this machine's times
//! POST /push/<token>      the stub distributor's push server: a Web Push in
//! GET  /poll?token=<t>    and out, held up to 25 s
//! ```

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use flottille_client::Connection;
use flottille_link::{Device, Hello, Request, Requests};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Value, json};

type Result<T> = std::result::Result<T, String>;

/// The name the daemon's machine takes in the member list: the phone's requests are for it.
const MACHINE: &str = "pc";

fn main() {
    if let Err(error) = run() {
        eprintln!("spike-server: {error}");
        std::process::exit(1);
    }
}

struct Options {
    bin: PathBuf,
    listen: String,
    hub: String,
    folder: Option<PathBuf>,
}

fn options() -> Result<Options> {
    let mut options = Options {
        bin: PathBuf::from("target/debug"),
        listen: "0.0.0.0:7470".into(),
        hub: "127.0.0.1:7448".into(),
        folder: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} takes a value"));
        match arg.as_str() {
            "--bin" => options.bin = PathBuf::from(value()?),
            "--listen" => options.listen = value()?,
            "--hub" => options.hub = value()?,
            "--folder" => options.folder = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    options.bin = options
        .bin
        .canonicalize()
        .map_err(|error| format!("{}: {error}", options.bin.display()))?;
    Ok(options)
}

fn run() -> Result<()> {
    let options = options()?;
    let folder = match &options.folder {
        Some(folder) => folder.clone(),
        None => std::env::temp_dir().join(format!("flottille-spike-{}", std::process::id())),
    };
    let user = User::make(&folder, &options.bin)?;
    say(&format!("folders in {}", folder.display()));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let served = runtime.block_on(async {
        let hub = flottille_hub::Hub::open(&folder.join("hub"), flottille_hub::SystemClock)
            .map_err(|error| error.to_string())?;
        let listener = tokio::net::TcpListener::bind(&options.hub)
            .await
            .map_err(|error| format!("the hub cannot listen on {}: {error}", options.hub))?;
        flottille_hub::serve(hub, listener, None, Hello::ours("0.4.0-spike"))
            .map_err(|error| error.to_string())
    })?;
    let hub_url = format!("http://{}", served.address());
    let code = served.with_hub(|hub| hub.setup_code().map(str::to_owned));
    let code = code.map_err(|error| error.to_string())?;
    say(&format!("hub on {hub_url}"));

    let _daemon = user.start_daemon()?;
    user.cli_with_input(
        &["remote", "setup", &hub_url, "--name", MACHINE],
        &format!("{code}\n"),
    )?;
    say("the daemon is the hub's first member, pc");
    let agent = user.session()?;
    say(&format!("the fake agent's session is {agent}"));

    let gate = Arc::new(Gate {
        user,
        agent,
        state: Mutex::new(State::default()),
        requests: Mutex::new(Requests::new(SystemTime::now())),
        pushes: (Mutex::new(HashMap::new()), Condvar::new()),
        vapid: Vapid::new()?,
    });
    {
        let gate = gate.clone();
        std::thread::spawn(move || gate.accept_joins());
    }
    let listener = TcpListener::bind(&options.listen)
        .map_err(|error| format!("the gate cannot listen on {}: {error}", options.listen))?;
    println!(
        "{}",
        json!({"ready": true, "gate": options.listen, "hub": hub_url, "folder": folder})
    );
    for stream in listener.incoming().flatten() {
        let gate = gate.clone();
        std::thread::spawn(move || gate.serve(stream));
    }
    drop(served);
    Ok(())
}

fn say(what: &str) {
    eprintln!("spike-server: {what}");
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ----------------------------------------------------------------------------------------------
// The daemon's user: temporary folders, the fake agent, the same variables as the tests'.

struct User {
    dir: PathBuf,
    bin: PathBuf,
}

impl User {
    fn make(dir: &Path, bin: &Path) -> Result<User> {
        let io = |error: std::io::Error| error.to_string();
        for sub in [
            "",
            "home",
            "state",
            "config",
            "config/flottille",
            "data",
            "runtime",
            "fake",
        ] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .recursive(true)
                .create(dir.join(sub))
                .map_err(io)?;
        }
        let config = dir.join("config/flottille");
        // The daemon's keyring is its memory, never the user's own; no update check; remote on.
        std::fs::write(config.join("keyring.toml"), "backend = \"memory\"\n").map_err(io)?;
        std::fs::write(config.join("updates.toml"), "check = \"never\"\n").map_err(io)?;
        std::fs::write(config.join("daemon.toml"), "[remote]\non = true\n").map_err(io)?;
        // The fake agent asks for a command on "hold": the gate holds it for the phone.
        let script = json!({"turns": [{"when": "hold", "do": [
            {"mcp": {"tool": "run", "arguments": {"line": "cat /etc/hostname"}}},
            {"say": "I asked for it."}
        ]}]});
        std::fs::write(dir.join("fake/script.json"), script.to_string()).map_err(io)?;
        Ok(User {
            dir: dir.to_owned(),
            bin: bin.to_owned(),
        })
    }

    fn command(&self, program: &str) -> Command {
        let dir = &self.dir;
        let mut command = Command::new(self.bin.join(program));
        command
            .env("HOME", dir.join("home"))
            .env("XDG_RUNTIME_DIR", dir.join("runtime"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_DATA_HOME", dir.join("data"))
            .env("TMUX_TMPDIR", dir.join("runtime"))
            // No session bus: no keyring of the user's, the device key is a file of the folder.
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}", dir.join("runtime/bus").display()),
            )
            .env("FLOTTILLE_TEST_NO_FETCH", "1")
            .env("FLOTTILLE_RELEASES", "http://127.0.0.1:9/repos/none")
            .env(
                "FLOTTILLE_AGENT_COMMAND",
                self.bin.join("flottille-fake-agent"),
            )
            .env("FLOTTILLE_FAKE_AGENT_DIR", dir.join("fake"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("SSH_AUTH_SOCK")
            .env_remove("TMUX");
        for name in [
            "WAYLAND_DISPLAY",
            "DISPLAY",
            "XAUTHORITY",
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CONFIG_DIR",
            "CODEX_ACCESS_TOKEN",
            "CODEX_HOME",
            "OPENAI_API_KEY",
        ] {
            command.env_remove(name);
        }
        command
    }

    fn start_daemon(&self) -> Result<Reaped> {
        let log = std::fs::File::create(self.dir.join("daemon.out")).map_err(|e| e.to_string())?;
        let child = self
            .command("flottille")
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .spawn()
            .map_err(|error| format!("flottille daemon: {error}"))?;
        let reaped = Reaped(child);
        let deadline = Instant::now() + Duration::from_secs(30);
        while UnixStream::connect(self.socket()).is_err() {
            if Instant::now() > deadline {
                return Err("the daemon did not open its socket in 30 s".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(reaped)
    }

    fn socket(&self) -> PathBuf {
        self.dir.join("runtime/flottille/daemon.sock")
    }

    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let stream = UnixStream::connect(self.socket()).map_err(|e| e.to_string())?;
        let mut connection = Connection::new(stream);
        match connection.ask(method, params) {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(error)) => Err(format!("{method}: {}", error.message)),
            Err(error) => Err(format!("{method}: {error}")),
        }
    }

    fn cli(&self, args: &[&str]) -> Result<String> {
        self.cli_with_input(args, "")
    }

    fn cli_with_input(&self, args: &[&str], input: &str) -> Result<String> {
        let mut child = self
            .command("flottille")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        child
            .stdin
            .take()
            .expect("piped")
            .write_all(input.as_bytes())
            .map_err(|error| error.to_string())?;
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "flottille {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn git(&self, folder: &Path, args: &[&str]) -> Result<()> {
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=Spike",
                "-c",
                "user.email=spike@example.invalid",
            ])
            .args(args)
            .current_dir(folder)
            .env("HOME", self.dir.join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(Stdio::null())
            .status()
            .map_err(|error| error.to_string())?;
        status
            .success()
            .then_some(())
            .ok_or(format!("git {}", args.join(" ")))
    }

    /// A project over a throwaway repository, a workspace, and a session of the fake agent: its id.
    fn session(&self) -> Result<String> {
        let folder = self.dir.join("work/app");
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        self.git(&folder, &["init", "--quiet", "-b", "main"])?;
        std::fs::write(folder.join("README.md"), "app\n").map_err(|e| e.to_string())?;
        self.git(&folder, &["add", "README.md"])?;
        self.git(&folder, &["commit", "--quiet", "-m", "start"])?;
        self.call("project.add", json!({"folder": folder, "name": "app"}))?;
        self.call("workspace.create", json!({"project": "app", "name": "w"}))?;
        let deadline = Instant::now() + Duration::from_secs(60);
        while self.call("workspace.show", json!({"project": "app", "name": "w"}))?["state"]
            != "ready"
        {
            if Instant::now() > deadline {
                return Err("the workspace was not ready in 60 s".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let answer = self.call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "hello"}),
        )?;
        answer["agent"]["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("agent.run said {answer}"))
    }

    /// The member list the daemon checked last, from the hub it pinned.
    fn members(&self) -> Result<flottille_link::Members> {
        let file = self.dir.join("state/flottille/remote/hub.json");
        let text =
            std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let device: Device = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        device.members().map_err(|error| error.to_string())
    }
}

/// A child process killed when this is dropped.
struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

// ----------------------------------------------------------------------------------------------
// The gate.

#[derive(Default)]
struct State {
    /// What happened, in order, each with `at`, this machine's time in ms since the epoch.
    events: Vec<Value>,
    /// The phone's push subscription: endpoint, p256dh and auth, as it registered them.
    subscription: Option<Value>,
    /// Join requests accepted already, by request id.
    accepted: Vec<String>,
}

/// The pushes held for the stub distributor, by token.
type Queues = HashMap<String, VecDeque<Vec<u8>>>;

struct Gate {
    user: User,
    agent: String,
    state: Mutex<State>,
    requests: Mutex<Requests>,
    /// The stub distributor's queue, by token.
    pushes: (Mutex<Queues>, Condvar),
    vapid: Vapid,
}

impl Gate {
    fn event(&self, mut event: Value) {
        event["at"] = json!(now_ms());
        say(&event.to_string());
        self.state.lock().unwrap().events.push(event);
    }

    /// Accepts every join request of the hub as `phone`, `phone2`…, a controller: the spike's
    /// user accepts on the phone's behalf, the four words kept for the phone to compare.
    fn accept_joins(&self) {
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let Ok(requests) = self.user.call("remote.requests", json!({})) else {
                continue;
            };
            for request in requests.as_array().into_iter().flatten() {
                let Some(id) = request["request"].as_str() else {
                    continue;
                };
                let count = {
                    let state = self.state.lock().unwrap();
                    if state.accepted.iter().any(|seen| seen == id) {
                        continue;
                    }
                    state.accepted.len()
                };
                let name = match count {
                    0 => "phone".to_owned(),
                    n => format!("phone{}", n + 1),
                };
                match self.user.cli(&[
                    "remote",
                    "accept",
                    id,
                    "--name",
                    &name,
                    "--role",
                    "controller",
                ]) {
                    Ok(_) => {
                        self.state.lock().unwrap().accepted.push(id.to_owned());
                        self.event(json!({"accepted": id, "name": name, "role": "controller",
                            "words": request["words"]}));
                    }
                    Err(error) => say(&error),
                }
            }
        }
    }

    fn serve(&self, stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        let (status, kind, body) = match read_request(stream) {
            Ok((method, path, body)) => self.route(&method, &path, &body),
            Err(error) => (
                400,
                "application/json",
                json!({"error": error}).to_string().into(),
            ),
        };
        let reason = match status {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            404 => "Not Found",
            _ => "Error",
        };
        let head = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n",
            body.len()
        );
        let _ = writer.write_all(head.as_bytes());
        let _ = writer.write_all(&body);
    }

    fn route(&self, method: &str, path: &str, body: &[u8]) -> (u16, &'static str, Vec<u8>) {
        let json = |status: u16, value: Value| -> (u16, &'static str, Vec<u8>) {
            (status, "application/json", value.to_string().into_bytes())
        };
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let answered = match (method, path) {
            ("GET", "/invitation") => self.invitation(),
            ("POST", "/register") => self.register(body),
            ("POST", "/hold") => self.hold(),
            ("POST", "/call") => self.call(body),
            ("POST", "/report") => self.report(body),
            ("GET", "/state") => Ok(self.snapshot()),
            ("GET", "/poll") => {
                let token = query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("token="))
                    .unwrap_or_default();
                return match self.poll(token) {
                    Some(push) => (200, "application/octet-stream", push),
                    None => (204, "application/octet-stream", Vec::new()),
                };
            }
            ("POST", push) if push.starts_with("/push/") => {
                let token = push.trim_start_matches("/push/").to_owned();
                let (queue, ready) = &self.pushes;
                queue
                    .lock()
                    .unwrap()
                    .entry(token)
                    .or_default()
                    .push_back(body.to_vec());
                ready.notify_all();
                return (201, "text/plain", Vec::new());
            }
            _ => return json(404, json!({"error": "no such path"})),
        };
        match answered {
            Ok(value) => json(200, value),
            Err(error) => {
                self.event(json!({"refused": error, "path": path}));
                json(400, json!({"refused": error}))
            }
        }
    }

    fn invitation(&self) -> Result<Value> {
        let invited: Value = serde_json::from_str(&self.user.cli(&["remote", "invite", "--json"])?)
            .map_err(|error| error.to_string())?;
        self.event(json!({"invited": true}));
        Ok(json!({"code": invited["code"]}))
    }

    /// The phone's `push.register`, signed with its device key: what #827 builds into the
    /// daemon, here in the gate.
    fn register(&self, body: &[u8]) -> Result<Value> {
        let text = std::str::from_utf8(body).map_err(|e| e.to_string())?;
        let request: Request = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let members = self.user.members()?;
        let member = members
            .get(&request.signed.device)
            .ok_or("refused: this request is not signed by a member")?;
        let bytes = request.signed_bytes().ok_or("refused: too long")?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(request.signed.device.as_bytes())
            .map_err(|e| e.to_string())?;
        key.verify_strict(
            &bytes,
            &ed25519_dalek::Signature::from_bytes(&request.sig.0),
        )
        .map_err(|_| "refused: this request is not signed by a member".to_owned())?;
        if request.signed.method != "push.register" || request.machine != MACHINE {
            return Err("refused: not a push.register for pc".into());
        }
        let params: Value =
            serde_json::from_str(&request.signed.params).map_err(|e| e.to_string())?;
        for field in ["endpoint", "p256dh", "auth"] {
            if !params[field].is_string() {
                return Err(format!("refused: push.register without {field}"));
            }
        }
        self.event(
            json!({"registered": member.name, "role": member.role.word(),
            "endpoint": params["endpoint"]}),
        );
        self.state.lock().unwrap().subscription = Some(params);
        Ok(json!({"ok": true}))
    }

    /// The fake agent asks for a command; once the request is held for the user, the push goes.
    fn hold(&self) -> Result<Value> {
        let before: Vec<u64> = self.pending()?;
        self.user.call(
            "agent.send",
            json!({"agent": self.agent, "message": "hold this, please"}),
        )?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let number = loop {
            if let Some(number) = self.pending()?.into_iter().find(|n| !before.contains(n)) {
                break number;
            }
            if Instant::now() > deadline {
                return Err("the fake agent's request was not held in 30 s".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        self.event(json!({"held": number}));
        let subscription = self
            .state
            .lock()
            .unwrap()
            .subscription
            .clone()
            .ok_or("no phone registered its endpoint")?;
        let payload = json!({"kind": "request", "id": number}).to_string();
        let sent = now_ms();
        let status = self.push(&subscription, payload.as_bytes())?;
        self.event(json!({"pushed": number, "status": status, "sent": sent}));
        Ok(json!({"number": number, "pushed": sent, "status": status}))
    }

    fn pending(&self) -> Result<Vec<u64>> {
        let listed = self.user.call("approval.list", Value::Null)?;
        Ok(listed["requests"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|request| request["number"].as_u64())
            .collect())
    }

    /// A request the phone signed, checked as a daemon checks one a link carries, then asked.
    fn call(&self, body: &[u8]) -> Result<Value> {
        let received = now_ms();
        let text = std::str::from_utf8(body).map_err(|e| e.to_string())?;
        let members = self.user.members()?;
        let accepted = self
            .requests
            .lock()
            .unwrap()
            .check(text, MACHINE, &members, SystemTime::now())
            .map_err(|error| error.to_string())?;
        let answer = self.user.call(&accepted.method, accepted.params.clone());
        self.event(json!({"call": accepted.method, "params": accepted.params,
            "by": accepted.device.name, "received": received,
            "answered": answer.is_ok(), "error": answer.as_ref().err()}));
        Ok(json!({"id": accepted.id, "answer": answer?}))
    }

    fn report(&self, body: &[u8]) -> Result<Value> {
        let report: Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
        self.event(json!({"report": report}));
        Ok(json!({"ok": true}))
    }

    fn snapshot(&self) -> Value {
        let state = self.state.lock().unwrap();
        json!({"now": now_ms(), "registered": state.subscription.is_some(),
            "events": state.events})
    }

    fn poll(&self, token: &str) -> Option<Vec<u8>> {
        let (queue, ready) = &self.pushes;
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut queue = queue.lock().unwrap();
        loop {
            if let Some(push) = queue.get_mut(token).and_then(VecDeque::pop_front) {
                return Some(push);
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            queue = ready.wait_timeout(queue, left).unwrap().0;
        }
    }

    /// Sends `payload` as an encrypted Web Push to the phone's subscription: the HTTP status.
    fn push(&self, subscription: &Value, payload: &[u8]) -> Result<u16> {
        let endpoint = subscription["endpoint"].as_str().unwrap_or_default();
        let decode = |field: &str| {
            let text = subscription[field].as_str().unwrap_or_default();
            URL_SAFE_NO_PAD
                .decode(text.trim_end_matches('='))
                .map_err(|error| format!("{field}: {error}"))
        };
        let body = encrypt(&decode("p256dh")?, &decode("auth")?, payload)?;
        let url = Url::parse(endpoint)?;
        let authorization = self.vapid.header(&url.origin())?;
        let head = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/octet-stream\r\n\
             Content-Encoding: aes128gcm\r\nTTL: 60\r\nUrgency: high\r\nAuthorization: \
             {authorization}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            url.path,
            url.host_port(),
            body.len()
        );
        let mut request = head.into_bytes();
        request.extend_from_slice(&body);
        let answer = url.exchange(&request)?;
        let status = answer
            .split(' ')
            .nth(1)
            .and_then(|code| code.parse().ok())
            .ok_or_else(|| format!("the push server answered {answer:?}"))?;
        Ok(status)
    }
}

/// One HTTP/1.1 request: its method, path and body.
fn read_request(stream: TcpStream) -> Result<(String, String, Vec<u8>)> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).map_err(|e| e.to_string())?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().map_err(|_| "a bad length")?;
        }
    }
    if length > 1 << 20 {
        return Err("too long".into());
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    Ok((method, path, body))
}

// ----------------------------------------------------------------------------------------------
// Web Push: RFC 8291's encryption, RFC 8188's aes128gcm framing, RFC 8292's VAPID.

fn hmac(key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    let mut context = ring::hmac::Context::with_key(&key);
    for part in parts {
        context.update(part);
    }
    context.sign().as_ref().to_vec()
}

/// HKDF's expand, one block: every length here is 32 bytes at most.
fn expand(prk: &[u8], info: &[u8], length: usize) -> Vec<u8> {
    let mut block = hmac(prk, &[info, &[1]]);
    block.truncate(length);
    block
}

/// The body of a Web Push of `payload` for the subscription keys `ua_public` (65 bytes) and
/// `auth` (16 bytes): one record, the sender's key in its header.
fn encrypt(ua_public: &[u8], auth: &[u8], payload: &[u8]) -> Result<Vec<u8>> {
    use ring::agreement;
    let rng = SystemRandom::new();
    let ephemeral = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng)
        .map_err(|_| "no ephemeral key")?;
    let as_public = ephemeral
        .compute_public_key()
        .map_err(|_| "no public key")?
        .as_ref()
        .to_vec();
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, ua_public);
    let shared = agreement::agree_ephemeral(ephemeral, &peer, |secret| secret.to_vec())
        .map_err(|_| "the phone's p256dh is not a P-256 key")?;
    let prk_key = hmac(auth, &[&shared]);
    let key_info = [b"WebPush: info\0".as_slice(), ua_public, &as_public].concat();
    let ikm = expand(&prk_key, &key_info, 32);
    let mut salt = [0u8; 16];
    rng.fill(&mut salt).map_err(|_| "no salt")?;
    let prk = hmac(&salt, &[&ikm]);
    let cek = expand(&prk, b"Content-Encoding: aes128gcm\0", 16);
    let nonce = expand(&prk, b"Content-Encoding: nonce\0", 12);
    let key = ring::aead::LessSafeKey::new(
        ring::aead::UnboundKey::new(&ring::aead::AES_128_GCM, &cek).map_err(|_| "no key")?,
    );
    let mut record = payload.to_vec();
    // The last record's delimiter, no padding.
    record.push(2);
    key.seal_in_place_append_tag(
        ring::aead::Nonce::try_assume_unique_for_key(&nonce).map_err(|_| "no nonce")?,
        ring::aead::Aad::empty(),
        &mut record,
    )
    .map_err(|_| "the encryption failed")?;
    let mut body = salt.to_vec();
    body.extend_from_slice(&4096u32.to_be_bytes());
    body.push(as_public.len() as u8);
    body.extend_from_slice(&as_public);
    body.extend_from_slice(&record);
    Ok(body)
}

/// The machine's VAPID key, made for this run.
struct Vapid {
    key: ring::signature::EcdsaKeyPair,
    rng: SystemRandom,
}

impl Vapid {
    fn new() -> Result<Vapid> {
        use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .map_err(|_| "no VAPID key")?;
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .map_err(|_| "no VAPID key")?;
        Ok(Vapid { key, rng })
    }

    fn header(&self, audience: &str) -> Result<String> {
        use ring::signature::KeyPair;
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = json!({"aud": audience, "exp": now_ms() / 1000 + 12 * 3600,
            "sub": "mailto:spike@example.invalid"});
        let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signed = format!("{header}.{claims}");
        let signature = self
            .key
            .sign(&self.rng, signed.as_bytes())
            .map_err(|_| "the VAPID signature failed")?;
        Ok(format!(
            "vapid t={signed}.{}, k={}",
            URL_SAFE_NO_PAD.encode(signature.as_ref()),
            URL_SAFE_NO_PAD.encode(self.key.public_key().as_ref())
        ))
    }
}

/// An `http://` or `https://` URL, as far as a push needs one.
struct Url {
    tls: bool,
    host: String,
    port: u16,
    path: String,
}

impl Url {
    fn parse(text: &str) -> Result<Url> {
        let (tls, rest) = if let Some(rest) = text.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = text.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(format!("{text} is not an http(s) URL"));
        };
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().map_err(|_| "a bad port")?),
            None => (authority, if tls { 443 } else { 80 }),
        };
        Ok(Url {
            tls,
            host: host.to_owned(),
            port,
            path: if path.is_empty() {
                "/".into()
            } else {
                path.into()
            },
        })
    }

    fn host_port(&self) -> String {
        match (self.tls, self.port) {
            (true, 443) | (false, 80) => self.host.clone(),
            _ => format!("{}:{}", self.host, self.port),
        }
    }

    fn origin(&self) -> String {
        format!(
            "{}://{}",
            if self.tls { "https" } else { "http" },
            self.host_port()
        )
    }

    /// Writes `request` and reads the answer's status line.
    fn exchange(&self, request: &[u8]) -> Result<String> {
        let io = |error: std::io::Error| format!("{}: {error}", self.host);
        let address = std::net::ToSocketAddrs::to_socket_addrs(&(self.host.as_str(), self.port))
            .map_err(io)?
            .next()
            .ok_or_else(|| format!("{} has no address", self.host))?;
        let mut tcp = TcpStream::connect_timeout(&address, Duration::from_secs(10)).map_err(io)?;
        tcp.set_read_timeout(Some(Duration::from_secs(20)))
            .map_err(io)?;
        let mut answer = Vec::new();
        if self.tls {
            let mut roots = rustls::RootCertStore::empty();
            for certificate in rustls_native_certs::load_native_certs().certs {
                let _ = roots.add(certificate);
            }
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
            let name = rustls::pki_types::ServerName::try_from(self.host.clone())
                .map_err(|e| e.to_string())?;
            let client =
                rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
            let mut stream = rustls::StreamOwned::new(client, tcp);
            stream.write_all(request).map_err(io)?;
            let _ = BufReader::new(stream).read_until(b'\n', &mut answer);
        } else {
            tcp.write_all(request).map_err(io)?;
            let _ = BufReader::new(tcp).read_until(b'\n', &mut answer);
        }
        Ok(String::from_utf8_lossy(&answer).trim().to_owned())
    }
}
