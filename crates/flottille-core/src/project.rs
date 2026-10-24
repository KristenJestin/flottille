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
/// Seconds a run step of the recipe may take unless the project or the step says otherwise.
const DEFAULT_RUN_TIME_LIMIT: u64 = 30 * 60;

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

/// One step of the recipe that prepares a new workspace, as written: exactly one of `copy`,
/// `link`, `root_entries`, `run` and `command`, done in one repository, in each, or in the
/// workspace folder (`docs/projects.md`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeStep {
    /// Globs of files to copy from the main checkout, never over a file already there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<Vec<String>>,
    /// One entry of the main checkout, linked into the workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Every top-level entry of the project folder that is neither a repository nor `.git` nor
    /// `.gitignore`, linked into the workspace folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_entries: Option<bool>,
    /// One command line, its words templated, run without a shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// A command of the catalogue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Where the step is done; the workspace folder when left out (or, for a command, where the
    /// catalogue says).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Done once in each repository of the workspace, in dependency order, leaves first.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub for_each_repository: bool,
    /// Seconds a `run` or a `command` may take, in place of the project's `run_time_limit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_limit: Option<u64>,
}

/// What a recipe step does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StepKind<'a> {
    Copy(&'a [String]),
    Link(&'a str),
    RootEntries,
    Run(&'a str),
    Command(&'a str),
}

impl RecipeStep {
    /// What the step does. A step of a checked project holds exactly one kind.
    pub fn kind(&self) -> StepKind<'_> {
        self.kinds()
            .into_iter()
            .next()
            .expect("a checked recipe step holds one kind")
    }

    fn kinds(&self) -> Vec<StepKind<'_>> {
        let mut kinds = Vec::new();
        if let Some(globs) = &self.copy {
            kinds.push(StepKind::Copy(globs));
        }
        if let Some(path) = &self.link {
            kinds.push(StepKind::Link(path));
        }
        if self.root_entries.is_some() {
            kinds.push(StepKind::RootEntries);
        }
        if let Some(line) = &self.run {
            kinds.push(StepKind::Run(line));
        }
        if let Some(name) = &self.command {
            kinds.push(StepKind::Command(name));
        }
        kinds
    }
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
    /// Seconds a run step of the recipe may take before it is stopped, unless the step says.
    pub run_time_limit: u64,
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
    /// The steps that prepare a new workspace, in order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recipe: Vec<RecipeStep>,
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
    #[serde(default = "default_run_time_limit")]
    run_time_limit: u64,
    #[serde(default)]
    never: Vec<Vec<String>>,
    repositories: Vec<Repository>,
    #[serde(default)]
    variables: BTreeMap<String, String>,
    #[serde(default)]
    commands: BTreeMap<String, CatalogueCommand>,
    #[serde(default)]
    recipe: Vec<RecipeStep>,
}

fn all_repositories() -> RepositorySet {
    RepositorySet::All
}

fn default_children_cap() -> i64 {
    DEFAULT_CHILDREN_CAP.into()
}

fn default_run_time_limit() -> u64 {
    DEFAULT_RUN_TIME_LIMIT
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
            run_time_limit: DEFAULT_RUN_TIME_LIMIT,
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
            recipe: Vec::new(),
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
    if file.run_time_limit == 0 {
        return Err("run_time_limit: must be at least 1 second".into());
    }
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

    for (variable, value) in &file.variables {
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
        crate::variables::check(value).map_err(|error| format!("variables.{variable}: {error}"))?;
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

    for (index, step) in file.recipe.iter().enumerate() {
        check_step(&format!("recipe[{index}]"), step, &known, &file.commands)?;
    }

    Ok(Project {
        name: name.to_owned(),
        folder: file.folder,
        branch_prefix,
        workspaces_root,
        default_repositories: file.default_repositories,
        max_children,
        run_time_limit: file.run_time_limit,
        never: file.never,
        repositories: file.repositories,
        variables: file.variables,
        commands: file.commands,
        recipe: file.recipe,
    })
}

/// One step of the recipe, `key` being `recipe[i]`.
fn check_step(
    key: &str,
    step: &RecipeStep,
    known: &dyn Fn(&str) -> bool,
    commands: &BTreeMap<String, CatalogueCommand>,
) -> Result<(), String> {
    let kinds = step.kinds();
    if kinds.len() != 1 {
        return Err(format!(
            "{key}: must hold exactly one of copy, link, root_entries, run, command"
        ));
    }
    if let Some(path) = &step.repository {
        if step.for_each_repository {
            return Err(format!(
                "{key}.for_each_repository: cannot go with repository"
            ));
        }
        if !known(path) {
            return Err(format!("{key}.repository: unknown repository \"{path}\""));
        }
    }
    match step.time_limit {
        Some(_) if !matches!(kinds[0], StepKind::Run(_) | StepKind::Command(_)) => {
            return Err(format!("{key}.time_limit: goes only with run or command"));
        }
        Some(0) => return Err(format!("{key}.time_limit: must be at least 1 second")),
        _ => {}
    }
    let inside = |key: String, path: &str| {
        if path != "." && valid_repository_path(path) {
            Ok(())
        } else {
            Err(format!(
                "{key}: must be a path inside the repository like \".env\" or \"config/*.json\", \
                 not \"{path}\""
            ))
        }
    };
    match kinds[0] {
        StepKind::Copy(globs) => {
            if globs.is_empty() {
                return Err(format!("{key}.copy: must name at least one file"));
            }
            for (index, glob) in globs.iter().enumerate() {
                inside(format!("{key}.copy[{index}]"), glob)?;
                if glob.split('/').any(|part| part == ".git") {
                    return Err(format!(
                        "{key}.copy[{index}]: \"{glob}\" can only match in .git, which a copy \
                         never touches"
                    ));
                }
            }
        }
        StepKind::Link(path) => inside(format!("{key}.link"), path)?,
        StepKind::RootEntries => {
            if step.root_entries == Some(false) {
                return Err(format!("{key}.root_entries: must be true, or left out"));
            }
            if step.repository.is_some() || step.for_each_repository {
                return Err(format!(
                    "{key}.root_entries: links the project folder's entries, takes no repository"
                ));
            }
        }
        StepKind::Run(line) => {
            let refuse = |error: String| format!("{key}.run: {error}");
            for word in crate::command::split_line(line).map_err(refuse)? {
                crate::variables::check(&word).map_err(refuse)?;
            }
        }
        StepKind::Command(name) => {
            if !commands.contains_key(name) {
                return Err(format!(
                    "{key}.command: no command \"{name}\" in the catalogue"
                ));
            }
        }
    }
    Ok(())
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
        let kinds: Vec<_> = project.recipe.iter().map(RecipeStep::kind).collect();
        assert_eq!(
            kinds,
            [
                StepKind::Copy(&[".env".into(), ".env.*".into()]),
                StepKind::RootEntries,
                StepKind::Run("npm install"),
            ]
        );
        assert!(project.recipe[2].for_each_repository);
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

    const TWO: &str = r#"
        folder = "/work/app"
        [[repositories]]
        path = "."
        [[repositories]]
        path = "api"
        [commands.install]
        line = "npm install"
        repository = "api"
    "#;

    #[test]
    fn a_recipe_is_read_in_order_with_each_kind_of_step() {
        let text = format!(
            r#"{TWO}
            [[recipe]]
            copy = [".env", ".env.*"]
            for_each_repository = true
            [[recipe]]
            link = "notes"
            [[recipe]]
            root_entries = true
            [[recipe]]
            run = "npm run build --prefix {{workspace.path}}/{{repository}}"
            for_each_repository = true
            [[recipe]]
            command = "install"
            "#
        );

        let project = parse(&text).unwrap();

        let kinds: Vec<_> = project.recipe.iter().map(RecipeStep::kind).collect();
        assert_eq!(
            kinds,
            [
                StepKind::Copy(&[".env".into(), ".env.*".into()]),
                StepKind::Link("notes"),
                StepKind::RootEntries,
                StepKind::Run("npm run build --prefix {workspace.path}/{repository}"),
                StepKind::Command("install"),
            ]
        );
        assert!(project.recipe[0].for_each_repository);
        assert_eq!(parse(&project.to_toml().unwrap()).unwrap(), project);
    }

    #[test]
    fn a_recipe_step_holds_exactly_one_kind() {
        let none = format!("{TWO}\n[[recipe]]\nrepository = \"api\"\n");
        let two = format!("{TWO}\n[[recipe]]\nrun = \"make\"\nlink = \"x\"\n");
        for text in [none, two] {
            assert_eq!(
                refusal(&text),
                "projects/app.toml: recipe[0]: must hold exactly one of copy, link, \
                 root_entries, run, command"
            );
        }
    }

    #[test]
    fn a_recipe_step_names_a_repository_of_the_project_or_each_of_them() {
        let unknown = format!("{TWO}\n[[recipe]]\nrun = \"make\"\nrepository = \"web\"\n");
        assert_eq!(
            refusal(&unknown),
            r#"projects/app.toml: recipe[0].repository: unknown repository "web""#
        );
        let both = format!(
            "{TWO}\n[[recipe]]\nrun = \"make\"\nrepository = \"api\"\nfor_each_repository = true\n"
        );
        assert_eq!(
            refusal(&both),
            "projects/app.toml: recipe[0].for_each_repository: cannot go with repository"
        );
        let root = format!("{TWO}\n[[recipe]]\nroot_entries = true\nrepository = \"api\"\n");
        assert_eq!(
            refusal(&root),
            "projects/app.toml: recipe[0].root_entries: links the project folder's entries, \
             takes no repository"
        );
        let off = format!("{TWO}\n[[recipe]]\nroot_entries = false\n");
        assert_eq!(
            refusal(&off),
            "projects/app.toml: recipe[0].root_entries: must be true, or left out"
        );
    }

    #[test]
    fn run_steps_are_stopped_after_30_minutes_unless_the_file_says_otherwise() {
        assert_eq!(parse(MINIMAL).unwrap().run_time_limit, 1800);
        let text =
            format!("run_time_limit = 600\n{TWO}\n[[recipe]]\nrun = \"make\"\ntime_limit = 7200\n");
        let project = parse(&text).unwrap();
        assert_eq!(project.run_time_limit, 600);
        assert_eq!(project.recipe[0].time_limit, Some(7200));
        assert_eq!(parse(&project.to_toml().unwrap()).unwrap(), project);
    }

    #[test]
    fn a_time_limit_is_a_number_of_seconds_above_zero_on_a_run_or_a_command() {
        assert_eq!(
            refusal(&format!("run_time_limit = 0\n{TWO}")),
            "projects/app.toml: run_time_limit: must be at least 1 second"
        );
        let zero = format!("{TWO}\n[[recipe]]\nrun = \"make\"\ntime_limit = 0\n");
        assert_eq!(
            refusal(&zero),
            "projects/app.toml: recipe[0].time_limit: must be at least 1 second"
        );
        let copy = format!("{TWO}\n[[recipe]]\ncopy = [\".env\"]\ntime_limit = 60\n");
        assert_eq!(
            refusal(&copy),
            "projects/app.toml: recipe[0].time_limit: goes only with run or command"
        );
    }

    #[test]
    fn a_run_step_with_shell_syntax_is_refused_with_its_key() {
        let text = format!("{TWO}\n[[recipe]]\nrun = \"npm install && npm run build\"\n");
        assert_eq!(
            refusal(&text),
            format!(
                "projects/app.toml: recipe[0].run: {}",
                crate::command::refusal("&&")
            )
        );
    }

    #[test]
    fn a_template_name_flottille_does_not_fill_is_refused_with_its_key() {
        let run = format!("{TWO}\n[[recipe]]\nrun = \"make {{home}}\"\n");
        assert!(
            refusal(&run).starts_with(
                "projects/app.toml: recipe[0].run: “{home}” is not a name Flottille fills"
            ),
            "{}",
            refusal(&run)
        );
        let variable = format!("{TWO}\n[variables]\nSTORE = \"{{workspace.root}}/s\"\n");
        assert!(
            refusal(&variable).starts_with(
                "projects/app.toml: variables.STORE: “{workspace.root}” is not a name"
            ),
            "{}",
            refusal(&variable)
        );
    }

    #[test]
    fn a_copy_or_a_link_stays_inside_its_repository() {
        for (step, key) in [
            ("copy = [\"../secret\"]", "recipe[0].copy[0]"),
            ("copy = [\".env\", \"/etc/passwd\"]", "recipe[0].copy[1]"),
            ("copy = []", "recipe[0].copy"),
            ("link = \".\"", "recipe[0].link"),
        ] {
            let text = format!("{TWO}\n[[recipe]]\n{step}\n");
            assert!(
                refusal(&text).starts_with(&format!("projects/app.toml: {key}: ")),
                "{step}: {}",
                refusal(&text)
            );
        }
    }

    #[test]
    fn a_copy_glob_that_can_only_match_git_is_refused() {
        for glob in [".git", ".git/config", "sub/.git/*"] {
            let text = format!("{TWO}\n[[recipe]]\ncopy = [\"{glob}\"]\n");
            assert_eq!(
                refusal(&text),
                format!(
                    "projects/app.toml: recipe[0].copy[0]: \"{glob}\" can only match in .git, \
                     which a copy never touches"
                )
            );
        }
        let text = format!("{TWO}\n[[recipe]]\ncopy = [\".gi*\"]\n");
        assert!(parse(&text).is_ok());
    }

    #[test]
    fn a_command_step_names_a_command_of_the_catalogue() {
        let text = format!("{TWO}\n[[recipe]]\ncommand = \"deploy\"\n");
        assert_eq!(
            refusal(&text),
            r#"projects/app.toml: recipe[0].command: no command "deploy" in the catalogue"#
        );
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
