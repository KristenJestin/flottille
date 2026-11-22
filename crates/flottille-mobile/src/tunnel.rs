//! The tunnels the phone holds to the runs of its machines (`docs/remote.md`, "Tunnels", "On the
//! phone"): the phone has no daemon, so it does what a client's daemon does (`flottille`'s
//! `tunnel.rs`): it signs `tunnel.open` for the machine of the run, listens on `127.0.0.1`, and
//! opens one stream of the machine's link for each TCP connection to that port. The tunnel is held
//! by its request's stream: it ends when the machine says it did (the run ended, the device was
//! removed), when the link ends, or when the user closes it, which drops the stream.
//!
//! What a tunnel may reach is decided by the machine, never by the phone: a port the run listens
//! on, nothing else.

use std::net::Ipv4Addr;
use std::sync::Arc;

use flottille_link::{Answers, Link};
use flottille_protocol::address;
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::MobileError;
use crate::mobile::Inner;

/// A tunnel the phone holds, as the view says it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TunnelView {
    pub tunnel: String,
    pub machine: String,
    pub run: String,
    /// The run's port, on its machine.
    pub port: u16,
    /// The port the phone listens on, on `127.0.0.1`.
    pub local_port: u16,
}

/// A tunnel that ended, and why: the machine's own words, or the link's.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TunnelEnd {
    pub tunnel: String,
    pub machine: String,
    pub run: String,
    /// `run r1 ended`, `closed by the user`, `the link was lost`…
    pub reason: String,
}

/// What `open_address` answers: the tunnel, and the address read through it, to hand to the
/// in-app browser or the user's.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct Opened {
    pub tunnel: TunnelView,
    /// The address as it is read on this phone: `http://localhost:<local port>/path`.
    pub url: String,
}

/// What the phone keeps of a tunnel besides what the view says: how to close it, and what asked
/// for it (so that a second tap on the same address makes no second tunnel).
pub(crate) struct Held {
    pub(crate) view: TunnelView,
    /// The port the user asked for; none for the run's own address.
    pub(crate) asked: Option<u16>,
    pub(crate) close: watch::Sender<Option<String>>,
}

/// The run of `machine` that serves `port`, among the commands `run.list` answered: the one that
/// reports an address on that port, else the only one that runs. The machine checks the port
/// again, so a wrong guess is refused there, never reached.
pub(crate) fn run_for_port(
    listed: &Value,
    port: u16,
    machine: &str,
) -> Result<String, MobileError> {
    let running: Vec<&Value> = listed["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|command| command["state"] == "running")
        .collect();
    let named = |command: &&Value| command["run"].as_str().map(str::to_owned);
    running
        .iter()
        .find(|command| command["url"].as_str().and_then(address::local_port) == Some(port))
        .or_else(|| (running.len() == 1).then(|| &running[0]))
        .and_then(named)
        .ok_or_else(|| MobileError::new(format!("no run of {machine} serves port {port}")))
}

impl Inner {
    /// Opens (or finds, when it is open already) the tunnel to the run of `machine` that `url`
    /// names: `run`'s own address when a run is given, else the port a `localhost` link writes.
    pub(crate) async fn open_address(
        self: &Arc<Self>,
        machine: &str,
        run: Option<String>,
        url: &str,
    ) -> Result<Opened, MobileError> {
        let port = address::local_port(url);
        if run.is_none() && port.is_none() {
            return Err(MobileError::new(format!(
                "{url} is no address of {machine}'s own: open it as it is"
            )));
        }
        let (run, asked) = match run {
            Some(run) => (run, None),
            None => {
                let port = port.expect("checked above");
                let listed = self.call(machine, "run.list", json!({})).await?;
                (run_for_port(&listed, port, machine)?, Some(port))
            }
        };
        if let Some(held) = self.reaching(machine, &run, asked) {
            return Ok(through(held, url));
        }
        self.open(machine, run, asked, url).await
    }

    /// The tunnel already held to `run` of `machine`, for the same asked port.
    fn reaching(&self, machine: &str, run: &str, asked: Option<u16>) -> Option<TunnelView> {
        self.state()
            .tunnels
            .values()
            .find(|held| {
                held.view.machine == machine && held.view.run == run && held.asked == asked
            })
            .map(|held| held.view.clone())
    }

    async fn open(
        self: &Arc<Self>,
        machine: &str,
        run: String,
        asked: Option<u16>,
        url: &str,
    ) -> Result<Opened, MobileError> {
        let link = self.link_of(machine)?;
        let mut params = json!({"run": run});
        if let Some(port) = asked {
            params["port"] = json!(port);
        }
        let mut answers = self.ask(machine, &link, "tunnel.open", params).await?;
        let first = match answers.next().await {
            Some(Ok(first)) => first,
            Some(Err(error)) => return Err(MobileError::new(error.to_string())),
            None => {
                return Err(MobileError::new(format!(
                    "the link to {machine} ended before it answered"
                )));
            }
        };
        if let Some(error) = first.get("error") {
            return Err(MobileError::new(
                error["message"]
                    .as_str()
                    .unwrap_or("the machine refused it"),
            ));
        }
        let result = &first["result"];
        let port = |key: &str| {
            result[key]
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
        };
        let (Some(tunnel), Some(run), Some(port)) = (
            result["tunnel"].as_str(),
            result["run"].as_str(),
            port("port"),
        ) else {
            return Err(MobileError::new(format!(
                "{machine} answered what is not a tunnel"
            )));
        };
        let (listener, local_port) = bind_local(port).map_err(|why| {
            MobileError::new(format!("cannot listen on 127.0.0.1 on this phone: {why}"))
        })?;
        let view = TunnelView {
            tunnel: tunnel.to_owned(),
            machine: machine.to_owned(),
            run: run.to_owned(),
            port,
            local_port,
        };
        let (close, closing) = watch::channel(None);
        {
            let mut state = self.state();
            state.view.tunnel_opened(view.clone());
            state.tunnels.insert(
                view.tunnel.clone(),
                Held {
                    view: view.clone(),
                    asked,
                    close,
                },
            );
        }
        let held = self.clone();
        let task_view = view.clone();
        self.handle.spawn(async move {
            let reason = hold(listener, link, answers, &task_view, closing).await;
            held.tunnel_ended(&task_view, reason);
        });
        self.note(crate::mobile::Note::Changed);
        Ok(through(view, url))
    }

    /// A tunnel ended: it leaves the view, which says why.
    fn tunnel_ended(&self, view: &TunnelView, reason: String) {
        let mut state = self.state();
        state.tunnels.remove(&view.tunnel);
        state.view.tunnel_ended(view, reason);
        drop(state);
        self.note(crate::mobile::Note::Changed);
    }

    /// Closes the tunnel `tunnel`; it leaves the view at once.
    pub(crate) fn close_tunnel(&self, tunnel: &str) -> Result<(), MobileError> {
        let state = self.state();
        let Some(held) = state.tunnels.get(tunnel) else {
            return Err(MobileError::new(format!(
                "no tunnel {tunnel} is held by this phone"
            )));
        };
        held.close
            .send_replace(Some("closed by the user".to_owned()));
        Ok(())
    }
}

/// `url` as it is read through `tunnel`.
fn through(tunnel: TunnelView, url: &str) -> Opened {
    Opened {
        url: address::through(url, tunnel.local_port),
        tunnel,
    }
}

/// Holds the tunnel until it ends, carrying its connections meanwhile: the reason it ended.
async fn hold(
    listener: TcpListener,
    link: Link,
    mut answers: Answers,
    view: &TunnelView,
    mut closing: watch::Receiver<Option<String>>,
) -> String {
    let accepting = tokio::spawn(accept(listener, link.clone(), view.tunnel.clone()));
    let reason = tokio::select! {
        // The machine ended it, saying why, or its stream ended with the link.
        frame = answers.next() => match frame {
            Some(Ok(frame)) => frame["closed"].as_str().unwrap_or("the tunnel ended").to_owned(),
            Some(Err(why)) => why.to_string(),
            None => format!("{} closed it", view.machine),
        },
        why = link.closed() => why,
        _ = closing.changed() => closing.borrow().clone().unwrap_or_default(),
    };
    // The listener and what it carries go; the request's stream is dropped with `answers`, which
    // tells the machine.
    accepting.abort();
    reason
}

/// Takes the connections of the local port: each one is a stream of the link.
async fn accept(listener: TcpListener, link: Link, tunnel: String) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((tcp, _)) = accepted else { continue };
                let (link, tunnel) = (link.clone(), tunnel.clone());
                connections.spawn(async move { carry(link, &tunnel, tcp).await });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

/// One TCP connection, as one stream of the link; no word of why when it fails: the machine's log
/// says.
async fn carry(link: Link, tunnel: &str, mut tcp: TcpStream) {
    let _ = tcp.set_nodelay(true);
    let Ok(mut stream) = link.tunnel(tunnel).await else {
        return;
    };
    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
}

/// Listens on `127.0.0.1` only: `port` when it is free, another one otherwise. The port it got.
fn bind_local(port: u16) -> std::io::Result<(TcpListener, u16)> {
    let bind = |port| {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        Ok::<_, std::io::Error>((TcpListener::from_std(listener)?, port))
    };
    bind(port).or_else(|_| bind(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed() -> Value {
        json!({"commands": [
            {"run": "r1", "state": "running", "url": "http://localhost:5173/"},
            {"run": "r2", "state": "running", "url": null},
            {"run": "r3", "state": "exited", "url": "http://localhost:3000/"},
        ]})
    }

    #[test]
    fn a_link_reaches_the_run_that_reports_its_port_else_the_only_one_running() {
        assert_eq!(run_for_port(&listed(), 5173, "server").unwrap(), "r1");
        // Two run: a port nobody reports is no guess.
        let why = run_for_port(&listed(), 9000, "server").unwrap_err();
        assert_eq!(why.to_string(), "no run of server serves port 9000");
        // One runs: it is the one taken.
        let one = json!({"commands": [{"run": "r2", "state": "running", "url": null}]});
        assert_eq!(run_for_port(&one, 9000, "server").unwrap(), "r2");
        // An ended run serves nothing.
        let ended = json!({"commands": [{"run": "r3", "state": "exited", "url": "http://localhost:3000/"}]});
        assert!(run_for_port(&ended, 3000, "server").is_err());
    }
}
