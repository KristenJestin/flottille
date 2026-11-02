//! The scenarios of the window's appearance as the daemon keeps it (`docs/protocol.md`, "The
//! appearance methods"): Omarchy and dark until the user chooses, a choice written to
//! `appearance.toml`, announced, and still said by a daemon started again. Temporary XDG folders.

mod common;

use common::User;
use serde_json::json;

#[test]
fn the_appearance_is_omarchy_with_the_dark_shade_until_the_user_chooses() {
    let user = User::new();
    let shown = user.call("appearance.show", json!({})).unwrap();
    assert_eq!(shown, json!({"theme": "omarchy", "shade": "dark"}));
    assert!(!user.path("config/flottille/appearance.toml").exists());
}

#[test]
fn a_chosen_appearance_is_kept_announced_and_read_back_after_a_restart() {
    let user = User::new();
    let watch = user.watch(&["--after", "0"]);

    let shown = user
        .call(
            "appearance.set",
            json!({"theme": "flottille", "shade": "light"}),
        )
        .unwrap();

    assert_eq!(shown, json!({"theme": "flottille", "shade": "light"}));
    let file = std::fs::read_to_string(user.path("config/flottille/appearance.toml")).unwrap();
    assert!(file.contains("theme = \"flottille\""), "{file}");
    assert!(file.contains("shade = \"light\""), "{file}");
    let seen = watch.until(|e| e["name"] == "appearance.changed");
    assert_eq!(
        seen.last().unwrap()["data"],
        json!({"theme": "flottille", "shade": "light"})
    );
    assert!(user.run(&["daemon", "stop"]).status.success());
    assert_eq!(
        user.call("appearance.show", json!({})).unwrap(),
        json!({"theme": "flottille", "shade": "light"})
    );
}

#[test]
fn an_appearance_that_names_no_theme_is_refused() {
    let user = User::new();
    let (code, _) = user
        .call(
            "appearance.set",
            json!({"theme": "solarized", "shade": "dark"}),
        )
        .unwrap_err();
    assert_eq!(code, "invalid-params");
    let (code, _) = user
        .call(
            "appearance.set",
            json!({"theme": "omarchy", "shade": "dim"}),
        )
        .unwrap_err();
    assert_eq!(code, "invalid-params");
}

#[test]
fn an_appearance_file_that_cannot_be_read_is_said() {
    let user = User::new();
    std::fs::create_dir_all(user.path("config/flottille")).unwrap();
    std::fs::write(
        user.path("config/flottille/appearance.toml"),
        "theme = \"neon\"\n",
    )
    .unwrap();
    let (code, message) = user.call("appearance.show", json!({})).unwrap_err();
    assert_eq!(code, "invalid-config");
    assert!(message.contains("appearance.toml"), "{message}");
}
