//! A project: a folder, its repositories, and the configuration Flottille keeps for it, one TOML
//! file per project (`docs/projects.md`).

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::{self, Deserializer, SeqAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// Children running at once per project: the bounds of `max_children`.
pub const CHILDREN_CAP: std::ops::RangeInclusive<u8> = 1..=6;
const DEFAULT_CHILDREN_CAP: u8 = 3;

/// A configuration that cannot be used, said with its file and, when there is one, its key.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigError {
    /// The project's name: its file is `projects/<name>.toml`.
    pub name: String,
    /// `<key>: <what is wrong>`, or just what is wrong when no key is to blame.
    pub message: String,
}

impl ConfigError {
    pub fn file(&self) -> String {
        format!("projects/{}.toml", self.name)
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.file(), self.message)
    }
}

impl std::error::Error for ConfigError {}

/// One git repository of a project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    /// Relative to the project folder, `.` for the folder itself.
    pub path: String,
    #[serde(default = "default_remote")]
    pub remote: String,
    /// Branches in order of preference: the first one that exists on the remote is the base.
    #[serde(default = "default_base")]
    pub base: Vec<String>,
    /// Paths of the repositories of the project this one is built on: they come first when
    /// preparing and merging.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

fn default_remote() -> String {
    "origin".into()
}

fn default_base() -> Vec<String> {
    vec!["main".into()]
}

/// A named command of the project, for the user and for agents.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogueCommand {
    /// One command line, run without a shell.
    pub line: String,
    /// Where it runs: the path of one of the repositories; the workspace folder when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Keeps running (a development server): started, not waited for.
    #[serde(default)]
    pub serve: bool,
    /// A check (tests, lint): says whether the work is sound.
    #[serde(default)]
    pub check: bool,
    /// Changes nothing, so a read-only place may run it.
    #[serde(default)]
    pub read_only: bool,
    /// Held for approval each time an agent runs it.
    #[serde(default)]
    pub ask_before_running: bool,
}

/// The repositories a new workspace takes.
#[derive(Clone, Debug, PartialEq)]
pub enum RepositorySet {
    All,
    Only(Vec<String>),
}

impl Serialize for RepositorySet {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            RepositorySet::All => serializer.serialize_str("all"),
            RepositorySet::Only(paths) => paths.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for RepositorySet {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Reading;
        impl<'de> Visitor<'de> for Reading {
            type Value = RepositorySet;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(r#""all" or a list of repository paths"#)
            }

            fn visit_str<E: de::Error>(self, word: &str) -> Result<Self::Value, E> {
                match word {
                    "all" => Ok(RepositorySet::All),
                    other => Err(E::invalid_value(de::Unexpected::Str(other), &self)),
                }
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut paths = Vec::new();
                while let Some(path) = seq.next_element()? {
                    paths.push(path);
                }
                Ok(RepositorySet::Only(paths))
            }
        }
        deserializer.deserialize_any(Reading)
    }
}

/// A project as Flottille uses it: checked, with every default filled in.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Project {
    pub name: String,
    /// Absolute.
    pub folder: PathBuf,
    /// Starts the branches of the project's workspaces; a slug of the name unless set.
    pub branch_prefix: String,
    /// Where the workspaces are made; absolute.
    pub workspaces_root: PathBuf,
    pub default_repositories: RepositorySet,
    /// Children running at once, within `CHILDREN_CAP`.
    pub max_children: u8,
    /// Programs with their leading arguments (`["git", "push"]`) that are refused to agents
    /// whatever else says; each entry is one word per element, never empty.
    pub never: Vec<Vec<String>>,
    /// At least one.
    pub repositories: Vec<Repository>,
    /// Name to templated value, handed to every command run in a workspace.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub commands: BTreeMap<String, CatalogueCommand>,
}

/// What `projects/<name>.toml` holds: the keys as written, before the defaults and the checks.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    name: Option<String>,
    folder: PathBuf,
    branch_prefix: Option<String>,
    workspaces_root: Option<PathBuf>,
    #[serde(default = "all_repositories")]
    default_repositories: RepositorySet,
    // Wide, so that a cap out of range is refused by the check, in its own words.
    #[serde(default = "default_children_cap")]
    max_children: i64,
    #[serde(default)]
    never: Vec<Vec<String>>,
    repositories: Vec<Repository>,
    #[serde(default)]
    variables: BTreeMap<String, String>,
    #[serde(default)]
    commands: BTreeMap<String, CatalogueCommand>,
}

fn all_repositories() -> RepositorySet {
    RepositorySet::All
}

fn default_children_cap() -> i64 {
    DEFAULT_CHILDREN_CAP.into()
}

impl Project {
    /// A project that takes everything by default: each repository with its remote `origin` and
    /// the base rule `[main]`, nothing else configured. `name` must be valid and `folder`
    /// absolute, `data` is Flottille's data folder.
    pub fn new(name: &str, folder: PathBuf, repositories: Vec<String>, data: &Path) -> Project {
        Project {
            name: name.to_owned(),
            folder,
            branch_prefix: slug(name),
            workspaces_root: data.join("workspaces").join(name),
            default_repositories: RepositorySet::All,
            max_children: DEFAULT_CHILDREN_CAP,
            never: Vec::new(),
            repositories: repositories
                .into_iter()
                .map(|path| Repository {
                    path,
                    remote: default_remote(),
                    base: default_base(),
                    depends_on: Vec::new(),
                })
                .collect(),
            variables: BTreeMap::new(),
            commands: BTreeMap::new(),
        }
    }

    /// The repositories, leaves first: each comes after those it depends on, and among those that
    /// are ready the first of the file comes first. For preparing a workspace and, later, merging.
    pub fn in_dependency_order(&self) -> Vec<&Repository> {
        let (mut order, left) = dependency_order(&self.repositories);
        // None are left for a project that was read from a file: a cycle is refused there.
        order.extend(left);
        order.into_iter().map(|i| &self.repositories[i]).collect()
    }

    /// Reads and checks the text of `projects/<name>.toml`. `data` is Flottille's data folder,
    /// where the default workspaces root lives. The first problem found is the one reported.
    pub fn parse(name: &str, text: &str, data: &Path) -> Result<Project, ConfigError> {
        let refuse = |message: String| ConfigError {
            name: name.to_owned(),
            message,
        };
        if !valid_name(name) {
            return Err(refuse(
                "name: must be 1 to 64 letters, digits, dots, dashes or underscores, starting \
                 with a letter or a digit"
                    .into(),
            ));
        }
        let line_of = |error: &toml::de::Error| {
            let line = error
                .span()
                .map(|span| text[..span.start].matches('\n').count() + 1);
            line.map_or_else(
                || error.message().to_owned(),
                |line| format!("line {line}: {}", error.message()),
            )
        };
        let reading = toml::Deserializer::parse(text).map_err(|e| refuse(line_of(&e)))?;
        let file: File = serde_path_to_error::deserialize(reading).map_err(|error| {
            let key = error.path().to_string();
            let cause = error.inner();
            // The path of a problem about the whole file is `.`: no key to name.
            refuse(match key.as_str() {
                "." => cause.message().to_owned(),
                key => format!("{key}: {}", cause.message()),
            })
        })?;
        check(name, file, data).map_err(refuse)
    }

    /// The text of the project's file.
    pub fn to_toml(&self) -> crate::Result<String> {
        toml::to_string_pretty(self).map_err(|error| {
            crate::Error::Refused(format!("cannot write the file of {}: {error}", self.name))
        })
    }
}

/// The checks and defaults of a project; the error is `<key>: <what is wrong>`.
fn check(name: &str, file: File, data: &Path) -> Result<Project, String> {
    if let Some(inside) = &file.name
        && inside != name
    {
        return Err(format!("name: must be \"{name}\", like the file"));
    }
    if !file.folder.is_absolute() {
        return Err(format!(
            "folder: must be absolute, not \"{}\"",
            file.folder.display()
        ));
    }
    let branch_prefix = match file.branch_prefix {
        Some(prefix) if prefix.is_empty() || prefix.contains(char::is_whitespace) => {
            return Err("branch_prefix: must not be empty nor hold spaces".into());
        }
        Some(prefix) => prefix,
        None => slug(name),
    };
    let workspaces_root = match file.workspaces_root {
        Some(root) if !root.is_absolute() => {
            return Err(format!(
                "workspaces_root: must be absolute, not \"{}\"",
                root.display()
            ));
        }
        Some(root) => root,
        None => data.join("workspaces").join(name),
    };
    let max_children = u8::try_from(file.max_children)
        .ok()
        .filter(|cap| CHILDREN_CAP.contains(cap))
        .ok_or_else(|| {
            format!(
                "max_children: must be between {} and {}, not {}",
                CHILDREN_CAP.start(),
                CHILDREN_CAP.end(),
                file.max_children
            )
        })?;
    for (index, entry) in file.never.iter().enumerate() {
        if entry.is_empty() {
            return Err(format!("never[{index}]: must not be empty"));
        }
        if entry.iter().any(|word| word.trim().is_empty()) {
            return Err(format!("never[{index}]: must not hold an empty word"));
        }
    }

    check_repositories(&file.repositories)?;
    let known = |path: &str| file.repositories.iter().any(|r| r.path == path);
    let unknown = |key: String, path: &str| format!("{key}: unknown repository \"{path}\"");
    match &file.default_repositories {
        RepositorySet::Only(paths) if paths.is_empty() => {
            return Err("default_repositories: must name at least one repository".into());
        }
        RepositorySet::Only(paths) => {
            if let Some(path) = paths.iter().find(|path| !known(path)) {
                return Err(unknown("default_repositories".into(), path));
            }
        }
        RepositorySet::All => {}
    }
    for (index, repository) in file.repositories.iter().enumerate() {
        let key = format!("repositories[{index}].depends_on");
        for dependency in &repository.depends_on {
            if *dependency == repository.path {
                return Err(format!("{key}: a repository cannot depend on itself"));
            }
            if !known(dependency) {
                return Err(unknown(key, dependency));
            }
        }
    }
    let (_, left) = dependency_order(&file.repositories);
    if !left.is_empty() {
        let (index, cycle) = cycle_among(&file.repositories, &left);
        return Err(format!(
            "repositories[{index}].depends_on: cycle {}",
            cycle.join(" -> ")
        ));
    }

    for variable in file.variables.keys() {
        let mut chars = variable.chars();
        let start = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        if !start || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!(
                "variables.{variable}: must be a name made of letters, digits and underscores, \
                 not starting with a digit"
            ));
        }
    }
    for (command, entry) in &file.commands {
        if command.is_empty() || command.contains(char::is_whitespace) {
            return Err(format!("commands.{command}: the name must not hold spaces"));
        }
        if entry.line.trim().is_empty() {
            return Err(format!("commands.{command}.line: must not be empty"));
        }
        if let Some(path) = &entry.repository
            && !known(path)
        {
            return Err(unknown(format!("commands.{command}.repository"), path));
        }
    }

    Ok(Project {
        name: name.to_owned(),
        folder: file.folder,
        branch_prefix,
        workspaces_root,
        default_repositories: file.default_repositories,
        max_children,
        never: file.never,
        repositories: file.repositories,
        variables: file.variables,
        commands: file.commands,
    })
}

/// What each repository holds by itself: a path that is its own, a remote, a base rule.
fn check_repositories(repositories: &[Repository]) -> Result<(), String> {
    if repositories.is_empty() {
        return Err("repositories: must hold at least one repository".into());
    }
    for (index, repository) in repositories.iter().enumerate() {
        let key = format!("repositories[{index}]");
        if !valid_repository_path(&repository.path) {
            return Err(format!(
                "{key}.path: must be \".\" or a path inside the project like \"app\" or \
                 \"libs/core\", not \"{}\"",
                repository.path
            ));
        }
        if repositories[..index]
            .iter()
            .any(|r| r.path == repository.path)
        {
            return Err(format!(
                "{key}.path: \"{}\" is already a repository of the project",
                repository.path
            ));
        }
        if repository.remote.trim().is_empty() || repository.remote.contains(char::is_whitespace) {
            return Err(format!("{key}.remote: must be a remote's name"));
        }
        if repository.base.is_empty() {
            return Err(format!("{key}.base: must name at least one branch"));
        }
        if repository
            .base
            .iter()
            .any(|branch| branch.is_empty() || branch.contains(char::is_whitespace))
        {
            return Err(format!("{key}.base: must name branches without spaces"));
        }
    }
    Ok(())
}

fn valid_repository_path(path: &str) -> bool {
    path == "." || path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

/// The repositories in dependency order, as indexes: one comes after those it depends on, and of
/// those that are ready the first of the file comes first. The second list holds, in file order,
/// the ones that cannot be placed because they stand on a cycle or behind one: it is empty for
/// repositories that passed the checks.
fn dependency_order(repositories: &[Repository]) -> (Vec<usize>, Vec<usize>) {
    let position = |path: &String| repositories.iter().position(|r| r.path == *path);
    let mut placed = vec![false; repositories.len()];
    let mut order = Vec::with_capacity(repositories.len());
    while let Some(next) = (0..repositories.len()).find(|&index| {
        !placed[index]
            && repositories[index]
                .depends_on
                .iter()
                .filter_map(position)
                .all(|dependency| placed[dependency])
    }) {
        placed[next] = true;
        order.push(next);
    }
    let left = (0..repositories.len()).filter(|&i| !placed[i]).collect();
    (order, left)
}

/// A cycle among the repositories `dependency_order` could not place, with the index of the
/// repository that closes it: `a -> b -> a`.
fn cycle_among<'a>(repositories: &'a [Repository], left: &[usize]) -> (usize, Vec<&'a str>) {
    let position = |path: &String| repositories.iter().position(|r| r.path == *path);
    let mut trail = vec![left[0]];
    loop {
        let at = trail[trail.len() - 1];
        // A repository is left because one of its dependencies is, so there is always a next.
        let next = repositories[at]
            .depends_on
            .iter()
            .filter_map(position)
            .find(|dependency| left.contains(dependency))
            .expect("a repository that was not placed depends on one that was not");
        if let Some(from) = trail.iter().position(|&seen| seen == next) {
            let names = trail[from..]
                .iter()
                .chain([&next])
                .map(|&index| repositories[index].path.as_str())
                .collect();
            return (next, names);
        }
        trail.push(next);
    }
}

/// What a name may be made of: it is a file name, a folder name and a word on the command line.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Lowercase words joined by dashes: `My App_2` becomes `my-app-2`.
pub fn slug(text: &str) -> String {
    let mut slug = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = "/data/flottille";

    fn parse(text: &str) -> Result<Project, ConfigError> {
        Project::parse("app", text, Path::new(DATA))
    }

    fn parse_as(name: &str, text: &str) -> Result<Project, ConfigError> {
        Project::parse(name, text, Path::new(DATA))
    }

    /// The message of the refusal of `text`, with its file.
    fn refusal(text: &str) -> String {
        parse(text).unwrap_err().to_string()
    }

    const MINIMAL: &str = r#"
        folder = "/work/app"
        [[repositories]]
        path = "."
    "#;

    #[test]
    fn a_minimal_configuration_gets_the_defaults() {
        let project = parse(MINIMAL).unwrap();

        assert_eq!(project.name, "app");
        assert_eq!(project.folder, Path::new("/work/app"));
        assert_eq!(project.branch_prefix, "app");
        assert_eq!(
            project.workspaces_root,
            Path::new("/data/flottille/workspaces/app")
        );
        assert_eq!(project.default_repositories, RepositorySet::All);
        assert_eq!(project.max_children, 3);
        assert!(project.never.is_empty());
        let repository = &project.repositories[0];
        assert_eq!(repository.remote, "origin");
        assert_eq!(repository.base, ["main"]);
        assert!(repository.depends_on.is_empty());
    }

    #[test]
    fn the_branch_prefix_defaults_to_a_slug_of_the_name() {
        let project = Project::parse("My_App.2", MINIMAL, Path::new(DATA)).unwrap();

        assert_eq!(project.branch_prefix, "my-app-2");
    }

    #[test]
    fn the_multi_repository_case_is_described_without_any_script() {
        let text = include_str!("../tests/fixtures/multi-repo-case.toml");

        let project = parse(text).unwrap();

        let paths: Vec<_> = project.repositories.iter().map(|r| &r.path[..]).collect();
        assert_eq!(
            paths,
            [
                ".",
                "app-web",
                "app-api",
                "lib-core",
                "lib-app",
                "lib-api",
                "e2e-tests"
            ]
        );
        assert_eq!(project.repositories[1].base, ["develop", "main"]);
        assert_eq!(project.repositories[1].depends_on, ["lib-app", "lib-core"]);
        assert_eq!(project.variables["COMPOSE_PROJECT_NAME"], "app-{workspace}");
        assert!(project.commands["api:dev"].serve);
        assert!(project.commands["test"].check && project.commands["test"].read_only);
        assert!(project.commands["generate-data"].ask_before_running);
        assert_eq!(project.never[0], ["git", "push"]);
    }

    #[test]
    fn a_new_project_is_one_that_reads_back_the_same() {
        let project = Project::new(
            "Shop_2",
            "/work/shop".into(),
            vec![".".into(), "web".into()],
            Path::new(DATA),
        );

        assert_eq!(project.branch_prefix, "shop-2");
        assert_eq!(project.max_children, 3);
        assert_eq!(
            parse_as("Shop_2", &project.to_toml().unwrap()).unwrap(),
            project
        );
    }

    #[test]
    fn a_workspace_can_take_a_chosen_set_of_repositories() {
        let text = r#"
            folder = "/work/app"
            default_repositories = ["a", "b"]
            [[repositories]]
            path = "a"
            [[repositories]]
            path = "b"
        "#;

        let project = parse(text).unwrap();

        assert_eq!(
            project.default_repositories,
            RepositorySet::Only(vec!["a".into(), "b".into()])
        );
    }

    #[test]
    fn a_project_written_then_read_is_the_same() {
        let text = include_str!("../tests/fixtures/multi-repo-case.toml");
        let project = parse(text).unwrap();

        let again = parse(&project.to_toml().unwrap()).unwrap();

        assert_eq!(again, project);
    }

    #[test]
    fn a_dependency_on_an_unknown_repository_is_refused_with_the_key() {
        let text = r#"
            folder = "/work/app"
            [[repositories]]
            path = "a"
            [[repositories]]
            path = "b"
            [[repositories]]
            path = "c"
            depends_on = ["x"]
        "#;

        assert_eq!(
            refusal(text),
            r#"projects/app.toml: repositories[2].depends_on: unknown repository "x""#
        );
    }

    #[test]
    fn an_empty_base_rule_is_refused_with_the_key() {
        let text = r#"
            folder = "/work/app"
            [[repositories]]
            path = "a"
            base = []
        "#;

        assert_eq!(
            refusal(text),
            "projects/app.toml: repositories[0].base: must name at least one branch"
        );
    }

    #[test]
    fn a_children_cap_of_9_is_refused_with_the_key() {
        let text = r#"
            folder = "/work/app"
            max_children = 9
            [[repositories]]
            path = "."
        "#;

        assert_eq!(
            refusal(text),
            "projects/app.toml: max_children: must be between 1 and 6, not 9"
        );
        assert!(refusal(&text.replace('9', "0")).contains("max_children"));
        assert!(parse(&text.replace('9', "6")).is_ok());
    }

    #[test]
    fn a_dependency_cycle_is_refused_with_the_key() {
        let text = r#"
            folder = "/work/app"
            [[repositories]]
            path = "a"
            depends_on = ["b"]
            [[repositories]]
            path = "b"
            depends_on = ["a"]
        "#;

        let message = refusal(text);

        assert!(
            message.starts_with("projects/app.toml: repositories[0].depends_on: cycle"),
            "{message}"
        );
    }

    #[test]
    fn repositories_come_in_dependency_order_leaves_first_ties_in_file_order() {
        let text = include_str!("../tests/fixtures/multi-repo-case.toml");
        let project = parse(text).unwrap();

        let order: Vec<_> = project
            .in_dependency_order()
            .into_iter()
            .map(|r| &r.path[..])
            .collect();

        // Each step takes the first repository of the file whose dependencies are all placed.
        assert_eq!(
            order,
            [
                ".",
                "lib-core",
                "lib-app",
                "app-web",
                "lib-api",
                "app-api",
                "e2e-tests"
            ]
        );
    }

    #[test]
    fn a_consumer_written_before_its_library_still_comes_after_it() {
        let text = r#"
            folder = "/work/app"
            [[repositories]]
            path = "consumer"
            depends_on = ["library"]
            [[repositories]]
            path = "library"
            [[repositories]]
            path = "other"
        "#;

        let order: Vec<_> = parse(text)
            .unwrap()
            .in_dependency_order()
            .into_iter()
            .map(|r| r.path.clone())
            .collect();

        assert_eq!(order, ["library", "consumer", "other"]);
    }

    #[test]
    fn a_longer_cycle_is_reported_once_from_the_repository_that_closes_it() {
        let text = r#"
            folder = "/work/app"
            [[repositories]]
            path = "x"
            depends_on = ["a"]
            [[repositories]]
            path = "a"
            depends_on = ["b"]
            [[repositories]]
            path = "b"
            depends_on = ["c"]
            [[repositories]]
            path = "c"
            depends_on = ["a"]
        "#;

        assert_eq!(
            refusal(text),
            "projects/app.toml: repositories[1].depends_on: cycle a -> b -> c -> a"
        );
    }

    #[test]
    fn a_repository_cannot_depend_on_itself() {
        let text = r#"
            folder = "/work/app"
            [[repositories]]
            path = "a"
            depends_on = ["a"]
        "#;

        assert!(refusal(text).contains("repositories[0].depends_on"));
    }

    #[test]
    fn a_project_needs_a_repository() {
        assert_eq!(
            refusal(r#"folder = "/work/app""#),
            "projects/app.toml: missing field `repositories`"
        );
        assert_eq!(
            refusal("folder = \"/work/app\"\nrepositories = []"),
            "projects/app.toml: repositories: must hold at least one repository"
        );
    }

    #[test]
    fn repository_paths_are_relative_and_unique() {
        for path in ["/abs", "../out", "./a", "a/", "a//b", ""] {
            let text = format!("folder = \"/work/app\"\n[[repositories]]\npath = {path:?}\n");
            let message = refusal(&text);
            assert!(
                message.starts_with("projects/app.toml: repositories[0].path"),
                "{path}: {message}"
            );
        }
        let twice = "folder = \"/work/app\"\n[[repositories]]\npath = \"a\"\n[[repositories]]\npath = \"a\"\n";
        assert_eq!(
            refusal(twice),
            r#"projects/app.toml: repositories[1].path: "a" is already a repository of the project"#
        );
    }

    #[test]
    fn the_folder_and_the_workspaces_root_are_absolute() {
        let relative = MINIMAL.replace("/work/app", "work/app");
        assert!(refusal(&relative).starts_with("projects/app.toml: folder: must be absolute"));
        let root = format!("workspaces_root = \"rel\"\n{MINIMAL}");
        assert!(refusal(&root).starts_with("projects/app.toml: workspaces_root: must be absolute"));
    }

    #[test]
    fn a_default_set_names_repositories_of_the_project() {
        let text = format!("default_repositories = [\"nope\"]\n{MINIMAL}");
        assert_eq!(
            refusal(&text),
            r#"projects/app.toml: default_repositories: unknown repository "nope""#
        );
        let empty = format!("default_repositories = []\n{MINIMAL}");
        assert!(refusal(&empty).starts_with("projects/app.toml: default_repositories"));
        let word = format!("default_repositories = \"some\"\n{MINIMAL}");
        assert!(refusal(&word).contains("default_repositories"));
    }

    #[test]
    fn a_command_names_its_repository_and_has_a_line() {
        let unknown =
            format!("{MINIMAL}\n[commands.build]\nline = \"make\"\nrepository = \"nope\"\n");
        assert_eq!(
            refusal(&unknown),
            r#"projects/app.toml: commands.build.repository: unknown repository "nope""#
        );
        let blank = format!("{MINIMAL}\n[commands.build]\nline = \"  \"\n");
        assert_eq!(
            refusal(&blank),
            "projects/app.toml: commands.build.line: must not be empty"
        );
        let spaced = format!("{MINIMAL}\n[commands.\"a b\"]\nline = \"make\"\n");
        assert!(refusal(&spaced).contains("commands.a b"));
    }

    #[test]
    fn a_variable_is_named_like_an_environment_variable() {
        let text = format!("{MINIMAL}\n[variables]\n\"2X\" = \"v\"\n");
        assert!(refusal(&text).starts_with("projects/app.toml: variables.2X: must be a name"));
    }

    #[test]
    fn a_never_entry_is_a_list_of_words_and_not_empty() {
        let empty = format!("never = [[\"git\", \"push\"], []]\n{MINIMAL}");
        assert_eq!(
            refusal(&empty),
            "projects/app.toml: never[1]: must not be empty"
        );
        let blank = format!("never = [[\"git\", \"push\"], [\"rm\", \" \"]]\n{MINIMAL}");
        assert_eq!(
            refusal(&blank),
            "projects/app.toml: never[1]: must not hold an empty word"
        );
        let words = format!("never = [[\"rm\", \"-rf\"]]\n{MINIMAL}");
        assert_eq!(parse(&words).unwrap().never, [["rm", "-rf"]]);
        let line = format!("never = [\"git push\"]\n{MINIMAL}");
        assert!(refusal(&line).starts_with("projects/app.toml: never[0]: "));
    }

    #[test]
    fn a_key_that_is_not_known_is_refused_with_its_name() {
        let text = format!("max_child = 2\n{MINIMAL}");
        let message = refusal(&text);
        assert!(
            message.starts_with("projects/app.toml: ") && message.contains("max_child"),
            "{message}"
        );
        let nested = MINIMAL.replace("path = \".\"", "path = \".\"\nbranch = \"x\"");
        assert!(
            refusal(&nested).contains("repositories[0]"),
            "{}",
            refusal(&nested)
        );
    }

    #[test]
    fn a_value_of_the_wrong_type_is_refused_with_its_key() {
        let text = format!("max_children = \"three\"\n{MINIMAL}");
        let message = refusal(&text);
        assert!(
            message.starts_with("projects/app.toml: max_children: "),
            "{message}"
        );
    }

    #[test]
    fn a_syntax_error_names_the_file_and_the_line() {
        let message = refusal("folder = \n");
        assert!(
            message.starts_with("projects/app.toml: line 1: "),
            "{message}"
        );
    }

    #[test]
    fn a_name_inside_the_file_must_be_the_file_name() {
        let text = format!("name = \"other\"\n{MINIMAL}");
        assert_eq!(
            refusal(&text),
            r#"projects/app.toml: name: must be "app", like the file"#
        );
        assert!(parse(&format!("name = \"app\"\n{MINIMAL}")).is_ok());
    }

    #[test]
    fn names_are_made_of_letters_digits_dots_dashes_and_underscores() {
        for good in ["app", "My_App.2", "a-b", "9"] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", ".hidden", "-x", "a b", "a/b", "é", &"x".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
        let message = Project::parse("a b", MINIMAL, Path::new(DATA))
            .unwrap_err()
            .to_string();
        assert!(
            message.starts_with("projects/a b.toml: name: "),
            "{message}"
        );
    }

    #[test]
    fn a_slug_is_lowercase_words_joined_by_dashes() {
        assert_eq!(slug("My App_2"), "my-app-2");
        assert_eq!(slug("--a..b--"), "a-b");
        assert_eq!(slug("Été"), "t");
        assert_eq!(slug("..."), "");
    }
}
