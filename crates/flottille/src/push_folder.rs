//! The git folder a push the gate let through runs from (`docs/vault.md`, "The runtime folder"):
//! one the daemon makes for it, outside every run's reach, so that nothing of the repository's own
//! settings, which a run can write, steers where the push goes or how it reaches it (a proxy, a
//! certificate not checked, a header, a URL rewritten). Git has no switch that leaves a
//! repository's settings unread, and a setting given on the command's line does not win over a
//! more specific one of the repository's (`http.<url>.sslVerify` over `http.sslVerify`): so the
//! push reads none of them. Its settings are, in git's order:
//!
//! - the system's (root's);
//! - the user's own, as they apply to the repository (an `includeIf` for its folder or branch),
//!   read from the files found at the daemon's start (`pushed::user_settings`);
//! - of the repository's, only what says which refs go to which remote: [`KEPT`];
//! - the daemon's own (`runs::lifted`).
//!
//! The repository's objects are its own, reached from the folder made; its refs are those it had
//! when the folder was made, written there, so that a run moving a branch between the checks of
//! the push and the push sends nothing they did not read. The remote-tracking refs the push moved
//! and an upstream it set (`-u`) are written back to the repository once it ended.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::git::Class;

/// The environment of a command.
type Env = std::collections::BTreeMap<OsString, OsString>;

/// The repository's settings a push still reads: which remote, at which URLs, a branch's
/// upstream and the remote-tracking refs. Each is a name of a remote, a URL or a refspec, never a
/// program nor a way to reach the remote (`http.*`, `url.*`, `remote.<name>.proxy`,
/// `remote.<name>.vcs` are left out), nor which refs a push that names none sends ([`REFUSED`]).
/// Names as `git config --list` writes them: the section and the key lower case, the middle as
/// it is.
const KEPT: [&str; 8] = [
    "remote.*.url",
    "remote.*.pushurl",
    "remote.*.fetch",
    "branch.*.remote",
    "branch.*.pushremote",
    "branch.*.merge",
    "remote.pushdefault",
    "push.autosetupremote",
];

/// The repository's settings that say which refs a push sends when its line names none, and how:
/// one a run wrote would make a plain `git push` delete a branch of the remote (`:main`), force
/// it (`+refs/heads/*`), prune it (`mirror`) or send to another branch than its own
/// (`push.default=upstream`). A push from a repository that sets one is refused rather than run
/// without it, which would send other refs than the user meant.
const REFUSED: [&str; 3] = ["remote.*.push", "remote.*.mirror", "push.default"];

/// What the folder made for a push holds.
pub struct PushFolder {
    /// The folder, removed once the push ended.
    folder: tempfile::TempDir,
    /// The repository's object folder.
    objects: PathBuf,
    /// The settings given to the push, in the order git reads them: the user's, then the
    /// repository's that are [`KEPT`], from `kept_from` on.
    settings: Vec<(String, String)>,
    kept_from: usize,
    /// The credential helpers the user set for one forge alone (`git::forge_helpers`): the
    /// pattern of the remotes' URLs that are that forge's, and the file of this folder that holds
    /// them.
    forge_helpers: Vec<(String, String)>,
    /// Those of `forge_helpers` the push reads ([`PushFolder::destined`]).
    included: Vec<String>,
    /// The one folder of the push's `PATH` (`git::rooted_folder`), made for it.
    path: PathBuf,
    /// Where the push was run from: the repository an upstream it sets is written back to.
    from: PathBuf,
    /// The repository's refs when the folder was made, each with its object: what the push
    /// reads, and what a remote-tracking ref it moved is moved from in the repository.
    refs: Vec<(String, String)>,
}

impl PushFolder {
    /// The variables of the push's environment that make it run from this folder, with no
    /// settings but the system's and [`PushFolder::settings`].
    pub fn env(&self) -> Vec<(OsString, OsString)> {
        vec![
            ("GIT_DIR".into(), self.folder.path().into()),
            ("GIT_OBJECT_DIRECTORY".into(), self.objects.clone().into()),
            ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
        ]
    }

    /// The settings to give the push before the daemon's own.
    pub fn settings(&self) -> &[(String, String)] {
        &self.settings
    }

    /// The `PATH` of the push.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The settings to give the push after the daemon's emptied its credential helpers: the
    /// user's helpers for one forge, included where every URL the push may go to is that
    /// forge's ([`PushFolder::destined`]).
    pub fn forge_helpers(&self) -> Vec<(String, String)> {
        self.included
            .iter()
            .map(|file| ("include.path".to_owned(), file.clone()))
            .collect()
    }

    /// Chooses the user's helpers for one forge the push reads, its environment `env` (its own,
    /// with none of them) and its `words`: those whose pattern every URL it may go to matches,
    /// as the daemon judges it. Git's own `hasconfig:remote.*.url:` would include a forge's
    /// helpers where any remote's URL matches, a remote or a `pushurl` a run wrote among them,
    /// and ask them for a sign-in to wherever the push goes. None where what the push may go to
    /// cannot be read.
    pub async fn destined(&mut self, env: &Env, words: &[String]) {
        self.included.clear();
        let Some(urls) = self.destinations(env, words).await else {
            return;
        };
        if urls.is_empty() {
            return;
        }
        self.included = self
            .forge_helpers
            .iter()
            .filter(|(pattern, _)| urls.iter().all(|url| wildmatch(pattern, url)))
            .map(|(_, file)| file.clone())
            .collect();
    }

    /// Every URL the push of `words` may go to, as git reads its settings with `env`: the push
    /// URLs of every remote it knows (`git remote -v`), which settles where a push that names none goes, whatever
    /// chooses it. None where a remote cannot be read, where a setting that chooses one
    /// (`branch.<name>.remote`, `branch.<name>.pushRemote`, `remote.pushDefault`) or the line
    /// names one git does not know (a URL), or where the line says `--repo`.
    async fn destinations(&self, env: &Env, words: &[String]) -> Option<Vec<String>> {
        let git = crate::git::program()?.to_string_lossy().into_owned();
        let read = async |args: &[&str]| {
            let words: Vec<String> = std::iter::once(git.as_str())
                .chain(args.iter().copied())
                .map(str::to_owned)
                .collect();
            crate::git::run_outside(&self.from, &words, env, None)
                .await
                .ok()
        };
        let listed = read(&["remote"]).await.filter(|read| read.success)?;
        let remotes: Vec<&str> = listed.stdout.lines().collect();
        // `git remote get-url` knows no remote the settings of the environment alone name, which
        // are all a push's; `-v` does, each push URL rewritten as the push rewrites it.
        let shown = read(&["remote", "-v"]).await.filter(|read| read.success)?;
        let urls: Vec<String> = shown
            .stdout
            .lines()
            .filter_map(|line| line.strip_suffix(" (push)")?.split_once('\t'))
            .map(|(_, url)| url.to_owned())
            .collect();
        let known = |name: &str| name == "." || remotes.contains(&name);
        // None of them set is no failure: git says nothing, and exits with 1.
        let chosen = read(&[
            "config",
            "--null",
            "--get-regexp",
            r"^(branch\..*\.(push)?remote|remote\.pushdefault)$",
        ])
        .await
        .filter(|read| read.success || read.stderr.trim().is_empty())?;
        if !crate::git::entries(&chosen.stdout)
            .iter()
            .all(|(_, name)| known(name))
        {
            return None;
        }
        let at_push = crate::gate::git_globals(&words[1..]).0 + 1;
        let rest = &words[at_push + 1..];
        if rest.iter().any(|word| word.starts_with("--repo")) {
            return None;
        }
        match crate::message_checks::pushed_to(rest) {
            Ok(Some(named)) if !remotes.contains(&named.as_str()) => None,
            Ok(_) => Some(urls),
            Err(_) => None,
        }
    }

    /// Drops the repository's `branch.<name>.merge` that names another branch than its own,
    /// where the push, read with `env` (its own), sends a branch to its upstream
    /// (`push.default=upstream`, the user's choice): one a run wrote would send a plain `git push`
    /// to another branch. Whether it dropped one, so that the push's settings are made again; why
    /// the push is refused where `push.default` cannot be read.
    pub async fn unsteered(&mut self, env: &Env) -> Result<bool, String> {
        let unread = |why: &str| {
            format!(
                "what push.default says cannot be read ({why}), and a branch's upstream a run \
                 wrote may send the push to another branch"
            )
        };
        let git = crate::git::program().ok_or_else(|| unread(crate::git::UNFOUND))?;
        let git = git.to_string_lossy().into_owned();
        let words = [git.as_str(), "config", "--get", "push.default"].map(str::to_owned);
        let read = crate::git::run_outside(&self.from, &words, env, None)
            .await
            .map_err(|why| unread(&why))?;
        // Unset is no failure: git says nothing, and exits with 1.
        if !read.success && !read.stderr.trim().is_empty() {
            return Err(unread(read.stderr.trim()));
        }
        let default = read.stdout.trim().to_ascii_lowercase();
        if !["upstream", "tracking"].contains(&default.as_str()) {
            return Ok(false);
        }
        let before = self.settings.len();
        let mut at = 0;
        let kept_from = self.kept_from;
        self.settings.retain(|(key, value)| {
            at += 1;
            at <= kept_from || !steering(key, value)
        });
        Ok(self.settings.len() != before)
    }

    /// Writes back to the repository what the push changed in the folder: the remote-tracking
    /// refs it moved, and the upstreams it set (`-u`, `push.autoSetupRemote`), which git wrote in
    /// the folder's own settings, which held no `branch.*`.
    pub async fn written_back(self) {
        self.tracking_back().await;
        // Read here and given to git on its input: the runtime folder is out of a confined
        // command's reach.
        let Ok(file) = std::fs::read(self.folder.path().join("config")) else {
            return;
        };
        let git = crate::git::own().to_string_lossy().into_owned();
        let words = [
            git.as_str(),
            "config",
            "--file",
            "-",
            "--null",
            "--get-regexp",
        ]
        .map(str::to_owned)
        .into_iter()
        .chain([r"^branch\.".to_owned()])
        .collect::<Vec<_>>();
        let listed = crate::git::run_in(&self.from, &words, &Default::default(), Some(file)).await;
        let Ok(listed) = listed else { return };
        for (key, value) in listed_settings(&listed.stdout) {
            let set = crate::git::run(
                &self.from,
                &["config", "--replace-all", &key, &value],
                Class::Read,
            )
            .await;
            if let Err(why) = set.and_then(|set| match set.success {
                true => Ok(()),
                false => Err(set.stderr),
            }) {
                eprintln!("flottille daemon: the upstream a push set is not kept: {why}");
            }
        }
    }

    /// Moves in the repository each remote-tracking ref the push moved in the folder, from the
    /// object it had when the folder was made: one a run moved since is left as it is.
    async fn tracking_back(&self) {
        let now = folder_refs(self.folder.path());
        let before = |name: &str| {
            self.refs
                .iter()
                .find(|(_, had)| had == name)
                .map(|(sha, _)| sha.as_str())
        };
        let mut commands = Vec::new();
        for (sha, name) in &now {
            if name.starts_with("refs/remotes/") && before(name) != Some(sha.as_str()) {
                commands.push(match before(name) {
                    Some(old) => format!("update {name} {sha} {old}\n"),
                    None => format!("create {name} {sha}\n"),
                });
            }
        }
        for (old, name) in &self.refs {
            if name.starts_with("refs/remotes/") && !now.iter().any(|(_, kept)| kept == name) {
                commands.push(format!("delete {name} {old}\n"));
            }
        }
        let git = crate::git::own().to_string_lossy().into_owned();
        let words = [git, "update-ref".to_owned(), "--stdin".to_owned()];
        // One at a time: a ref a run moved since is not moved, and the others still are.
        for command in commands {
            let moved = crate::git::run_in(
                &self.from,
                &words,
                &Default::default(),
                Some(command.clone().into_bytes()),
            )
            .await;
            if let Err(why) = moved.and_then(|moved| match moved.success {
                true => Ok(()),
                false => Err(moved.stderr),
            }) {
                eprintln!(
                    "flottille daemon: a remote-tracking ref a push moved is not moved ({}): {why}",
                    command.trim()
                );
            }
        }
    }
}

/// The refs of the folder a push ran from, each with its object: those of its `packed-refs`, and
/// the loose ones the push wrote over them.
fn folder_refs(folder: &Path) -> Vec<(String, String)> {
    let mut refs: Vec<(String, String)> = std::fs::read_to_string(folder.join("packed-refs"))
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.starts_with('#') && !line.starts_with('^'))
        .filter_map(|line| line.split_once(' '))
        .map(|(sha, name)| (sha.to_owned(), name.to_owned()))
        .collect();
    let mut folders = vec![folder.join("refs")];
    while let Some(at) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => folders.push(path),
                Ok(kind) if kind.is_file() => {
                    let Ok(name) = path.strip_prefix(folder) else {
                        continue;
                    };
                    let name = name.to_string_lossy().into_owned();
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    let sha = text.trim();
                    if name.ends_with(".lock") || sha.starts_with("ref:") || sha.is_empty() {
                        continue;
                    }
                    refs.retain(|(_, had)| *had != name);
                    refs.push((sha.to_owned(), name));
                }
                _ => {}
            }
        }
    }
    refs
}

/// Whether `url` matches `pattern` as git matches a `hasconfig:remote.*.url:` pattern (its
/// `wildmatch` with `WM_PATHNAME`): `*` and `?` never cross a `/`, `**` between two of them, or
/// at an end, does. A class (`[…]`) or an escape (`\`) matches nothing: a forge's helpers are
/// given where the daemon is sure.
fn wildmatch(pattern: &str, url: &str) -> bool {
    fn from(pattern: &[u8], text: &[u8], after_slash: bool) -> bool {
        if let [b'*', b'*', rest @ ..] = pattern
            && after_slash
            && matches!(rest.first(), None | Some(b'/'))
        {
            let Some(rest) = rest.get(1..) else {
                return true;
            };
            // `**/`: none or more whole folders.
            return (0..=text.len())
                .filter(|&at| at == 0 || text[at - 1] == b'/')
                .any(|at| from(rest, &text[at..], true));
        }
        match pattern.first() {
            None => text.is_empty(),
            Some(b'*') => {
                let rest = &pattern[pattern.iter().take_while(|&&c| c == b'*').count()..];
                (0..=text.len())
                    .take_while(|&at| at == 0 || text[at - 1] != b'/')
                    .any(|at| from(rest, &text[at..], at == 0 && after_slash))
            }
            Some(b'?') => text
                .first()
                .is_some_and(|&c| c != b'/' && from(&pattern[1..], &text[1..], false)),
            Some(b'[' | b'\\') => false,
            Some(&c) => text
                .first()
                .is_some_and(|&t| t == c && from(&pattern[1..], &text[1..], c == b'/')),
        }
    }
    from(pattern.as_bytes(), url.as_bytes(), true)
}

/// Whether the repository's `key` is a `branch.<name>.merge` naming another branch than
/// `<name>`.
fn steering(key: &str, value: &str) -> bool {
    matches("branch.*.merge", key)
        && key
            .strip_prefix("branch.")
            .and_then(|rest| rest.strip_suffix(".merge"))
            .is_some_and(|name| value != format!("refs/heads/{name}"))
}

/// Makes the folder a push from the repository at `from` runs from, in `under` (the daemon's
/// runtime folder, which no run reaches), with the user's own settings `user` as they apply
/// there. Why it cannot be made, when it cannot: the push is then refused.
pub async fn made(
    under: &Path,
    from: &Path,
    user: Vec<(String, String)>,
) -> Result<PushFolder, String> {
    let read = |args: &'static [&'static str]| async move {
        crate::git::output(from, args, Class::Read).await
    };
    let common = read(&["rev-parse", "--path-format=absolute", "--git-common-dir"]).await?;
    let common = PathBuf::from(common.trim());
    let head =
        match crate::git::run(from, &["symbolic-ref", "--quiet", "HEAD"], Class::Read).await? {
            branch if branch.success => format!("ref: {}\n", branch.stdout.trim()),
            _ => format!(
                "{}\n",
                read(&["rev-parse", "--verify", "HEAD"]).await?.trim()
            ),
        };
    let listed = read(&["config", "--list", "--show-scope", "--includes", "--null"]).await?;
    // A symbolic one (`refs/remotes/origin/HEAD`) as the ref it leads to: the push moves none.
    let refs: Vec<(String, String)> = read(&["for-each-ref", "--format=%(objectname) %(refname)"])
        .await?
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(sha, name)| (sha.to_owned(), name.to_owned()))
        .collect();
    let (repository, kept) = repository_settings(&listed)?;
    let mut settings = user;
    let kept_from = settings.len();
    settings.extend(kept);
    std::fs::create_dir_all(under).map_err(|e| format!("cannot make {}: {e}", under.display()))?;
    let folder = tempfile::Builder::new()
        .prefix("push-")
        .tempdir_in(under)
        .map_err(|e| format!("cannot make the push's folder in {}: {e}", under.display()))?;
    let write = |name: &str, text: &str| {
        std::fs::write(folder.path().join(name), text)
            .map_err(|e| format!("cannot write the push's {name}: {e}"))
    };
    write("HEAD", &head)?;
    write("config", &repository)?;
    for name in ["objects", "refs"] {
        std::fs::create_dir(folder.path().join(name))
            .map_err(|e| format!("cannot make the push's folder: {e}"))?;
    }
    // Its refs are the repository's as they are now, which the checks read and the push sends.
    let packed: String = refs
        .iter()
        .map(|(sha, name)| format!("{sha} {name}\n"))
        .collect();
    write("packed-refs", &packed)?;
    // Where its history stops.
    if let Ok(shallow) = std::fs::read(common.join("shallow")) {
        std::fs::write(folder.path().join("shallow"), shallow)
            .map_err(|e| format!("cannot write the push's shallow: {e}"))?;
    }
    // Each in a file of its own, where only the daemon reaches: a setting given to a command
    // includes no file but by its path.
    let mut forge_helpers = Vec::new();
    for (index, (condition, helpers)) in crate::git::forge_helpers().iter().enumerate() {
        // `includeif.hasconfig:remote.*.url:<pattern>.path`, the only condition of the kind git
        // has.
        let Some(pattern) = condition
            .strip_prefix("includeif.hasconfig:remote.*.url:")
            .and_then(|rest| rest.strip_suffix(".path"))
        else {
            continue;
        };
        let name = format!("forge-helpers-{index}");
        write(&name, &crate::git::helpers_file(helpers))?;
        let path = folder.path().join(name).to_string_lossy().into_owned();
        forge_helpers.push((pattern.to_owned(), path));
    }
    let path = crate::git::rooted_folder(&folder.path().join("path"));
    Ok(PushFolder {
        folder,
        objects: common.join("objects"),
        settings,
        kept_from,
        forge_helpers,
        included: Vec::new(),
        path,
        from: from.to_owned(),
        refs,
    })
}

/// Of the repository's settings as `git config --list --show-scope --null` writes them: the file
/// of the push's folder (the format of its repository, which git must read from a file), and the
/// settings it keeps ([`KEPT`]), in their order. Refused when the format is not one written
/// plainly, or its refs are not kept in files.
fn repository_settings(listed: &str) -> Result<(String, Vec<(String, String)>), String> {
    let mut parts = listed.split('\0');
    let mut entries = Vec::new();
    while let (Some(scope), Some(entry)) = (parts.next(), parts.next()) {
        if scope == "local" || scope == "worktree" {
            let (key, value) = entry.split_once('\n').unwrap_or((entry, "true"));
            entries.push((key.to_owned(), value.to_owned()));
        }
    }
    let plain = |value: &str| {
        !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    };
    let mut file = String::from("[core]\n\tbare = true\n");
    let mut extensions = String::new();
    let mut kept = Vec::new();
    for (key, value) in entries {
        if key == "core.repositoryformatversion" {
            if !value.chars().all(|c| c.is_ascii_digit()) || value.is_empty() {
                return Err(format!(
                    "its repository's format ({value}) is not one git writes"
                ));
            }
            file.push_str(&format!("\trepositoryformatversion = {value}\n"));
        } else if let Some(name) = key.strip_prefix("extensions.") {
            if !plain(name) || !plain(&value) {
                return Err(format!(
                    "its repository's extension {name} is not one git writes"
                ));
            }
            if name == "refstorage" && value != "files" {
                return Err(format!(
                    "its repository keeps its refs in a {value}, and a push the gate let \
                     through runs only from a repository that keeps them in files"
                ));
            }
            // The settings of each worktree are given apart: the push's folder has none.
            if name != "worktreeconfig" {
                extensions.push_str(&format!("\t{name} = {value}\n"));
            }
        } else if let Some(pattern) = REFUSED.iter().find(|pattern| matches(pattern, &key)) {
            // A mirror turned off is no mirror.
            let off = ["false", "no", "off", "0"].contains(&value.to_ascii_lowercase().as_str());
            if !(pattern.ends_with(".mirror") && off) {
                return Err(format!(
                    "its repository's settings say which refs a push sends ({key}), which a run \
                     may have written, and a push the gate let through follows only the refs \
                     its line names and your own settings: unset it (git config --unset-all \
                     {key}) or set it in your own git settings, and name the refs on the line \
                     (git push origin HEAD:refs/heads/<branch>)"
                ));
            }
        } else if KEPT.iter().any(|pattern| matches(pattern, &key)) {
            kept.push((key, value));
        }
    }
    if !extensions.is_empty() {
        file.push_str("[extensions]\n");
        file.push_str(&extensions);
    }
    Ok((file, kept))
}

/// Whether `key` is `pattern`, a `*` standing for the middle part of a key (a remote's or a
/// branch's name, which may hold dots).
fn matches(pattern: &str, key: &str) -> bool {
    match pattern.split_once(".*.") {
        Some((section, name)) => key
            .strip_prefix(section)
            .and_then(|rest| rest.strip_prefix('.'))
            .and_then(|rest| rest.rsplit_once('.'))
            .is_some_and(|(middle, last)| !middle.is_empty() && last == name),
        None => key == pattern,
    }
}

/// The settings of `git config --null --list` (or `--get-regexp`): each key with its value, a
/// key with none (a boolean said by its name alone) as `true`.
pub fn listed_settings(listed: &str) -> Vec<(String, String)> {
    listed
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (key, value) = entry.split_once('\n').unwrap_or((entry, "true"));
            (key.to_owned(), value.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forge_s_pattern_is_matched_as_git_matches_it() {
        for (pattern, url, matched) in [
            (
                "http://127.0.0.1:*/**",
                "http://127.0.0.1:8080/app.git",
                true,
            ),
            (
                "http://127.0.0.1:*/**",
                "http://localhost:8080/app.git",
                false,
            ),
            (
                "https://forge.invalid/acme/**",
                "https://forge.invalid/acme/a/b.git",
                true,
            ),
            (
                "https://forge.invalid/acme/**",
                "https://forge.invalid/other/b.git",
                false,
            ),
            (
                "https://forge.invalid/*",
                "https://forge.invalid/acme/b.git",
                false,
            ),
            (
                "https://*.invalid/acme",
                "https://evil.example/x.invalid/acme",
                false,
            ),
            (
                "https://forge.invalid/**/app.git",
                "https://forge.invalid/app.git",
                true,
            ),
            (
                "https://forge.invalid/**/app.git",
                "https://forge.invalid/a/b/app.git",
                true,
            ),
            (
                "https://forge.invalid/a**",
                "https://forge.invalid/a/b",
                false,
            ),
            (
                "https://forge.invalid/?cme",
                "https://forge.invalid/acme",
                true,
            ),
            (
                "https://forge.invalid/[a]cme",
                "https://forge.invalid/acme",
                false,
            ),
            ("**", "https://anything.example/x", true),
            (
                "https://forge.invalid/acme",
                "https://forge.invalid/acme.evil.example",
                false,
            ),
        ] {
            assert_eq!(wildmatch(pattern, url), matched, "{pattern} {url}");
        }
    }

    #[tokio::test]
    async fn a_push_default_that_cannot_be_read_refuses_the_push() {
        let folder = tempfile::tempdir().unwrap();
        let mut made = PushFolder {
            objects: folder.path().join("objects"),
            folder,
            settings: vec![("branch.main.merge".into(), "refs/heads/elsewhere".into())],
            kept_from: 0,
            forge_helpers: Vec::new(),
            included: Vec::new(),
            path: PathBuf::from(crate::git::NO_PATH),
            // git cannot run there: what `push.default` is cannot be read.
            from: PathBuf::from("/nonexistent/flottille-no-repository"),
            refs: Vec::new(),
        };

        let unsteered = made.unsteered(&Default::default()).await;

        assert!(unsteered.is_err(), "{unsteered:?}");
    }

    #[test]
    fn only_what_says_which_refs_go_where_is_kept_of_the_repository_s_settings() {
        let listed = [
            "local\0core.repositoryformatversion\n1",
            "local\0core.bare\nfalse",
            "local\0extensions.objectformat\nsha256",
            "local\0remote.origin.url\nhttps://forge.invalid/acme/app.git",
            "local\0remote.origin.fetch\n+refs/heads/*:refs/remotes/origin/*",
            "local\0remote.origin.proxy\nhttp://127.0.0.1:9",
            "local\0remote.origin.vcs\nplanted",
            "local\0branch.feat/a.b.remote\norigin",
            "local\0branch.feat/a.b.merge\nrefs/heads/feat/a.b",
            "local\0http.proxy\nhttp://127.0.0.1:9",
            "local\0http.https://forge.invalid/.sslverify\nfalse",
            "local\0url.https://elsewhere.invalid/.insteadof\nhttps://forge.invalid/",
            "local\0core.sshcommand\nplanted",
            "local\0include.path\n/tmp/planted",
            "local\0remote.origin.mirror\nfalse",
            "global\0push.default\ncurrent",
            "worktree\0push.autosetupremote\ntrue",
            "",
        ]
        .join("\0");

        let (file, kept) = repository_settings(&listed).unwrap();

        assert_eq!(
            file,
            "[core]\n\tbare = true\n\trepositoryformatversion = 1\n[extensions]\n\
             \tobjectformat = sha256\n"
        );
        let kept: Vec<&str> = kept.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            kept,
            [
                "remote.origin.url",
                "remote.origin.fetch",
                "branch.feat/a.b.remote",
                "branch.feat/a.b.merge",
                "push.autosetupremote",
            ]
        );
    }

    #[test]
    fn a_repository_that_says_which_refs_a_push_sends_is_refused() {
        for listed in [
            "local\0remote.origin.push\n:refs/heads/main",
            "local\0remote.origin.push\n+refs/heads/*:refs/heads/*",
            "local\0remote.origin.mirror\ntrue",
            "local\0remote.origin.mirror",
            "worktree\0push.default\nupstream",
        ] {
            assert!(repository_settings(listed).is_err(), "{listed}");
        }
    }

    #[test]
    fn the_refs_a_push_moved_in_its_folder_are_read_over_those_it_had() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(
            folder.path().join("packed-refs"),
            "# pack-refs with: sorted\n1111 refs/heads/main\n2222 refs/remotes/origin/main\n\
             3333 refs/tags/v1\n^4444\n",
        )
        .unwrap();
        let origin = folder.path().join("refs/remotes/origin");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::write(origin.join("main"), "5555\n").unwrap();
        std::fs::write(origin.join("new"), "6666\n").unwrap();
        std::fs::write(origin.join("new.lock"), "7777\n").unwrap();

        let mut refs = folder_refs(folder.path());
        refs.sort_by(|a, b| a.1.cmp(&b.1));

        let refs: Vec<(&str, &str)> = refs.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        assert_eq!(
            refs,
            [
                ("1111", "refs/heads/main"),
                ("5555", "refs/remotes/origin/main"),
                ("6666", "refs/remotes/origin/new"),
                ("3333", "refs/tags/v1"),
            ]
        );
    }

    #[test]
    fn a_repository_format_not_written_plainly_is_refused() {
        for listed in [
            "local\0core.repositoryformatversion\n1\n[x]",
            "local\0extensions.objectformat\nsha256\"\n",
            "local\0extensions.refstorage\nreftable",
        ] {
            assert!(repository_settings(listed).is_err(), "{listed}");
        }
    }
}
