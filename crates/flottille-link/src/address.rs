//! A machine's address as the user declares it and as machines tell each other
//! (`docs/remote.md`, "The transport"): a host, an IP address or a DNS name, and a port when it is
//! not the link's own.

use std::fmt;
use std::net::{IpAddr, SocketAddr};

/// A host and, when it says one, its port.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Address {
    host: Host,
    port: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Host {
    Ip(IpAddr),
    Name(String),
}

impl Address {
    /// `100.64.0.7`, `100.64.0.7:7447`, `server.example.net:7447`, `[fd7a::7]:7447`, `fd7a::7`.
    pub fn parse(text: &str) -> Result<Address, String> {
        let refused = || {
            format!(
                "{text} is not an address: give a host name or an IP address, and its port if it \
                 is not the link's own (100.64.0.7, server.example.net:7447)"
            )
        };
        if let Ok(socket) = text.parse::<SocketAddr>() {
            return Ok(Address::from(socket));
        }
        if let Ok(ip) = text.parse::<IpAddr>() {
            return Ok(Address {
                host: Host::Ip(ip),
                port: None,
            });
        }
        if let Some(inner) = text
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let ip = inner.parse::<IpAddr>().map_err(|_| refused())?;
            return Ok(Address {
                host: Host::Ip(ip),
                port: None,
            });
        }
        let (name, port) = match text.rsplit_once(':') {
            Some((name, port)) => {
                let port = port
                    .parse::<u16>()
                    .ok()
                    .filter(|port| *port != 0)
                    .ok_or_else(refused)?;
                (name, Some(port))
            }
            None => (text, None),
        };
        if !is_name(name) {
            return Err(refused());
        }
        Ok(Address {
            host: Host::Name(name.to_ascii_lowercase()),
            port,
        })
    }

    /// Its IP address, when it is one rather than a name.
    pub fn ip(&self) -> Option<IpAddr> {
        match &self.host {
            Host::Ip(ip) => Some(*ip),
            Host::Name(_) => None,
        }
    }

    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// This address, with `port` when it says none.
    pub fn with_port(&self, port: u16) -> Address {
        Address {
            host: self.host.clone(),
            port: self.port.or(Some(port)),
        }
    }

    /// The sockets it leads to, `port` when it says none: a name is looked up as the system
    /// looks names up, nothing more.
    pub async fn resolve(&self, port: u16) -> Vec<SocketAddr> {
        let port = self.port.unwrap_or(port);
        match &self.host {
            Host::Ip(ip) => vec![SocketAddr::new(*ip, port)],
            Host::Name(name) => tokio::net::lookup_host((name.as_str(), port))
                .await
                .map(|found| found.collect())
                .unwrap_or_default(),
        }
    }
}

impl From<SocketAddr> for Address {
    fn from(socket: SocketAddr) -> Address {
        Address {
            host: Host::Ip(socket.ip()),
            port: Some(socket.port()),
        }
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.host, self.port) {
            (Host::Ip(IpAddr::V6(ip)), Some(port)) => write!(f, "[{ip}]:{port}"),
            (Host::Ip(ip), Some(port)) => write!(f, "{ip}:{port}"),
            (Host::Ip(ip), None) => write!(f, "{ip}"),
            (Host::Name(name), Some(port)) => write!(f, "{name}:{port}"),
            (Host::Name(name), None) => f.write_str(name),
        }
    }
}

/// A DNS name: labels of letters, digits and hyphens, none at either end of a label.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_a_host_and_maybe_a_port() {
        for (text, shown) in [
            ("100.64.0.7", "100.64.0.7"),
            ("100.64.0.7:7447", "100.64.0.7:7447"),
            ("Server.Example.net:7447", "server.example.net:7447"),
            ("localhost", "localhost"),
            ("fd7a::7", "fd7a::7"),
            ("[fd7a::7]", "fd7a::7"),
            ("[fd7a::7]:7447", "[fd7a::7]:7447"),
        ] {
            assert_eq!(Address::parse(text).unwrap().to_string(), shown, "{text}");
        }
        for text in [
            "",
            "local host",
            "server:",
            "server:0",
            "server:70000",
            "-server",
            "a..b",
            "http://server",
            "[server]",
        ] {
            let refused = Address::parse(text).unwrap_err();
            assert!(refused.contains("is not an address"), "{text}: {refused}");
        }
    }

    #[test]
    fn an_address_without_a_port_takes_the_links() {
        let address = Address::parse("100.64.0.7").unwrap();
        assert_eq!(address.with_port(7447).to_string(), "100.64.0.7:7447");
        let address = Address::parse("100.64.0.7:4000").unwrap();
        assert_eq!(address.with_port(7447).to_string(), "100.64.0.7:4000");
    }
}
