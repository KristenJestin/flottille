//! What makes two repositories on two machines the same one: the address of their remote, said
//! alike whatever the way it was written (`docs/remote.md`, "One list of every machine"). The
//! scheme, the user and the port, the `.git` suffix, the slashes at its ends and the case of the
//! host do not tell repositories apart; the case of the path does, as it does on a forge.

/// `host/path` of a remote's address, or `None` when it names no host (a path of the machine, a
/// `file://` address): a repository that has none is the same as no other.
///
/// `https://GitHub.com/acme/app.git`, `git@github.com:acme/app.git` and `ssh://git@github.com:22/acme/app`
/// all say `github.com/acme/app`.
pub fn normalised(url: &str) -> Option<String> {
    let url = url.trim();
    let (host, path) = match url.split_once("://") {
        Some(("file", _)) => return None,
        Some((_, rest)) => rest.split_once('/')?,
        // `user@host:path`: no scheme, a colon before any slash, and not a path of the machine.
        None => {
            let (before, path) = url.split_once(':')?;
            if before.contains('/') || before.is_empty() {
                return None;
            }
            (before, path)
        }
    };
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    let host = match host.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
        _ => host,
    };
    let path = path.trim_matches('/');
    let path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{path}", host.to_ascii_lowercase()))
}

/// The key of a project: its repositories' remotes, normalised, sorted and each once. `None` when
/// one of its repositories has no remote to say it by (it then matches no other project), or when
/// it has no repository.
pub fn project_key<'a>(remotes: impl IntoIterator<Item = Option<&'a str>>) -> Option<Vec<String>> {
    let mut keys = Vec::new();
    for remote in remotes {
        keys.push(normalised(remote?)?);
    }
    keys.sort();
    keys.dedup();
    (!keys.is_empty()).then_some(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_remote_written_four_ways_is_one_key() {
        let writings = [
            "https://github.com/acme/app.git",
            "https://GitHub.com/acme/app",
            "git@github.com:acme/app.git",
            "ssh://git@github.com:22/acme/app/",
            "https://user:token@github.com/acme/app.git",
        ];
        for writing in writings {
            assert_eq!(
                normalised(writing).as_deref(),
                Some("github.com/acme/app"),
                "{writing}"
            );
        }
    }

    #[test]
    fn the_case_of_the_path_tells_repositories_apart() {
        assert_ne!(
            normalised("https://github.com/acme/App"),
            normalised("https://github.com/acme/app")
        );
    }

    #[test]
    fn an_address_that_names_no_host_has_no_key() {
        for writing in [
            "/srv/git/app.git",
            "../app",
            "file:///srv/git/app.git",
            "",
            "app",
        ] {
            assert_eq!(normalised(writing), None, "{writing}");
        }
    }

    #[test]
    fn a_project_matches_when_all_its_repositories_do() {
        let both = project_key([
            Some("git@h.example:a/one.git"),
            Some("https://h.example/a/two"),
        ]);
        let swapped = project_key([Some("https://h.example/a/two.git"), Some("h.example:a/one")]);
        assert!(both.is_some());
        assert_eq!(both, swapped);
        assert_ne!(both, project_key([Some("https://h.example/a/one")]));
    }

    #[test]
    fn a_project_with_a_repository_that_has_no_remote_has_no_key() {
        assert_eq!(project_key([Some("https://h.example/a/one"), None]), None);
        assert_eq!(project_key([]), None);
    }
}
