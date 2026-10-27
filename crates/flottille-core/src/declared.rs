//! What a project declares of its forges and its tracker (`docs/projects.md`, "Forges and
//! tracker"). Flottille never guesses them: a repository is read and written on the forge its
//! entry declares, tickets are read on the tracker the project declares, and nothing else.
//!
//! A remote's address still serves twice: to propose a declaration (the window pre-fills its
//! settings with it, the user saves it or not), and to check that a declared forge is the one
//! the remote leads to. A declaration that disagrees with its remote is refused with the reason;
//! neither wins silently, since a procedure writes to the forge.

use serde::{Deserialize, Serialize};

use crate::ticket::Key;

/// Bitbucket Cloud's host.
pub const BITBUCKET_CLOUD: &str = "bitbucket.org";
/// GitHub's host.
pub const GITHUB: &str = "github.com";

/// The forge one repository of a project is on, as its entry declares it:
/// `forge = { kind = "github", repository = "acme/api" }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RepositoryForge {
    /// `owner/name`.
    Github { repository: String },
    /// `workspace/name`.
    BitbucketCloud { repository: String },
    /// `PROJECT/name` on the server `host` (a host name, no scheme, port nor path).
    BitbucketDataCenter { host: String, repository: String },
}

impl RepositoryForge {
    /// `github`, `bitbucket-cloud` or `bitbucket-data-center`, as the file writes it.
    pub fn kind(&self) -> &'static str {
        match self {
            RepositoryForge::Github { .. } => "github",
            RepositoryForge::BitbucketCloud { .. } => "bitbucket-cloud",
            RepositoryForge::BitbucketDataCenter { .. } => "bitbucket-data-center",
        }
    }

    /// The host the forge is on, lowercase once checked.
    pub fn host(&self) -> &str {
        match self {
            RepositoryForge::Github { .. } => GITHUB,
            RepositoryForge::BitbucketCloud { .. } => BITBUCKET_CLOUD,
            RepositoryForge::BitbucketDataCenter { host, .. } => host,
        }
    }

    /// `owner/name`, `workspace/name` or `PROJECT/name`.
    pub fn repository(&self) -> &str {
        match self {
            RepositoryForge::Github { repository }
            | RepositoryForge::BitbucketCloud { repository }
            | RepositoryForge::BitbucketDataCenter { repository, .. } => repository,
        }
    }

    /// The repository's two names.
    pub fn owner_and_name(&self) -> (&str, &str) {
        self.repository()
            .split_once('/')
            .unwrap_or((self.repository(), ""))
    }

    /// As a sentence says it: `GitHub acme/api`, `Bitbucket Data Center PROJ/api on host`.
    pub fn said(&self) -> String {
        match self {
            RepositoryForge::Github { repository } => format!("GitHub {repository}"),
            RepositoryForge::BitbucketCloud { repository } => {
                format!("Bitbucket Cloud {repository}")
            }
            RepositoryForge::BitbucketDataCenter { host, repository } => {
                format!("Bitbucket Data Center {repository} on {host}")
            }
        }
    }

    /// Checks the declaration and puts its host in lowercase. The error is `.<key>: <what>`, to
    /// follow the key of the entry.
    pub fn check(&mut self) -> Result<(), String> {
        if let RepositoryForge::BitbucketDataCenter { host, .. } = self {
            *host = host.trim().to_ascii_lowercase();
            check_host(host).map_err(|what| format!(".host: {what}"))?;
            if host == BITBUCKET_CLOUD || host == GITHUB {
                return Err(format!(
                    ".host: {host} is not a Bitbucket Data Center server: use the kind of its \
                     forge"
                ));
            }
        }
        let shape = match self {
            RepositoryForge::Github { .. } => "owner/name",
            RepositoryForge::BitbucketCloud { .. } => "workspace/name",
            RepositoryForge::BitbucketDataCenter { .. } => "PROJECT/name",
        };
        match self.repository().split_once('/') {
            Some((owner, name)) if word(owner) && word(name) => Ok(()),
            _ => Err(format!(
                ".repository: must be {shape}, each a name of letters, digits, '.', '_', '~' or \
                 '-' not starting with '-', not \"{}\"",
                self.repository()
            )),
        }
    }

    /// Whether the remote's address `url` leads to this forge and repository: same host, same
    /// repository (case aside, as the forges take it). The error says where it leads instead.
    pub fn agrees(&self, url: &str) -> Result<(), String> {
        let Some((host, path)) = address(url) else {
            return Err(format!(
                "it leads to {}, which is not on a forge",
                url.trim()
            ));
        };
        let found = if host != self.host() {
            None
        } else {
            match self {
                RepositoryForge::Github { .. } => two_names(&path),
                RepositoryForge::BitbucketCloud { .. } => bitbucket_names(true, &path),
                RepositoryForge::BitbucketDataCenter { .. } => bitbucket_names(false, &path),
            }
        };
        let (owner, name) = self.owner_and_name();
        match found {
            Some((o, n)) if o.eq_ignore_ascii_case(owner) && n.eq_ignore_ascii_case(name) => Ok(()),
            _ => Err(format!("it leads to {host}/{path}, not to {}", self.said())),
        }
    }

    /// What a remote's address suggests, for the user to confirm: GitHub on `github.com`,
    /// Bitbucket Cloud on `bitbucket.org`, Bitbucket Data Center on another host when the address
    /// has Data Center's shape (`/scm/`, `projects/…/repos/…`, or SSH on port 7999). Nothing is
    /// read or written on a proposal.
    pub fn proposed(url: &str) -> Option<RepositoryForge> {
        let (host, path) = address(url)?;
        let repository = |(owner, name): (String, String)| format!("{owner}/{name}");
        match host.as_str() {
            GITHUB => two_names(&path).map(|names| RepositoryForge::Github {
                repository: repository(names),
            }),
            BITBUCKET_CLOUD => {
                bitbucket_names(true, &path).map(|names| RepositoryForge::BitbucketCloud {
                    repository: repository(names),
                })
            }
            _ => {
                let lower = path.to_ascii_lowercase();
                let shaped = lower.starts_with("scm/")
                    || lower.contains("/scm/")
                    || lower.contains("projects/")
                    || url.contains(":7999/");
                if !shaped {
                    return None;
                }
                bitbucket_names(false, &path).map(|names| RepositoryForge::BitbucketDataCenter {
                    host,
                    repository: repository(names),
                })
            }
        }
    }
}

/// The tracker of a project's tickets, as its `[tracker]` table declares it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ProjectTracker {
    /// The issues of one repository of the project, by its path; its forge is declared GitHub.
    Github { repository: String },
    /// Jira Cloud on `site` (`example.atlassian.net`), its tickets in these projects only.
    Jira { site: String, projects: Vec<String> },
}

impl ProjectTracker {
    pub fn kind(&self) -> &'static str {
        match self {
            ProjectTracker::Github { .. } => "github",
            ProjectTracker::Jira { .. } => "jira",
        }
    }

    /// Checks the Jira part of the declaration (the repository of GitHub's is checked with the
    /// project's repositories) and puts the site in lowercase and the keys in uppercase. The error
    /// is `.<key>: <what>`.
    pub fn check(&mut self) -> Result<(), String> {
        let ProjectTracker::Jira { site, projects } = self else {
            return Ok(());
        };
        *site = site.trim().to_ascii_lowercase();
        check_host(site).map_err(|what| format!(".site: {what}"))?;
        if projects.is_empty() {
            return Err(".projects: must name at least one Jira project key, such as ABC".into());
        }
        for (index, key) in projects.iter_mut().enumerate() {
            *key = key.trim().to_ascii_uppercase();
            let valid = key.len() >= 2
                && key.starts_with(|c: char| c.is_ascii_alphabetic())
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid {
                return Err(format!(
                    ".projects[{index}]: must be a Jira project key (letters, digits and '_', \
                     starting with a letter, at least two), not \"{key}\""
                ));
            }
        }
        projects.dedup();
        Ok(())
    }

    /// As a sentence says it: `GitHub issues of api`, `Jira on example.atlassian.net (ABC, DEF)`.
    pub fn said(&self) -> String {
        match self {
            ProjectTracker::Github { repository } => format!("GitHub issues of {repository}"),
            ProjectTracker::Jira { site, projects } => {
                format!("Jira on {site} ({})", projects.join(", "))
            }
        }
    }

    /// Whether `key` is a ticket of this tracker: a number for GitHub issues, a key of one of the
    /// declared Jira projects. The error is the refusal, without `refused: `.
    pub fn accepts(&self, key: &Key) -> Result<(), String> {
        match (self, key) {
            (ProjectTracker::Github { .. }, Key::Issue(_)) => Ok(()),
            (ProjectTracker::Jira { projects, .. }, Key::Jira(text)) => {
                let project = text.rsplit_once('-').map_or("", |(project, _)| project);
                if projects.iter().any(|p| p == project) {
                    Ok(())
                } else {
                    Err(format!(
                        "{text} is not in a Jira project of this project's tracker ({}): its \
                         tickets are {}",
                        self.said(),
                        projects
                            .iter()
                            .map(|p| format!("{p}-123"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            }
            (ProjectTracker::Github { .. }, Key::Jira(text)) => Err(format!(
                "{text} is a Jira key, and this project's tracker is {}: write an issue number, \
                 #123",
                self.said()
            )),
            (ProjectTracker::Jira { .. }, Key::Issue(number)) => Err(format!(
                "#{number} is a GitHub issue, and this project's tracker is {}: write a key such \
                 as {}-123",
                self.said(),
                match self {
                    ProjectTracker::Jira { projects, .. } => projects[0].as_str(),
                    ProjectTracker::Github { .. } => "ABC",
                }
            )),
        }
    }
}

/// A host name alone: letters, digits, `.` and `-`.
fn check_host(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err("must not be empty".into());
    }
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        || host.starts_with(['-', '.'])
    {
        return Err(format!(
            "must be a host name (example.com), without a scheme, a port or a path, not \"{host}\""
        ));
    }
    Ok(())
}

/// A name a command line takes as one word, never as an option or a path.
fn word(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._~-".contains(c))
}

/// The host (lowercase, without user nor port) and the path (without its slashes at the ends
/// nor `.git`) of a remote's address: `git@host:path`, `ssh://git@host:7999/path`,
/// `https://host/path`. `None` for a path on this machine.
pub fn address(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        (host.split_once(':').map_or(host, |(host, _)| host), path)
    } else if let Some((before, path)) = url.split_once(':')
        && !before.is_empty()
        && !before.contains('/')
    {
        (
            before.rsplit_once('@').map_or(before, |(_, host)| host),
            path,
        )
    } else {
        return None;
    };
    let host = host.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    Some((host, path.to_owned()))
}

/// `owner/name`, exactly two names.
fn two_names(path: &str) -> Option<(String, String)> {
    match path.split('/').collect::<Vec<_>>()[..] {
        [owner, name] if word(owner) && word(name) => Some((owner.to_owned(), name.to_owned())),
        _ => None,
    }
}

/// The Bitbucket repository a remote's path names: `workspace/name` on Cloud; on Data Center
/// `PROJECT/name` over SSH, behind `scm/` over HTTPS (after the context path, if any), or a
/// `projects/PROJECT/repos/name` page's address.
fn bitbucket_names(cloud: bool, path: &str) -> Option<(String, String)> {
    let segments: Vec<&str> = path.split('/').collect();
    let at = |word: &str| segments.iter().position(|s| s.eq_ignore_ascii_case(word));
    let found = if cloud {
        match segments[..] {
            [owner, name] => Some((owner, name)),
            _ => None,
        }
    } else if let [owner, name] = segments[..]
        && !["scm", "projects"].contains(&owner.to_ascii_lowercase().as_str())
    {
        Some((owner, name))
    } else if let Some(at) = at("scm") {
        match segments[at + 1..] {
            [owner, name] => Some((owner, name)),
            _ => None,
        }
    } else if let Some(at) = at("projects") {
        match segments[at + 1..] {
            [owner, repos, name, ..] if repos.eq_ignore_ascii_case("repos") => Some((owner, name)),
            _ => None,
        }
    } else {
        None
    };
    match found {
        Some((owner, name)) if word(owner) && word(name) => {
            Some((owner.to_owned(), name.to_owned()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github(repository: &str) -> RepositoryForge {
        RepositoryForge::Github {
            repository: repository.into(),
        }
    }

    fn data_center(host: &str, repository: &str) -> RepositoryForge {
        RepositoryForge::BitbucketDataCenter {
            host: host.into(),
            repository: repository.into(),
        }
    }

    #[test]
    fn a_remote_proposes_its_forge_and_nothing_where_its_shape_says_none() {
        for url in [
            "git@github.com:acme/api.git",
            "ssh://git@github.com/acme/api",
            "https://token@github.com/acme/api.git",
        ] {
            assert_eq!(
                RepositoryForge::proposed(url),
                Some(github("acme/api")),
                "{url}"
            );
        }
        assert_eq!(
            RepositoryForge::proposed("https://bitbucket.org/acme/api.git"),
            Some(RepositoryForge::BitbucketCloud {
                repository: "acme/api".into()
            })
        );
        for url in [
            "ssh://git@bitbucket.example.com:7999/PROJ/api.git",
            "https://bitbucket.example.com/scm/PROJ/api.git",
            "https://bitbucket.example.com/context/scm/PROJ/api.git",
            "https://bitbucket.example.com/projects/PROJ/repos/api/browse",
        ] {
            assert_eq!(
                RepositoryForge::proposed(url),
                Some(data_center("bitbucket.example.com", "PROJ/api")),
                "{url}"
            );
        }
        for url in [
            "git@gitlab.com:acme/api.git",
            "https://git.example.com/acme/api.git",
            "/srv/git/api.git",
            "../api",
            "https://github.com/acme",
            "https://github.com/acme/api/extra",
            "git@github.com:acme/--upload-pack=x.git",
        ] {
            assert_eq!(RepositoryForge::proposed(url), None, "{url}");
        }
    }

    #[test]
    fn a_declared_forge_agrees_only_with_a_remote_on_the_same_host_and_repository() {
        assert_eq!(
            github("acme/api").agrees("git@github.com:Acme/API.git"),
            Ok(())
        );
        assert_eq!(
            data_center("bitbucket.example.com", "PROJ/api")
                .agrees("https://bitbucket.example.com/scm/proj/api.git"),
            Ok(())
        );
        assert_eq!(
            data_center("bitbucket.example.com", "PROJ/api")
                .agrees("ssh://git@bitbucket.example.com:7999/PROJ/api.git"),
            Ok(())
        );
        assert_eq!(
            github("acme/api").agrees("git@github.com:acme/web.git"),
            Err("it leads to github.com/acme/web, not to GitHub acme/api".into())
        );
        assert_eq!(
            github("acme/api").agrees("git@gitlab.com:acme/api.git"),
            Err("it leads to gitlab.com/acme/api, not to GitHub acme/api".into())
        );
        assert!(
            data_center("bitbucket.example.com", "PROJ/api")
                .agrees("https://other.example.com/scm/PROJ/api.git")
                .is_err()
        );
        assert_eq!(
            github("acme/api").agrees("/srv/git/api.git"),
            Err("it leads to /srv/git/api.git, which is not on a forge".into())
        );
    }

    #[test]
    fn a_declared_forge_names_one_repository_and_a_data_center_host_alone() {
        let mut fine = data_center("Bitbucket.Example.com", "PROJ/api");
        assert_eq!(fine.check(), Ok(()));
        assert_eq!(fine.host(), "bitbucket.example.com");
        for (mut forge, wrong) in [
            (github("acme"), ".repository: must be owner/name"),
            (github("acme/api/x"), ".repository: must be owner/name"),
            (github("-x/api"), ".repository: must be owner/name"),
            (
                data_center("https://h.example.com", "P/a"),
                ".host: must be a host name",
            ),
            (data_center("h.example.com:7990", "P/a"), ".host: must be"),
            (
                data_center("bitbucket.org", "P/a"),
                ".host: bitbucket.org is not",
            ),
        ] {
            let said = forge.check().unwrap_err();
            assert!(said.starts_with(wrong), "{forge:?}: {said}");
        }
    }

    #[test]
    fn a_ticket_of_another_tracker_or_jira_project_is_refused() {
        let jira = ProjectTracker::Jira {
            site: "example.atlassian.net".into(),
            projects: vec!["ABC".into()],
        };
        let issues = ProjectTracker::Github {
            repository: "api".into(),
        };
        assert_eq!(jira.accepts(&Key::parse("abc-12").unwrap()), Ok(()));
        assert_eq!(issues.accepts(&Key::parse("#12").unwrap()), Ok(()));
        let other = jira.accepts(&Key::parse("XYZ-1").unwrap()).unwrap_err();
        assert!(
            other.starts_with("XYZ-1 is not in a Jira project"),
            "{other}"
        );
        let number = jira.accepts(&Key::parse("#4").unwrap()).unwrap_err();
        assert!(number.starts_with("#4 is a GitHub issue"), "{number}");
        let key = issues.accepts(&Key::parse("ABC-1").unwrap()).unwrap_err();
        assert!(key.starts_with("ABC-1 is a Jira key"), "{key}");
    }

    #[test]
    fn a_jira_tracker_names_its_site_and_at_least_one_project_key() {
        let mut fine = ProjectTracker::Jira {
            site: "Example.Atlassian.net".into(),
            projects: vec!["abc".into(), "D_2".into()],
        };
        assert_eq!(fine.check(), Ok(()));
        assert_eq!(
            fine,
            ProjectTracker::Jira {
                site: "example.atlassian.net".into(),
                projects: vec!["ABC".into(), "D_2".into()],
            }
        );
        for (site, projects, wrong) in [
            ("", vec!["ABC"], ".site: must not be empty"),
            (
                "https://x.atlassian.net",
                vec!["ABC"],
                ".site: must be a host name",
            ),
            ("x.atlassian.net", vec![], ".projects: must name"),
            ("x.atlassian.net", vec!["A"], ".projects[0]: must be"),
            (
                "x.atlassian.net",
                vec!["ABC", "1A"],
                ".projects[1]: must be",
            ),
        ] {
            let mut tracker = ProjectTracker::Jira {
                site: site.into(),
                projects: projects.into_iter().map(str::to_owned).collect(),
            };
            let said = tracker.check().unwrap_err();
            assert!(said.starts_with(wrong), "{site} {said}");
        }
    }
}
