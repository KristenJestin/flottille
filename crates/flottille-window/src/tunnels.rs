//! The tunnels the window holds to the runs of other machines (`docs/remote.md`, "Tunnels"): a
//! run's address, or a `localhost` link an agent of that machine showed, is opened through one,
//! then the system's default handler opens the local address. The window asks for it with the
//! same signed request the command line uses (`tunnel.open`), on a connection of its own that it
//! keeps for as long as the tunnel is wanted: Close, or the window quitting, ends it.
//!
//! What the window knows of them is a global ([`Tunnels`]), so that every row that draws a run
//! says its tunnel ([`TunnelLines`]) without each page being given them.

use gpui_design_system::{Button, Item, Listing, Palette, Toast, Type as _, space, toast};
use gpui_kit::{
    AnyElement, App, Context, ElementId, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Subscription, TestSupportExt as _,
    WeakEntity, div,
};
use serde_json::Value;

use crate::shell::Shell;

/// One tunnel the window holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Tunnel {
    pub id: String,
    /// The machine of the run.
    pub machine: String,
    pub run: String,
    /// The port the window asked for, none for the run's own address.
    pub asked: Option<u16>,
    /// The run's port, on its machine.
    pub port: u16,
    /// The port this machine listens on, on `127.0.0.1`.
    pub local_port: u16,
}

impl Tunnel {
    /// What the row says: `localhost:5173 → server`.
    pub fn said(&self) -> String {
        format!("localhost:{} → {}", self.local_port, self.machine)
    }

    /// The tunnel the daemon answered a request with.
    pub fn of(ask: &Ask, run: &str, answer: &Value) -> Result<Tunnel, String> {
        let port = |key: &str| {
            answer[key]
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
        };
        match (answer["tunnel"].as_str(), port("port"), port("local_port")) {
            (Some(id), Some(port), Some(local_port)) => Ok(Tunnel {
                id: id.to_owned(),
                machine: ask.machine.clone(),
                run: run.to_owned(),
                asked: ask.port.filter(|_| ask.run.is_some()),
                port,
                local_port,
            }),
            _ => Err(format!("{} answered what is not a tunnel", ask.machine)),
        }
    }
}

impl Tunnel {
    /// A tunnel as `tunnel.list` says it: the ones this machine carries, whoever opened them.
    pub fn listed(value: &Value) -> Option<Tunnel> {
        let port = |key: &str| {
            value[key]
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
        };
        Some(Tunnel {
            id: value["tunnel"].as_str()?.to_owned(),
            machine: value["machine"].as_str()?.to_owned(),
            run: value["run"].as_str().unwrap_or_default().to_owned(),
            asked: None,
            port: port("port")?,
            local_port: port("local_port")?,
        })
    }
}

/// What to do with the daemon's answer to a request of the Tunnels section.
#[derive(Clone, Debug, PartialEq)]
pub enum Then {
    /// `tunnel.list`: every tunnel this machine carries.
    Listed,
    /// `tunnel.close` of the tunnel.
    Closed { id: String },
}

/// What the user wants to reach on a machine, and what to open once it is reached.
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    pub machine: String,
    /// The run whose port it is; none for a link, whose run is the one that serves `port`.
    pub run: Option<String>,
    pub port: Option<u16>,
    /// The address the user clicked: what the system's handler is given, once it is read through
    /// the tunnel.
    pub url: String,
}

impl Ask {
    /// Whether `tunnel` already reaches what is asked.
    fn reached_by(&self, tunnel: &Tunnel) -> bool {
        tunnel.machine == self.machine
            && match (&self.run, self.port) {
                (Some(run), asked) => tunnel.run == *run && tunnel.asked == asked,
                (None, Some(port)) => tunnel.port == port,
                (None, None) => false,
            }
    }

    /// Whether `other` asks for the same thing.
    fn same_as(&self, other: &Ask) -> bool {
        self.machine == other.machine && self.run == other.run && self.port == other.port
    }
}

/// What a connection that holds a tunnel tells the window.
#[derive(Clone, Debug, PartialEq)]
pub enum News {
    Opened {
        ask: Ask,
        tunnel: Tunnel,
    },
    /// It could not be opened, and why.
    Refused {
        ask: Ask,
        why: String,
    },
    /// It ended: Close, the run's end, the link's loss.
    Closed {
        tunnel: String,
        reason: String,
    },
    /// The daemon refused to close it, and why.
    NotClosed {
        why: String,
    },
}

/// The tunnels the window holds and those it is opening, and the shell that asks for them.
pub struct Tunnels {
    shell: WeakEntity<Shell>,
    open: Vec<Tunnel>,
    opening: Vec<Ask>,
}

impl gpui_kit::Global for Tunnels {}

impl Tunnels {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Tunnels {
            shell,
            open: Vec::new(),
            opening: Vec::new(),
        }
    }

    /// The tunnels held, in the order they were opened.
    pub fn open(&self) -> &[Tunnel] {
        &self.open
    }

    /// The tunnel that already reaches what `ask` is for.
    pub fn reaching(&self, ask: &Ask) -> Option<&Tunnel> {
        self.open.iter().find(|tunnel| ask.reached_by(tunnel))
    }

    /// Whether `ask`'s tunnel is being opened: a second click waits for the first.
    pub fn is_opening(&self, ask: &Ask) -> bool {
        self.opening.iter().any(|opening| opening.same_as(ask))
    }

    pub fn begin(&mut self, ask: Ask) {
        self.opening.push(ask);
    }

    pub fn opened(&mut self, ask: &Ask, tunnel: Tunnel) {
        self.opening.retain(|opening| !opening.same_as(ask));
        self.open.retain(|held| held.id != tunnel.id);
        self.open.push(tunnel);
    }

    pub fn refused(&mut self, ask: &Ask) {
        self.opening.retain(|opening| !opening.same_as(ask));
    }

    pub fn closed(&mut self, tunnel: &str) {
        self.open.retain(|held| held.id != tunnel);
    }
}

/// Opens an address of the machine shown: a run's own (`run` given) or one a link names. The
/// shell decides whether a tunnel is needed.
pub fn open_address(run: Option<&str>, url: &str, cx: &mut App) {
    let Some(shell) = cx.try_global::<Tunnels>().map(|held| held.shell.clone()) else {
        return;
    };
    let (run, url) = (run.map(str::to_owned), url.to_owned());
    shell
        .update(cx, |shell, cx| shell.open_address(run.as_deref(), &url, cx))
        .ok();
}

/// Opens a link written in a Markdown text (a reply, a description): only a web address whose
/// authority names no user (`flottille_core::address::web_authority`) opens, through
/// [`open_address`] so another machine's `localhost` goes through its tunnel. Any other scheme, or
/// an address that reads as another place than it goes to, opens nothing.
pub fn open_markdown_link(url: &str, cx: &mut App) {
    if flottille_core::address::web_authority(url).is_none() {
        return;
    }
    open_address(None, url, cx);
}

/// Opens what an agent showed, after the checks Open makes (`Shell::open_shown`).
pub fn open_shown(target: &str, cx: &mut App) {
    let Some(shell) = cx.try_global::<Tunnels>().map(|held| held.shell.clone()) else {
        return;
    };
    let target = target.to_owned();
    shell
        .update(cx, |shell, cx| shell.open_shown(&target, cx))
        .ok();
}

/// Closes a tunnel the window holds.
pub fn close(tunnel: &str, cx: &mut App) {
    let Some(shell) = cx.try_global::<Tunnels>().map(|held| held.shell.clone()) else {
        return;
    };
    let tunnel = tunnel.to_owned();
    shell
        .update(cx, |shell, cx| shell.close_tunnel(&tunnel, cx))
        .ok();
}

/// A tunnel that could not open or close is a toast with the reason.
pub fn failed(why: &str, cx: &mut App) {
    toast(Toast::failure(format!("Not opened: {why}")), cx);
}

/// The run of `machine` that serves `port`, among the commands `run.list` answered: the one that
/// reports an address on that port, else the only one that runs. The machine of the run checks
/// the port again, so a wrong guess is refused there, never reached.
pub fn run_for_port(listed: &Value, port: u16, machine: &str) -> Result<String, String> {
    let running: Vec<&Value> = listed["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|command| command["state"] == "running")
        .collect();
    let named = |command: &&Value| command["run"].as_str().map(str::to_owned);
    running
        .iter()
        .find(|command| {
            command["url"]
                .as_str()
                .and_then(flottille_core::address::local_port)
                == Some(port)
        })
        .or_else(|| (running.len() == 1).then(|| &running[0]))
        .and_then(named)
        .ok_or_else(|| format!("no run of {machine} serves port {port}"))
}

/// The tunnel lines of the rows that draw runs: each run says the tunnels held to it, under its
/// own lines. A line unfolds and fades in when its tunnel opens and folds away when it closes.
#[derive(Default)]
pub struct TunnelLines {
    lines: Listing<Tunnel>,
    watching: Option<Subscription>,
}

impl TunnelLines {
    /// Takes the tunnels held to the machine shown: on each render of the view that draws them,
    /// which this makes draw again when they change.
    pub fn sync<T: 'static>(&mut self, cx: &mut Context<T>) {
        if self.watching.is_none() {
            self.watching = Some(cx.observe_global::<Tunnels>(|_, cx| cx.notify()));
        }
        let shown = crate::machines::shown_machine(cx);
        let held: Vec<(String, Tunnel)> = cx
            .try_global::<Tunnels>()
            .map(|held| {
                held.open()
                    .iter()
                    .filter(|tunnel| Some(&tunnel.machine) == shown.as_ref())
                    .map(|tunnel| (tunnel.id.clone(), tunnel.clone()))
                    .collect()
            })
            .unwrap_or_default();
        self.lines
            .sync(held, cx.background_executor().now(), cx.reduce_motion());
    }

    /// The lines of the tunnels held to `run`, each in its `Item`.
    pub fn of_run(&self, run: &str, p: Palette) -> Vec<AnyElement> {
        self.lines
            .rows()
            .filter(|row| row.value.run == run)
            .map(|row| {
                let tunnel = row.value;
                Item::new(
                    ElementId::Name(format!("tunnel-item-{}", tunnel.id).into()),
                    row.placement,
                    line(tunnel, p),
                )
                .into_any_element()
            })
            .collect()
    }
}

/// One tunnel's line: where it opens and to which machine, and Close.
fn line(tunnel: &Tunnel, p: Palette) -> AnyElement {
    let said = tunnel.said();
    let id = tunnel.id.clone();
    div()
        .id(ElementId::Name(format!("tunnel-{}", tunnel.id).into()))
        .flex()
        .items_center()
        .gap(space::SM)
        .pt(space::XS)
        .mono(11.)
        .text_color(p.faint)
        .aria_label(SharedString::from(said.clone()))
        .child(div().min_w_0().truncate().child(SharedString::from(said)))
        .child(
            Button::quiet(
                ElementId::Name(format!("tunnel-{}-close", tunnel.id).into()),
                "Close",
            )
            .on_click(move |_, _, cx| {
                // The row's own click opens its drawer: this one only closes the tunnel.
                cx.stop_propagation();
                close(&id, cx);
            }),
        )
        .test_support()
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tunnel(id: &str, run: &str, asked: Option<u16>, port: u16) -> Tunnel {
        Tunnel {
            id: id.into(),
            machine: "server".into(),
            run: run.into(),
            asked,
            port,
            local_port: 41000,
        }
    }

    #[test]
    fn a_tunnel_is_said_with_its_local_address_and_its_machine() {
        assert_eq!(
            tunnel("t1", "r1", None, 5173).said(),
            "localhost:41000 → server"
        );
    }

    #[test]
    fn a_run_s_own_address_and_a_link_reach_the_same_tunnel() {
        let held = tunnel("t1", "r1", None, 5173);
        let row = Ask {
            machine: "server".into(),
            run: Some("r1".into()),
            port: None,
            url: "http://localhost:5173".into(),
        };
        let link = Ask {
            machine: "server".into(),
            run: None,
            port: Some(5173),
            url: "http://localhost:5173/docs".into(),
        };
        assert!(row.reached_by(&held));
        assert!(link.reached_by(&held));
        // Another run, another port, another machine.
        assert!(
            !Ask {
                run: Some("r2".into()),
                ..row.clone()
            }
            .reached_by(&held)
        );
        assert!(
            !Ask {
                port: Some(3000),
                ..link.clone()
            }
            .reached_by(&held)
        );
        assert!(
            !Ask {
                machine: "laptop".into(),
                ..link
            }
            .reached_by(&held)
        );
    }

    #[test]
    fn a_link_reaches_the_run_that_reports_its_port_else_the_only_one_running() {
        let listed = json!({"commands": [
            {"run": "r1", "state": "running", "url": "http://localhost:3000"},
            {"run": "r2", "state": "running", "url": "http://localhost:5173/x"},
            {"run": "r3", "state": "exited", "url": "http://localhost:4000"}]});
        assert_eq!(run_for_port(&listed, 5173, "server").unwrap(), "r2");
        assert_eq!(
            run_for_port(&listed, 4000, "server").unwrap_err(),
            "no run of server serves port 4000"
        );
        let alone = json!({"commands": [{"run": "r9", "state": "running"}]});
        assert_eq!(run_for_port(&alone, 8080, "server").unwrap(), "r9");
        let none = json!({"commands": []});
        assert!(run_for_port(&none, 8080, "server").is_err());
    }

    #[test]
    fn the_daemon_s_answer_is_a_tunnel_or_it_is_refused() {
        let ask = Ask {
            machine: "server".into(),
            run: Some("r1".into()),
            port: None,
            url: "http://localhost:5173".into(),
        };
        let answer = json!({"tunnel": "t1", "machine": "server", "run": "r1", "port": 5173,
            "local_port": 41000, "address": "127.0.0.1:41000"});
        assert_eq!(
            Tunnel::of(&ask, "r1", &answer).unwrap(),
            tunnel("t1", "r1", None, 5173)
        );
        assert!(Tunnel::of(&ask, "r1", &json!({})).is_err());
    }
}
