//! The address a run reports (`http://localhost:3000/`), read for the machine it is on: whether
//! it names that machine's own loopback, and the port it writes. The daemon takes the port of a
//! tunnel from it (`docs/remote.md`, "Tunnels"), and the window decides from it whether a link of
//! another machine opens through one.

/// The host and the port written in an address, the host without its brackets and in lower
/// case; the port is none when it is not written or is no number.
fn authority(url: &str) -> Option<(String, Option<u16>)> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']')?;
            (host, after.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    Some((
        host.to_ascii_lowercase(),
        port.and_then(|port| port.parse().ok()),
    ))
}

/// Whether the host is a name of the machine it is read on: `localhost`, a name under it,
/// `127.0.0.1`, `::1` or `0.0.0.0`.
fn here(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || matches!(host, "127.0.0.1" | "::1" | "0.0.0.0")
}

/// Whether the address names the machine it is read on, with a port written or not.
pub fn is_local(url: &str) -> bool {
    authority(url).is_some_and(|(host, _)| here(&host))
}

/// The port of an address that names the machine it is read on, when one is written.
pub fn local_port(url: &str) -> Option<u16> {
    authority(url)
        .filter(|(host, _)| here(host))
        .and_then(|(_, port)| port)
}

/// The same address read on another machine, reached through a tunnel whose local end is
/// `127.0.0.1:<local_port>`: its port is the tunnel's and its host `localhost`, except a name
/// under `.localhost` (`app.localhost`), which a server may tell apart from another and is kept.
/// The scheme, the path, the query and the fragment are the address's own.
pub fn through(url: &str, local_port: u16) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((scheme, rest)) => (format!("{scheme}://"), rest),
        None => (String::new(), url),
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority_text, tail) = rest.split_at(end);
    let (login, host_part) = match authority_text.rsplit_once('@') {
        Some((login, host)) => (format!("{login}@"), host),
        None => (String::new(), authority_text),
    };
    let host = match authority(url) {
        Some((host, _)) if host.ends_with(".localhost") => {
            // Written as it was, in its own case.
            let written = host_part
                .rsplit_once(':')
                .map_or(host_part, |(name, _)| name);
            written.to_owned()
        }
        _ => "localhost".to_owned(),
    };
    format!("{scheme}{login}{host}:{local_port}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_s_address_is_a_port_of_this_machine_or_none() {
        assert_eq!(local_port("http://localhost:3000/"), Some(3000));
        assert_eq!(local_port("http://127.0.0.1:8080"), Some(8080));
        assert_eq!(local_port("https://app.localhost:5173/x?y=1"), Some(5173));
        assert_eq!(local_port("http://[::1]:9000/"), Some(9000));
        assert_eq!(local_port("http://0.0.0.0:4000"), Some(4000));
        // No port written, a host of another machine, a name that only ends like it.
        assert_eq!(local_port("http://localhost/"), None);
        assert_eq!(local_port("https://example.org:8443/"), None);
        assert_eq!(local_port("http://evil-localhost:80/"), None);
        assert_eq!(local_port("http://user@example.org:8443/"), None);
        assert_eq!(local_port("not a url"), None);
    }

    #[test]
    fn a_backslash_ends_the_authority_as_the_web_view_reads_it() {
        // A web view reads `\` as `/`: this host is `evil.example`, whatever follows.
        assert!(!is_local("http://evil.example\\@localhost:5173/"));
        assert_eq!(local_port("http://evil.example\\@localhost:5173/"), None);
        assert!(!is_local("http://evil.example\\localhost:5173/"));
        // And the host before it is the address's own.
        assert_eq!(local_port("http://localhost:5173\\docs"), Some(5173));
        assert_eq!(
            through("http://localhost:5173\\docs?a=1", 41000),
            "http://localhost:41000\\docs?a=1"
        );
    }

    #[test]
    fn an_address_with_a_login_names_no_machine_of_ours() {
        assert!(!is_local("http://user@localhost:5173/"));
        assert_eq!(local_port("http://user:pw@127.0.0.1:8080/"), None);
        assert!(!is_local("http://localhost@evil.example/"));
    }

    #[test]
    fn an_address_names_this_machine_with_a_port_or_without() {
        assert!(is_local("http://localhost/"));
        assert!(is_local("http://localhost:3000/x"));
        assert!(is_local("https://web.localhost"));
        assert!(is_local("http://[::1]/"));
        assert!(is_local("http://127.0.0.1"));
        assert!(!is_local("https://example.org/t/42"));
        assert!(!is_local("http://evil-localhost:80/"));
        assert!(!is_local("http://localhost.example.org:80/"));
        assert!(!is_local("/work/app/report.html"));
    }

    #[test]
    fn an_address_through_a_tunnel_keeps_what_follows_its_port() {
        assert_eq!(
            through("http://localhost:5173/docs?a=1#top", 41000),
            "http://localhost:41000/docs?a=1#top"
        );
        assert_eq!(
            through("http://127.0.0.1:8080", 41000),
            "http://localhost:41000"
        );
        assert_eq!(
            through("http://[::1]:9000/x", 41000),
            "http://localhost:41000/x"
        );
        assert_eq!(
            through("http://0.0.0.0:4000/", 41000),
            "http://localhost:41000/"
        );
        // A name under `.localhost` tells one server from another: it stays.
        assert_eq!(
            through("https://Web.App.localhost:5173/login", 41000),
            "https://Web.App.localhost:41000/login"
        );
    }
}
