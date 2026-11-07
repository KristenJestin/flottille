//! When a task's run is ready and where it answers (`docs/projects.md`, "Tasks"): the address its
//! task gives, or prints with a line run once as the run starts, and the probes that say it is
//! ready, an address answered or a line that exits with 0, tried once a second. Nothing here runs
//! a shell, and nothing loads the machine: one probe at a time per run, each with a short limit.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use flottille_core::command::split_line;
use flottille_core::project::{Ready, TaskUrl};
use flottille_core::variables::{self, Names};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::rustls::{self, ClientConfig, pki_types::ServerName};
use url::{Host, Url};

use crate::agents::{signal_group, start_time};
use crate::daemon::Daemon;

/// How long a probe, or the line that prints an address, may take.
pub const PROBE_LIMIT: Duration = Duration::from_secs(5);
/// How often a starting run is probed.
pub const PROBE_EVERY: Duration = Duration::from_secs(1);

/// What says a run is ready, its templates filled.
pub enum Probe {
    /// A line of its output holds the text.
    Line(String),
    /// A request to the address is answered.
    Url(String),
    /// The words, run in the run's folder, exit with 0.
    Command(Vec<String>),
}

/// The task's `ready`, filled for this run, `{url}` being its address (empty without one).
pub fn probe(ready: &Ready, names: &Names, url: Option<&str>) -> Result<Probe, String> {
    let extra = [("url", url.unwrap_or_default())];
    Ok(match ready {
        Ready::Line(text) => Probe::Line(variables::fill_also(text, names, &extra)?),
        Ready::Url(template) => {
            let address = variables::fill_also(template, names, &extra)?;
            if !is_address(&address) {
                return Err(format!(
                    "“{address}” is not an http:// or https:// address with a host"
                ));
            }
            Probe::Url(address)
        }
        Ready::Command(line) => Probe::Command(words(line, names, &extra)?),
    })
}

/// A line split without a shell, each word a template.
fn words(line: &str, names: &Names, extra: &[(&str, &str)]) -> Result<Vec<String>, String> {
    split_line(line)?
        .iter()
        .map(|word| variables::fill_also(word, names, extra))
        .collect()
}

/// The run's address: the task's, filled, or the first line with something on it its command
/// prints, which must be an `http(s)://` address. `Err` says why there is none.
pub async fn address(
    daemon: &Daemon,
    url: &TaskUrl,
    names: &Names,
    env: &BTreeMap<OsString, OsString>,
    folder: &Path,
) -> Result<String, String> {
    let command = match url {
        TaskUrl::Address(template) => return variables::fill(template, names),
        TaskUrl::Command { command } => command,
    };
    let words = words(command, names, &[])?;
    let shown = words.join(" ");
    let mut child = spawn(&words, env, folder, true)
        .map_err(|e| format!("the command for its address, `{shown}`, cannot run: {e}"))?;
    let _group = Group::new(daemon, &child, "address");
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let ran = async {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (_, _, status) = tokio::join!(
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err),
            child.wait()
        );
        (out, err, status)
    };
    let Ok((out, err, status)) = tokio::time::timeout(PROBE_LIMIT, ran).await else {
        return Err(format!(
            "the command for its address, `{shown}`, did not end within {} s",
            PROBE_LIMIT.as_secs()
        ));
    };
    let status = status.map_err(|e| format!("`{shown}`: {e}"))?;
    if !status.success() {
        let said = last_line(&err).or_else(|| last_line(&out));
        let code = status
            .code()
            .map_or("was killed by a signal".to_owned(), |code| {
                format!("exited with code {code}")
            });
        return Err(match said {
            Some(said) => format!("the command for its address, `{shown}`, {code}: {said}"),
            None => format!("the command for its address, `{shown}`, {code}"),
        });
    }
    let first = String::from_utf8_lossy(&out)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned);
    match first {
        Some(line) if is_address(&line) => Ok(line),
        Some(line) => Err(format!(
            "the command for its address, `{shown}`, printed “{}”, not an http:// or https:// \
             address",
            line.chars().take(200).collect::<String>()
        )),
        None => Err(format!(
            "the command for its address, `{shown}`, printed nothing"
        )),
    }
}

/// An `http(s)://` address with a host, as a request can be sent to.
pub fn is_address(text: &str) -> bool {
    Url::parse(text).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some_and(|h| !h.is_empty())
    })
}

/// A line's session, killed whole when it is dropped, whatever ends the line: its end, its
/// limit, or the run it probes ending first. Recorded as a run's are, so that a daemon started
/// after a crash kills it too.
struct Group<'a> {
    daemon: &'a Daemon,
    pid: Option<u32>,
}

impl<'a> Group<'a> {
    fn new(daemon: &'a Daemon, child: &tokio::process::Child, run: &str) -> Self {
        let pid = child.id();
        if let Some(pid) = pid
            && let Some(started) = start_time(pid)
        {
            let recorded = daemon.read(|store| store.add_run_process(pid, started, run));
            if let Err(error) = recorded {
                eprintln!("flottille daemon: recording the probe process {pid}: {error}");
            }
        }
        Self { daemon, pid }
    }
}

impl Drop for Group<'_> {
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            signal_group(pid, libc::SIGKILL);
            // A drop cannot wait, and ending a session reads all of `/proc`: on a thread of its own.
            std::thread::spawn(move || crate::children::end_session_now(pid));
            let _ = self.daemon.read(|store| store.remove_run_process(pid));
        }
    }
}

fn last_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(300).collect())
}

/// A line of the project's, started without a shell in a session of its own, with the
/// run's environment; its output piped when `piped`, else dropped.
fn spawn(
    words: &[String],
    env: &BTreeMap<OsString, OsString>,
    folder: &Path,
    piped: bool,
) -> std::io::Result<tokio::process::Child> {
    let output = || {
        if piped { Stdio::piped() } else { Stdio::null() }
    };
    let mut command = tokio::process::Command::new(&words[0]);
    command
        .args(&words[1..])
        .env_clear()
        .envs(env)
        .current_dir(folder)
        .stdin(Stdio::null())
        .stdout(output())
        .stderr(output())
        .kill_on_drop(true);
    crate::children::spawn(crate::children::in_own_session(&mut command))
}

/// Whether the line exits with 0 within the probe's limit; killed past it.
pub async fn command_passes(
    daemon: &Daemon,
    run: &str,
    words: &[String],
    env: &BTreeMap<OsString, OsString>,
    folder: &Path,
) -> bool {
    let Ok(mut child) = spawn(words, env, folder, false) else {
        return false;
    };
    let _group = Group::new(daemon, &child, run);
    tokio::time::timeout(PROBE_LIMIT, child.wait())
        .await
        .is_ok_and(|status| status.is_ok_and(|status| status.success()))
}

/// Whether the address answers a request, whatever its status, within the probe's limit.
pub async fn answers(address: &str) -> bool {
    let Ok(url) = Url::parse(address) else {
        return false;
    };
    tokio::time::timeout(PROBE_LIMIT, request(&url))
        .await
        .is_ok_and(|answered| answered.unwrap_or(false))
}

/// A host of this machine: `localhost`, a name under `.localhost` (which always means this
/// machine, RFC 6761), or a loopback address.
fn local(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    }
}

async fn request(url: &Url) -> std::io::Result<bool> {
    let host = url.host().ok_or(std::io::ErrorKind::InvalidInput)?;
    let port = url
        .port_or_known_default()
        .ok_or(std::io::ErrorKind::InvalidInput)?;
    let is_local = local(&host);
    // A name under `.localhost` is this machine whatever the resolver says of it.
    let stream = match &host {
        Host::Domain(_) if is_local => {
            match TcpStream::connect(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)).await {
                Ok(stream) => stream,
                Err(_) => {
                    TcpStream::connect(SocketAddr::new(Ipv6Addr::LOCALHOST.into(), port)).await?
                }
            }
        }
        Host::Domain(name) => TcpStream::connect((*name, port)).await?,
        Host::Ipv4(ip) => TcpStream::connect(SocketAddr::new(IpAddr::V4(*ip), port)).await?,
        Host::Ipv6(ip) => TcpStream::connect(SocketAddr::new(IpAddr::V6(*ip), port)).await?,
    };
    let path = &url[url::Position::BeforePath..url::Position::AfterQuery];
    let head = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: flottille\r\nConnection: close\r\n\r\n",
        &url[url::Position::BeforeHost..url::Position::AfterPort]
    );
    let mut answer = [0; 5];
    if url.scheme() == "https" {
        let name = match host {
            Host::Domain(name) => ServerName::try_from(name.to_owned())
                .map_err(|_| std::io::ErrorKind::InvalidInput)?,
            Host::Ipv4(ip) => ServerName::IpAddress(IpAddr::V4(ip).into()),
            Host::Ipv6(ip) => ServerName::IpAddress(IpAddr::V6(ip).into()),
        };
        let config = match is_local {
            true => LOCAL_TLS.clone(),
            false => TLS.clone(),
        }
        .map_err(std::io::Error::other)?;
        let mut stream = tokio_rustls::TlsConnector::from(config)
            .connect(name, stream)
            .await?;
        stream.write_all(head.as_bytes()).await?;
        stream.read_exact(&mut answer).await?;
    } else {
        let mut stream = stream;
        stream.write_all(head.as_bytes()).await?;
        stream.read_exact(&mut answer).await?;
    }
    Ok(&answer == b"HTTP/")
}

/// Built once, the system's certificates read once, at the first probe that needs them.
static TLS: LazyLock<Result<Arc<ClientConfig>, String>> = LazyLock::new(|| tls(false));
static LOCAL_TLS: LazyLock<Result<Arc<ClientConfig>, String>> = LazyLock::new(|| tls(true));

/// The TLS of a probe: the system's certificates, or, for a host of this machine only, any
/// certificate, since a local development server serves one of its own authority (a probe sends
/// nothing but a GET with no credential, and reads five bytes of the answer).
fn tls(local: bool) -> Result<Arc<ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let config = if local {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyLocal(provider)))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().certs {
            let _ = roots.add(cert);
        }
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    Ok(Arc::new(config))
}

/// Takes the certificate of a host of this machine as it is, its signatures still checked.
#[derive(Debug)]
struct AnyLocal(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AnyLocal {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        signature: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            signature,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_host_of_this_machine_is_local() {
        for (host, is_local) in [
            ("https://localhost:3000", true),
            ("https://fix-ui.myapp.localhost", true),
            ("http://127.0.0.1:8080", true),
            ("http://[::1]:8080", true),
            ("https://example.com", false),
            ("https://localhost.example.com", false),
            ("http://192.168.1.10", false),
        ] {
            let url = Url::parse(host).unwrap();
            assert_eq!(local(&url.host().unwrap()), is_local, "{host}");
        }
    }
}
