//! What a push the gate let through reads, and so runs, outside the sandbox (#1138,
//! `docs/vault.md`, "The runtime folder"): `~/.ssh` whole and the user's own git settings are
//! read-only to every run, each folder on the way to them kept where it is, and a configuration
//! that reaches outside them lends the user's SSH agent to no push, `flottille doctor` saying why.
//! The agent is the fake ACP agent working in Full in a throwaway repository, with temporary XDG
//! folders and home; the account's home (what `ssh` reads `~` as) is the test's too; the run is a
//! small Python program that says what each write it tries came to.

use crate::common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::mcp::{Answer, Mcp, server_of};
use common::{User, user_namespaces};
use serde_json::{Value, json};

/// A project `app` over a throwaway repository, its workspace `w`, and the session of a root
/// agent working there in Auto, its judge allowing every call: a run in Full is the user's own,
/// unconfined (`docs/tools.md`). Its session, the workspace's folder.
fn session(user: &User) -> (Mcp, PathBuf) {
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();
    let made = user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while user
        .call("workspace.show", json!({"project": "app", "name": "w"}))
        .unwrap()["state"]
        != "ready"
    {
        assert!(Instant::now() < deadline, "the workspace never got ready");
        std::thread::sleep(Duration::from_millis(50));
    }
    let place = PathBuf::from(made["folder"].as_str().unwrap());
    let opened = user.received_requests("session/new").len();
    let answer = user
        .call(
            "agent.run",
            json!({"project": "app", "workspace": "w", "message": "start the app"}),
        )
        .unwrap();
    let id = answer["agent"]["id"].as_str().unwrap();
    let (url, authorization) = server_of(user, opened);
    let output = user.run(&["agent", "mode", id, "auto"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (Mcp::initialize(&url, &authorization), place)
}

fn done(answer: Answer) -> String {
    assert!(!answer.error, "an error: {}", answer.text);
    assert!(
        !answer.text.starts_with("Waiting for"),
        "held: {}",
        answer.text
    );
    answer.text
}

/// What the program `body` fills the dictionary `out` with, run by the agent from the workspace.
/// `attempt(name, action)` runs `action` and says `done` or the error's name (`EROFS`, `EBUSY`…).
fn ran(mcp: &Mcp, place: &Path, body: &str) -> Value {
    std::fs::write(
        place.join("probe.py"),
        format!(
            "import errno, json, os, time\nout = {{}}\n\
             def attempt(name, action):\n\
             \x20   try:\n\
             \x20       action()\n\
             \x20       out[name] = 'done'\n\
             \x20   except OSError as e:\n\
             \x20       out[name] = errno.errorcode.get(e.errno, str(e.errno))\n\
             def write(path):\n\
             \x20   return lambda: open(path, 'a').write('ProxyCommand sh -c planted\\n')\n\
             {body}\nprint(json.dumps({{'out': out}}))\n"
        ),
    )
    .unwrap();
    let ran = done(mcp.call("run", json!({"line": "python3 probe.py", "timeout": 60})));
    let line = ran
        .lines()
        .find(|line| line.trim_start().starts_with("{\"out\""))
        .unwrap_or_else(|| panic!("{ran}"));
    serde_json::from_str::<Value>(line.trim()).unwrap()["out"].clone()
}

/// A Python string literal of `path`.
fn py(path: &Path) -> String {
    format!("{:?}", path.to_str().unwrap())
}

fn skipped_without_user_namespaces() -> bool {
    if !user_namespaces() {
        eprintln!("skipped: this kernel refuses user namespaces to this user");
        return true;
    }
    false
}

/// The user's `~/.ssh/config`, written as they would, with the folder.
fn ssh_config(user: &User, text: &str) {
    std::fs::create_dir_all(user.path("home/.ssh")).unwrap();
    std::fs::write(user.path("home/.ssh/config"), text).unwrap();
}

/// Whether the daemon lends the user's SSH agent to a push, as `flottille doctor --json` says it,
/// and its warnings.
fn lent(user: &User) -> (bool, String) {
    let doctor = user.run(&["doctor", "--json"]);
    let report: Value = serde_json::from_slice(&doctor.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&doctor.stdout)));
    assert!(report["sandbox"]["ssh_lent"].is_boolean(), "{report}");
    (
        report["sandbox"]["ssh_lent"] == true,
        report["warnings"].to_string(),
    )
}

/// The user's daemon, started.
fn started(user: &User) {
    user.call("project.list", Value::Null).unwrap();
}

#[test]
fn a_run_cannot_write_ssh() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    ssh_config(&user, "Host forge.invalid\n  User git\n");
    let (mcp, place) = session(&user);
    let ssh = user.path("home/.ssh");

    let out = ran(
        &mcp,
        &place,
        &format!(
            "ssh = {ssh}\n\
             attempt('config', write(ssh + '/config'))\n\
             attempt('new', write(ssh + '/new'))\n\
             attempt('removed', lambda: os.unlink(ssh + '/config'))\n\
             attempt('renamed', lambda: os.rename(ssh, ssh + '-old'))\n\
             attempt('folder', lambda: os.rmdir(ssh))\n\
             out['read'] = open(ssh + '/config').read()\n",
            ssh = py(&ssh)
        ),
    );

    for name in ["config", "new", "removed", "renamed", "folder"] {
        assert_ne!(out[name], "done", "{name}: {out}");
    }
    assert_eq!(out["read"], "Host forge.invalid\n  User git\n", "{out}");
    assert_eq!(
        std::fs::read_to_string(ssh.join("config")).unwrap(),
        "Host forge.invalid\n  User git\n"
    );
}

#[test]
fn a_run_cannot_add_a_file_to_an_included_folder() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    ssh_config(&user, "Include config.d/*\n");
    std::fs::create_dir(user.path("home/.ssh/config.d")).unwrap();
    std::fs::write(
        user.path("home/.ssh/config.d/forge"),
        "Host forge.invalid\n",
    )
    .unwrap();
    let (mcp, place) = session(&user);
    let included = user.path("home/.ssh/config.d");

    let out = ran(
        &mcp,
        &place,
        &format!(
            "d = {d}\n\
             attempt('added', write(d + '/planted'))\n\
             attempt('folder', lambda: os.mkdir(d + '/planted.d'))\n\
             attempt('renamed', lambda: os.rename(d, d + '-old'))\n",
            d = py(&included)
        ),
    );

    for name in ["added", "folder", "renamed"] {
        assert_ne!(out[name], "done", "{name}: {out}");
    }
    assert!(!included.join("planted").exists());
    assert!(lent(&user).0, "{}", lent(&user).1);
}

#[test]
fn a_run_cannot_rename_a_folder_on_the_way_to_the_git_settings() {
    if skipped_without_user_namespaces() {
        return;
    }
    let settings = "dots/git/config";
    let user = User::with_fake_agent().with_allowing_judge();
    std::fs::create_dir_all(user.path("home/dots/git")).unwrap();
    std::fs::write(
        user.path(&format!("home/{settings}")),
        "[user]\n\tname = Test\n",
    )
    .unwrap();
    let settings = user.path(&format!("home/{settings}"));
    let user = user.with_env("GIT_CONFIG_GLOBAL", settings.to_str().unwrap());
    let (mcp, place) = session(&user);
    let home = user.path("home");

    let out = ran(
        &mcp,
        &place,
        &format!(
            "home = {home}\n\
             attempt('dots', lambda: os.rename(home + '/dots', home + '/dots-old'))\n\
             attempt('git', lambda: os.rename(home + '/dots/git', home + '/dots/git-old'))\n\
             attempt('home', lambda: os.rename(home, home + '-old'))\n\
             attempt('settings', write(home + '/dots/git/config'))\n\
             attempt('beside', write(home + '/dots/git/other'))\n",
            home = py(&home)
        ),
    );

    for name in ["dots", "git", "home", "settings"] {
        assert_ne!(out[name], "done", "{name}: {out}");
    }
    // Only the file is read-only: what else is in its folder stays the run's to write.
    assert_eq!(out["beside"], "done", "{out}");
    assert!(home.join(settings).exists());
}

#[test]
fn a_run_cannot_write_the_user_s_git_settings_nor_what_they_include() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    std::fs::write(
        user.path("home/.gitconfig"),
        "[includeIf \"gitdir:~/work/\"]\n\tpath = ~/.gitconfig-work\n",
    )
    .unwrap();
    std::fs::write(user.path("home/.gitconfig-work"), "[user]\n\tname = Work\n").unwrap();
    let settings = user.path("home/.gitconfig");
    let user = user.with_env("GIT_CONFIG_GLOBAL", settings.to_str().unwrap());
    let (mcp, place) = session(&user);
    let home = user.path("home");

    let out = ran(
        &mcp,
        &place,
        &format!(
            "home = {home}\n\
             attempt('settings', write(home + '/.gitconfig'))\n\
             attempt('included', write(home + '/.gitconfig-work'))\n\
             attempt('removed', lambda: os.unlink(home + '/.gitconfig-work'))\n",
            home = py(&home)
        ),
    );

    for name in ["settings", "included", "removed"] {
        assert_ne!(out[name], "done", "{name}: {out}");
    }
    assert!(lent(&user).0, "{}", lent(&user).1);
}

#[test]
fn the_ssh_of_the_account_is_read_only_when_home_is_another_folder() {
    if skipped_without_user_namespaces() {
        return;
    }
    // `ssh` reads `~` as the account's home, whatever `HOME` says.
    let user = User::with_fake_agent().with_allowing_judge();
    std::fs::create_dir_all(user.path("account/.ssh")).unwrap();
    std::fs::write(user.path("account/.ssh/config"), "Host forge.invalid\n").unwrap();
    let account = user.path("account");
    let user = user.with_env("FLOTTILLE_TEST_ACCOUNT_HOME", account.to_str().unwrap());
    let (mcp, place) = session(&user);

    let out = ran(
        &mcp,
        &place,
        &format!(
            "attempt('config', write({config}))\n",
            config = py(&account.join(".ssh/config"))
        ),
    );

    assert_ne!(out["config"], "done", "{out}");
}

#[test]
fn a_tilde_in_an_include_is_the_account_s_home() {
    if skipped_without_user_namespaces() {
        return;
    }
    // `~/.ssh/more` is in the `.ssh` ssh reads, the account's: lent.
    let user = User::with_fake_agent();
    std::fs::create_dir_all(user.path("account/.ssh")).unwrap();
    std::fs::write(user.path("account/.ssh/config"), "Include ~/.ssh/more\n").unwrap();
    std::fs::write(user.path("account/.ssh/more"), "Host forge.invalid\n").unwrap();
    let account = user.path("account");
    let user = user.with_env("FLOTTILLE_TEST_ACCOUNT_HOME", account.to_str().unwrap());
    started(&user);
    assert!(lent(&user).0, "{}", lent(&user).1);

    // `~/notes` is the account's home itself, which a run writes: lent to no push.
    let other = User::with_fake_agent();
    std::fs::create_dir_all(other.path("account/.ssh")).unwrap();
    std::fs::write(other.path("account/.ssh/config"), "Include ~/notes\n").unwrap();
    let account = other.path("account");
    let other = other.with_env("FLOTTILLE_TEST_ACCOUNT_HOME", account.to_str().unwrap());
    started(&other);
    let (lent, warnings) = lent(&other);
    assert!(!lent, "{warnings}");
    assert!(warnings.contains("notes"), "{warnings}");
}

#[test]
fn an_include_outside_ssh_lends_no_push() {
    let user = User::with_fake_agent();
    ssh_config(
        &user,
        "Host forge.invalid\n  Include ~/dots/ssh/forge.conf\n",
    );
    std::fs::create_dir_all(user.path("home/dots/ssh")).unwrap();
    std::fs::write(user.path("home/dots/ssh/forge.conf"), "User git\n").unwrap();
    started(&user);

    let (lent, warnings) = lent(&user);

    assert!(!lent, "{warnings}");
    assert!(warnings.contains("dots/ssh/forge.conf"), "{warnings}");
    // How to fix it, said.
    assert!(warnings.contains("into ~/.ssh"), "{warnings}");
}

#[test]
fn a_link_in_a_wildcard_part_lends_no_push() {
    let user = User::with_fake_agent();
    ssh_config(&user, "Include conf.*/*\n");
    std::fs::create_dir_all(user.path("home/dots")).unwrap();
    std::fs::write(user.path("home/dots/forge"), "Host forge.invalid\n").unwrap();
    std::os::unix::fs::symlink(user.path("home/dots"), user.path("home/.ssh/conf.d")).unwrap();
    started(&user);

    let (lent, warnings) = lent(&user);

    assert!(!lent, "{warnings}");
    assert!(warnings.contains("conf.d"), "{warnings}");
}

#[test]
fn a_link_out_of_ssh_lends_no_push() {
    let user = User::with_fake_agent();
    std::fs::create_dir_all(user.path("home/dots")).unwrap();
    std::fs::write(user.path("home/dots/ssh_config"), "Host forge.invalid\n").unwrap();
    std::fs::create_dir_all(user.path("home/.ssh")).unwrap();
    std::os::unix::fs::symlink(
        user.path("home/dots/ssh_config"),
        user.path("home/.ssh/config"),
    )
    .unwrap();
    started(&user);

    let (lent, warnings) = lent(&user);

    assert!(!lent, "{warnings}");
    assert!(warnings.contains(".ssh/config"), "{warnings}");
}

#[test]
fn a_hard_link_in_ssh_lends_no_push() {
    let user = User::with_fake_agent();
    std::fs::create_dir_all(user.path("home/dots")).unwrap();
    std::fs::write(user.path("home/dots/ssh_config"), "Host forge.invalid\n").unwrap();
    std::fs::create_dir_all(user.path("home/.ssh")).unwrap();
    std::fs::hard_link(
        user.path("home/dots/ssh_config"),
        user.path("home/.ssh/config"),
    )
    .unwrap();
    started(&user);

    let (lent, warnings) = lent(&user);

    assert!(!lent, "{warnings}");
    assert!(warnings.contains("hard link"), "{warnings}");
}

#[test]
fn a_command_in_ssh_s_configuration_lends_no_push() {
    for line in [
        "Host forge.invalid\n  ProxyCommand ~/bin/proxy %h %p\n",
        "Match exec \"test -e ~/flag\"\n  User git\n",
        "Host *\n  KnownHostsCommand=~/bin/hosts\n",
        // Quoted, as ssh reads a keyword too.
        "Host forge.invalid\n  \"ProxyCommand\" ~/bin/proxy %h %p\n",
    ] {
        let user = User::with_fake_agent();
        ssh_config(&user, line);
        started(&user);

        let (lent, warnings) = lent(&user);

        assert!(!lent, "{line}: {warnings}");
        assert!(warnings.contains("runs a program"), "{line}: {warnings}");
    }
}

#[test]
fn a_missing_include_of_the_git_settings_is_judged_when_it_appears() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent();
    std::fs::write(
        user.path("home/.gitconfig"),
        "[includeIf \"gitdir:~/work/\"]\n\tpath = ~/.gitconfig-work\n",
    )
    .unwrap();
    let settings = user.path("home/.gitconfig");
    let user = user.with_env("GIT_CONFIG_GLOBAL", settings.to_str().unwrap());
    started(&user);

    // Git reads a missing include as nothing: so does Flottille.
    let (lent_at_start, warnings) = lent(&user);
    assert!(lent_at_start, "{warnings}");

    // Made since the start, by the user or a run: not kept read-only, so it lends nothing.
    std::fs::write(user.path("home/.gitconfig-work"), "[user]\n\tname = x\n").unwrap();
    let (lent, warnings) = lent(&user);
    assert!(!lent, "{warnings}");
    assert!(warnings.contains(".gitconfig-work"), "{warnings}");
}

#[test]
fn a_link_ssh_never_reads_lends_the_push() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent();
    ssh_config(&user, "Host forge.invalid\n  IdentityFile ~/.ssh/work\n");
    // The agent's socket linked where a terminal multiplexer finds it, and a file of `~/.ssh`
    // linked to another in it.
    std::fs::create_dir_all(user.path("agent")).unwrap();
    let _listening = std::os::unix::net::UnixListener::bind(user.path("agent/agent.1")).unwrap();
    std::os::unix::fs::symlink(
        user.path("agent/agent.1"),
        user.path("home/.ssh/ssh_auth_sock"),
    )
    .unwrap();
    std::fs::write(user.path("home/.ssh/work-key"), "").unwrap();
    std::os::unix::fs::symlink("work-key", user.path("home/.ssh/work")).unwrap();
    started(&user);

    let (lent, warnings) = lent(&user);

    assert!(lent, "{warnings}");
}

#[test]
fn a_link_ssh_reads_out_of_ssh_lends_no_push() {
    for (config, link) in [
        ("Host forge.invalid\n", "id_ed25519"),
        ("Host forge.invalid\n  IdentityFile ~/.ssh/work\n", "work"),
        ("Host *\n  UserKnownHostsFile ~/.ssh/hosts\n", "hosts"),
    ] {
        let user = User::with_fake_agent();
        ssh_config(&user, config);
        std::fs::create_dir_all(user.path("home/dots")).unwrap();
        std::fs::write(user.path("home/dots/file"), "").unwrap();
        std::os::unix::fs::symlink(
            user.path("home/dots/file"),
            user.path("home/.ssh").join(link),
        )
        .unwrap();
        started(&user);

        let (lent, warnings) = lent(&user);

        assert!(!lent, "{link}: {warnings}");
        assert!(warnings.contains(link), "{link}: {warnings}");
    }
}

#[test]
fn a_change_after_the_start_is_judged_at_the_push() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent();
    ssh_config(&user, "Host forge.invalid\n");
    started(&user);
    assert!(lent(&user).0, "{}", lent(&user).1);

    // The user includes a file outside `~/.ssh` while the daemon runs.
    std::fs::create_dir_all(user.path("home/dots")).unwrap();
    std::fs::write(user.path("home/dots/forge.conf"), "User git\n").unwrap();
    ssh_config(&user, "Host forge.invalid\n  Include ~/dots/forge.conf\n");

    let (lent, warnings) = lent(&user);
    assert!(!lent, "{warnings}");
}

#[test]
fn a_run_started_before_a_change_cannot_write_ssh() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    ssh_config(&user, "Host forge.invalid\n");
    let (mcp, place) = session(&user);
    let ssh = user.path("home/.ssh");
    let flag = user.path("changed");
    // The user changes the configuration while the run goes on: an included folder made since.
    let changing = {
        let (ssh, flag) = (ssh.clone(), flag.clone());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(2));
            std::fs::create_dir(ssh.join("config.d")).unwrap();
            std::fs::write(ssh.join("config"), "Include config.d/*\n").unwrap();
            std::fs::write(&flag, "").unwrap();
        })
    };

    let out = ran(
        &mcp,
        &place,
        &format!(
            "ssh, flag = {ssh}, {flag}\n\
             deadline = time.time() + 30\n\
             while not os.path.exists(flag) and time.time() < deadline:\n\
             \x20   time.sleep(0.05)\n\
             out['changed'] = os.path.exists(flag)\n\
             attempt('added', write(ssh + '/config.d/planted'))\n\
             attempt('config', write(ssh + '/config'))\n",
            ssh = py(&ssh),
            flag = py(&flag)
        ),
    );
    changing.join().unwrap();

    assert_eq!(out["changed"], true, "{out}");
    assert_ne!(out["added"], "done", "{out}");
    assert_ne!(out["config"], "done", "{out}");
}

#[test]
fn the_daemon_makes_a_missing_ssh_folder_at_its_start() {
    if skipped_without_user_namespaces() {
        return;
    }
    let user = User::with_fake_agent();
    assert!(!user.path("home/.ssh").exists());

    started(&user);

    let made = std::fs::symlink_metadata(user.path("home/.ssh")).unwrap();
    assert!(made.is_dir());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(made.permissions().mode() & 0o777, 0o700);
    assert!(lent(&user).0, "{}", lent(&user).1);
}

#[test]
fn the_agent_is_lent_to_no_push_where_user_namespaces_are_refused() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_NO_USER_NAMESPACES", "1");
    started(&user);

    let doctor = user
        .command(&["doctor", "--json"])
        .env_remove("FLOTTILLE_TEST_NO_USER_NAMESPACES")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();

    assert_eq!(report["sandbox"]["ssh_lent"], false, "{report}");
    assert!(
        report["warnings"].to_string().contains("lent to no push"),
        "{report}"
    );
}

#[test]
fn the_agent_is_lent_to_no_push_where_landlock_is_absent() {
    let user = User::with_fake_agent().with_env("FLOTTILLE_TEST_NO_LANDLOCK", "1");
    started(&user);

    let doctor = user
        .command(&["doctor", "--json"])
        .env_remove("FLOTTILLE_TEST_NO_LANDLOCK")
        .output()
        .unwrap();
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();

    assert_eq!(report["sandbox"]["ssh_lent"], false, "{report}");
    assert!(
        report["warnings"].to_string().contains("Landlock"),
        "{report}"
    );
}

/// Whether the real `ssh` client is on the `PATH`: the tests of #1302 run it, never a stand-in,
/// since a stand-in reads no configuration as OpenSSH does and so misses what it refuses.
fn ssh_client() -> bool {
    let found = std::process::Command::new("ssh")
        .arg("-V")
        .output()
        .is_ok_and(|output| output.status.success());
    if !found {
        eprintln!(
            "skipped: no ssh client on the PATH, and only the real one reads its configuration"
        );
    }
    found
}

/// The session of a root agent in Full, its home's `~/.ssh` empty: what `ssh` reads there adds
/// nothing to the machine's own `/etc/ssh/ssh_config` and what it includes.
fn session_with_empty_ssh(user: &User) -> (Mcp, PathBuf) {
    std::fs::create_dir_all(user.path("home/.ssh")).unwrap();
    session(user)
}

/// The user whose daemon is told the machine's configuration is a file of the test's that
/// includes the machine's own, root's, with that file: `ssh` told to read it (`-F`) includes a
/// root-owned file whatever the machine's configuration includes (#1334). None, saying so, on a
/// machine with no `/etc/ssh/ssh_config`.
fn with_root_owned_include(user: User) -> Option<(User, PathBuf)> {
    if !std::fs::metadata("/etc/ssh/ssh_config").is_ok_and(|m| m.is_file()) {
        eprintln!("skipped: no /etc/ssh/ssh_config on this machine");
        return None;
    }
    std::fs::create_dir_all(user.path("system")).unwrap();
    let system = user.path("system/ssh_config");
    std::fs::write(&system, "Include /etc/ssh/ssh_config\n").unwrap();
    let user = user.with_env("FLOTTILLE_TEST_SSH_SYSTEM_CONFIG", system.to_str().unwrap());
    Some((user, system))
}

#[test]
fn a_run_s_ssh_reads_the_root_owned_system_config() {
    if skipped_without_user_namespaces() || !ssh_client() {
        return;
    }
    // The machine's configuration is root's, and so are the fragments it includes: in the run's
    // namespace they were owned by no one ssh knows, and it refused them (#1302). What ssh says
    // is not looked at, only whether it read its configuration.
    let Some((user, system)) =
        with_root_owned_include(User::with_fake_agent().with_allowing_judge())
    else {
        return;
    };
    let (mcp, place) = session_with_empty_ssh(&user);

    let out = ran(
        &mcp,
        &place,
        &format!(
            "import subprocess\n\
             out['ssh'] = subprocess.run(['ssh', '-G', '-F', {system}, 'example.invalid'],\n\
             \x20   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode\n",
            system = py(&system),
        ),
    );

    assert_eq!(out["ssh"], 0, "{out}");
}

#[test]
fn a_run_fetches_from_an_ssh_remote_with_a_root_owned_system_config() {
    if skipped_without_user_namespaces() || !ssh_client() {
        return;
    }
    // The real git and ssh, to a host that does not resolve: they get as far as the network,
    // past the configuration that stopped them before (#1302).
    let Some((user, system)) =
        with_root_owned_include(User::with_fake_agent().with_allowing_judge())
    else {
        return;
    };
    let (mcp, place) = session_with_empty_ssh(&user);

    let out = ran(
        &mcp,
        &place,
        &format!(
            "import shlex, subprocess\n\
             env = dict(os.environ, GIT_SSH_COMMAND='ssh -F ' + shlex.quote({system}))\n\
             for name, line in [('ls-remote', ['git', 'ls-remote', 'ssh://nohost.invalid/x']),\n\
             \x20   ('fetch', ['git', 'fetch', 'ssh://nohost.invalid/x'])]:\n\
             \x20   said = subprocess.run(line, capture_output=True, text=True, env=env).stderr\n\
             \x20   out[name] = 'refused' if 'Bad owner or permissions' in said else \\\n\
             \x20       'unresolved' if 'nohost.invalid' in said and 'resolve' in said \\\n\
             \x20       else said[-500:]\n",
            system = py(&system),
        ),
    );

    assert_eq!(out["ls-remote"], "unresolved", "{out}");
    assert_eq!(out["fetch"], "unresolved", "{out}");
}

#[test]
fn a_run_still_cannot_read_an_ssh_setting_it_can_write() {
    if skipped_without_user_namespaces() || !ssh_client() {
        return;
    }
    // A root-owned file ssh includes is the run's own in its namespace, as its owner: never one
    // it may change, by a write or its mode, nor from a namespace of its own made inside, where it
    // holds every capability, by a remount or an unmount of the copy (Landlock refuses a
    // sandboxed process every mount, and the copy's file system is read-only besides). The
    // propagation of `/` is left as it is, which Landlock would refuse to change before any of
    // these was tried. The daemon is told the machine's configuration is a file of the test's
    // that includes the machine's own, so that one root-owned file is always given.
    let root_owned = PathBuf::from("/etc/ssh/ssh_config");
    if !std::fs::metadata(&root_owned).is_ok_and(|m| m.is_file()) {
        eprintln!("skipped: no /etc/ssh/ssh_config on this machine");
        return;
    }
    let user = User::with_fake_agent().with_allowing_judge();
    std::fs::create_dir_all(user.path("system")).unwrap();
    let system = user.path("system/ssh_config");
    std::fs::write(&system, "Include /etc/ssh/ssh_config\n").unwrap();
    let before = std::fs::read(&root_owned).unwrap();
    let user = user.with_env("FLOTTILLE_TEST_SSH_SYSTEM_CONFIG", system.to_str().unwrap());
    let (mcp, place) = session_with_empty_ssh(&user);
    // In a user and mount namespace of the run's own: what each attempt came to, as JSON.
    std::fs::write(
        place.join("nested.py"),
        "import ctypes, errno, json, os, sys\n\
         libc = ctypes.CDLL(None, use_errno=True)\n\
         target = sys.argv[1].encode()\n\
         out = {'root': os.getuid() == 0}\n\
         def call(name, result):\n\
         \x20   out[name] = 'done' if result == 0 else errno.errorcode.get(ctypes.get_errno())\n\
         MS_RDONLY, MS_REMOUNT, MS_BIND = 1, 32, 4096\n\
         call('remount_bind_rw', libc.mount(None, target, None, MS_REMOUNT | MS_BIND, None))\n\
         call('remount_rw', libc.mount(None, target, None, MS_REMOUNT, None))\n\
         call('unmounted', libc.umount2(target, 2))\n\
         try:\n\
         \x20   open(sys.argv[1], 'a').write('ProxyCommand sh -c planted\\n')\n\
         \x20   out['write'] = 'done'\n\
         except OSError as e:\n\
         \x20   out['write'] = errno.errorcode.get(e.errno)\n\
         print(json.dumps(out))\n",
    )
    .unwrap();

    let out = ran(
        &mcp,
        &place,
        &format!(
            "import subprocess\n\
             target = {root_owned}\n\
             attempt('write', write(target))\n\
             attempt('mode', lambda: os.chmod(target, 0o666))\n\
             out['owner'] = os.stat(target).st_uid == os.getuid()\n\
             nested = subprocess.run(['unshare', '-Urm', '--propagation', 'unchanged',\n\
             \x20   'python3', 'nested.py', target],\n\
             \x20   capture_output=True, text=True)\n\
             out['nested'] = json.loads(nested.stdout) if nested.returncode == 0 \\\n\
             \x20   else nested.stderr[-500:]\n\
             out['ssh'] = subprocess.run(['ssh', '-G', '-F', {system}, 'example.invalid'],\n\
             \x20   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode\n\
             out['planted'] = 'planted' in open(target).read()\n",
            root_owned = py(&root_owned),
            system = py(&system),
        ),
    );

    // Refused for the copy's file system being read-only, not for the file being root's.
    assert_eq!(out["write"], "EROFS", "{out}");
    assert_eq!(out["mode"], "EROFS", "{out}");
    assert_eq!(out["owner"], true, "{out}");
    let nested = &out["nested"];
    assert_eq!(nested["root"], true, "{out}");
    for name in ["remount_bind_rw", "remount_rw", "unmounted", "write"] {
        assert_ne!(nested[name], "done", "{name}: {out}");
    }
    assert_eq!(out["ssh"], 0, "{out}");
    assert_eq!(out["planted"], false, "{out}");
    assert_eq!(std::fs::read(&root_owned).unwrap(), before);
}

#[test]
fn a_fifo_included_by_the_user_s_ssh_config_blocks_no_run() {
    if skipped_without_user_namespaces() {
        return;
    }
    // Read at every confined start: a FIFO, which a read waits on for a writer, is not read.
    let user = User::with_fake_agent().with_allowing_judge();
    ssh_config(&user, "Include fifo\n");
    let made = std::process::Command::new("mkfifo")
        .arg(user.path("home/.ssh/fifo"))
        .status()
        .unwrap();
    assert!(made.success());
    let (mcp, place) = session(&user);

    let out = ran(&mcp, &place, "out['ran'] = True\n");

    assert_eq!(out["ran"], true, "{out}");
}

#[test]
fn the_daemon_s_fetch_of_an_ssh_origin_reaches_the_network() {
    if skipped_without_user_namespaces() || !ssh_client() {
        return;
    }
    // The daemon's own fetch of a workspace's base runs confined as a run does, with the real git
    // and ssh: an origin on a host that does not resolve fails on the host, past the machine's
    // root-owned configuration that stopped it before (#1302).
    let Some((user, _)) = with_root_owned_include(User::with_fake_agent()) else {
        return;
    };
    std::fs::create_dir_all(user.path("home/.ssh")).unwrap();
    let folder = user.path("work/app");
    user.git_init(&folder);
    std::fs::write(folder.join("README.md"), "app\n").unwrap();
    user.git(&folder, &["add", "README.md"]);
    user.git(&folder, &["commit", "--quiet", "-m", "start"]);
    user.git(
        &folder,
        &["remote", "add", "origin", "ssh://nohost.invalid/x"],
    );
    user.call("project.add", json!({"folder": folder, "name": "app"}))
        .unwrap();

    let created = user
        .call("workspace.create", json!({"project": "app", "name": "w"}))
        .unwrap();

    let said = created.to_string();
    assert!(!said.contains("Bad owner or permissions"), "{said}");
    assert!(
        said.contains("could not be fetched") && said.contains("nohost.invalid"),
        "{said}"
    );
}

#[test]
fn doctor_says_the_ssh_settings_could_not_be_copied_until_they_are() {
    if skipped_without_user_namespaces() {
        return;
    }
    // While the copies fail, `flottille doctor` says so; a command that gets them clears it.
    let Some((user, _)) = with_root_owned_include(User::with_fake_agent().with_allowing_judge())
    else {
        return;
    };
    std::fs::write(user.path("system/failing"), "").unwrap();
    let failing = user.path("system/failing");
    let user = user.with_env("FLOTTILLE_TEST_SSH_COPY_FAILS", failing.to_str().unwrap());
    let (mcp, place) = session_with_empty_ssh(&user);
    ran(&mcp, &place, "out['ran'] = True\n");
    let (_, failed) = lent(&user);

    std::fs::remove_file(&failing).unwrap();
    ran(&mcp, &place, "out['ran'] = True\n");
    let (_, given) = lent(&user);

    assert!(failed.contains("could not be copied"), "{failed}");
    assert!(!given.contains("could not be copied"), "{given}");
}

#[test]
fn doctor_says_the_root_owned_files_past_those_copied() {
    if skipped_without_user_namespaces() {
        return;
    }
    // More root-owned files included than are copied: a run's ssh refuses the rest, and the
    // user is told. The machine's own licence files, root's, play them.
    let many = std::fs::read_dir("/usr/share/doc")
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().join("copyright").is_file())
                .count()
        })
        .unwrap_or(0);
    if many <= 64 {
        eprintln!("skipped: fewer than 65 files /usr/share/doc/*/copyright on this machine");
        return;
    }
    let user = User::with_fake_agent();
    std::fs::create_dir_all(user.path("system")).unwrap();
    let system = user.path("system/ssh_config");
    std::fs::write(&system, "Include /usr/share/doc/*/copyright\n").unwrap();
    let user = user.with_env("FLOTTILLE_TEST_SSH_SYSTEM_CONFIG", system.to_str().unwrap());
    started(&user);

    let (_, warnings) = lent(&user);

    assert!(warnings.contains("64 files"), "{warnings}");
}
