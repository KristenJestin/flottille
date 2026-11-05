//! `web {action: "show_link"}`: an agent hands the user a link (`docs/tools.md`, "Showing a link").
//! The daemon opens nothing: it checks the link and says it in an event, `agent.link_shown`, and
//! the window offers the user an Open button (`docs/events.md`).

use std::path::PathBuf;
use std::sync::Arc;

use flottille_core::secrets;
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use crate::daemon::Daemon;
use crate::gate::Verdict;
use crate::mcp::{Grant, Outcome, arguments};
use crate::place::Place;

/// The action's name in `web`.
pub const ACTION: &str = "show_link";
/// A title is a short line: the toast and the card show it whole.
const TITLE_CHARS: usize = 120;
/// What a link may be long: it rides in an event.
const URL_CHARS: usize = 2_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    #[allow(dead_code)]
    action: String,
    url: Option<String>,
    path: Option<String>,
    title: Option<String>,
}

/// Whether a call of `web` is this action.
pub fn is(args: &Value) -> bool {
    args["action"] == ACTION
}

/// What is shown, checked: an address or a file of the agent's place.
enum Link {
    Url { url: Url, host: String },
    File { real: PathBuf, shown: String },
}

fn refuse(why: impl std::fmt::Display) -> String {
    format!("refused: {why}; nothing was shown")
}

/// The link and its title, or the refusal saying why and what to give instead.
fn asked(place: &Place, args: &Value) -> Result<(Link, Option<String>), String> {
    let args: Arguments = match arguments("web", args.clone()) {
        Ok(args) => args,
        Err(Outcome::Refused(why)) => return Err(why),
        Err(_) => unreachable!("arguments only refuse"),
    };
    let title = args
        .title
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty());
    if title
        .as_ref()
        .is_some_and(|t| t.chars().count() > TITLE_CHARS)
    {
        return Err(refuse(format!(
            "the title is longer than {TITLE_CHARS} characters: give a short one"
        )));
    }
    let link = match (args.url, args.path) {
        (Some(url), None) => address(&url)?,
        (None, Some(path)) => file(place, &path)?,
        _ => {
            return Err(refuse(
                "web show_link takes a url (http or https) or a path of your place, one of them",
            ));
        }
    };
    Ok((link, title))
}

fn address(text: &str) -> Result<Link, String> {
    if text.chars().count() > URL_CHARS {
        return Err(refuse(format!(
            "this URL is longer than {URL_CHARS} characters"
        )));
    }
    let url = Url::parse(text.trim())
        .map_err(|error| refuse(format!("{text:?} is not a URL ({error})")))?;
    let host = url.host_str().unwrap_or_default().to_owned();
    if !matches!(url.scheme(), "http" | "https") || host.is_empty() {
        return Err(refuse(format!(
            "show_link shows an http or https URL, and {text:?} is not one"
        )));
    }
    let env = std::env::vars_os().collect();
    if secrets::mask(url.as_str(), &env) != url.as_str() {
        return Err(refuse("this URL holds what looks like a secret"));
    }
    Ok(Link::Url { url, host })
}

fn file(place: &Place, named: &str) -> Result<Link, String> {
    let resolved = place.resolve(&place.root, named);
    if !resolved.inside || resolved.unfollowed.is_some() {
        return Err(refuse(format!(
            "{} is not inside your place ({}): show_link shows a file of your own workspace or \
             checkout, where it really leads once links and .. are followed",
            resolved.shown,
            place.root.display()
        )));
    }
    if !resolved.path.is_file() {
        return Err(refuse(format!("{} is not a file", resolved.shown)));
    }
    Ok(Link::File {
        real: resolved.path,
        shown: resolved.shown,
    })
}

/// The gate's verdict: it changes nothing, so a link that holds passes in every mode.
pub fn verdict(place: &Place, args: &Value) -> Verdict {
    match asked(place, args) {
        Ok(_) => Verdict::Allow,
        Err(why) => Verdict::Deny(why),
    }
}

/// Says the link in an event; the user opens it, or not.
pub fn show(daemon: &Arc<Daemon>, grant: &Grant, args: Value) -> Outcome {
    let (link, title) = match asked(&grant.place, &args) {
        Ok(asked) => asked,
        Err(why) => return Outcome::Refused(why),
    };
    let env = std::env::vars_os().collect();
    let title = title.map(|title| secrets::mask_also(&title, &env, &grant.place.secret_values()));
    let (named, data) = match &link {
        Link::Url { url, host } => (
            title.clone().unwrap_or_else(|| host.clone()),
            json!({"agent": grant.agent, "kind": "url", "target": url.as_str(),
                "title": title, "host": host}),
        ),
        Link::File { real, shown } => (
            title.clone().unwrap_or_else(|| shown.clone()),
            json!({"agent": grant.agent, "kind": "file", "target": real, "title": title,
                "host": null, "shown": shown}),
        ),
    };
    let said = daemon.write(|store| {
        let event = store.append_agent_event(&grant.agent, "agent.link_shown", data, false)?;
        Ok(((), event))
    });
    match said {
        Ok(()) => Outcome::Done(format!(
            "Shown to the user: {named}. They open it with a click; nothing opened by itself."
        )),
        Err(error) => Outcome::Failed(error.to_string()),
    }
}
