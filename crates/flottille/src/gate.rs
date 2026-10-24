//! Where every tool call is decided, once, before it is performed (`docs/design.md` §7).
//!
//! The gate itself comes later (#6), with its verdicts (deny, ask, allow), its never list and its
//! requests. It will replace `decide` and add its `Ask` to `Verdict`, and nothing around it
//! changes: `mcp::played` calls it once for every call of every tool, with what the call is (the
//! agent's grant, the tool, its arguments, the words of the command it would start, the paths it
//! reaches). Until then the decision is the place's alone: a call is allowed inside the place and
//! refused outside it.

use serde_json::Value;

use crate::mcp::Grant;
use crate::place::Resolved;

/// A call, as the gate sees it. Today's decision reads only its paths; the gate of #6 reads the
/// rest.
#[allow(dead_code)]
pub struct Call<'a> {
    /// The agent that makes it, its place and its tools.
    pub grant: &'a Grant,
    pub tool: &'a str,
    pub args: &'a Value,
    /// The words of the line a run would start, split without a shell; empty for any other call,
    /// and for a line that cannot be split (the tool refuses it with its own sentence).
    pub words: &'a [String],
    /// The paths the call reaches, resolved; empty for a call that names none, or names one that
    /// cannot be read (the tool refuses it).
    pub paths: &'a [Resolved],
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Allow,
    /// Refused, with the sentence that says why and what to do.
    Deny(String),
}

impl Verdict {
    /// How a recorded call names it.
    pub fn word(&self) -> &'static str {
        match self {
            Verdict::Allow => "allow",
            Verdict::Deny(_) => "deny",
        }
    }
}

/// The verdict on a call.
pub fn decide(call: &Call) -> Verdict {
    contained(call.paths)
}

/// Whether the paths are inside the place. This is the place's own invariant, not a policy: the
/// tools check it again on the paths they resolve themselves, whatever the gate decides.
pub fn contained(paths: &[Resolved]) -> Verdict {
    match paths.iter().find(|path| !path.inside) {
        Some(path) => Verdict::Deny(format!(
            "refused: {} is outside this agent's place: its tools reach only the files of the \
             folder it works in",
            path.shown
        )),
        None => Verdict::Allow,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::place::Place;

    fn path(shown: &str, inside: bool) -> Resolved {
        Resolved {
            shown: shown.into(),
            path: shown.into(),
            inside,
        }
    }

    fn grant() -> Grant {
        Grant {
            agent: "a1".into(),
            tools: vec!["fs_read", "commands_stop"],
            place: Place {
                project: "app".into(),
                workspace: None,
                root: "/work/app".into(),
                repositories: Vec::new(),
                branch: String::new(),
                variables: Default::default(),
                home: None,
            },
        }
    }

    #[test]
    fn a_call_is_allowed_inside_the_place_and_refused_outside() {
        let grant = grant();
        let args = json!({});
        let (a, b) = (path("src/a.rs", true), path("~/x", false));
        let call = |paths: &[Resolved]| {
            decide(&Call {
                grant: &grant,
                tool: "fs_read",
                args: &args,
                words: &[],
                paths,
            })
        };

        assert_eq!(call(&[]), Verdict::Allow);
        assert_eq!(call(std::slice::from_ref(&a)), Verdict::Allow);
        assert_eq!(
            call(&[a, b]),
            Verdict::Deny(
                "refused: ~/x is outside this agent's place: its tools reach only the files of \
                 the folder it works in"
                    .into()
            )
        );
    }

    #[test]
    fn a_call_that_reaches_no_path_is_allowed_too() {
        let grant = grant();
        let args = json!({"run": "r1"});
        let call = Call {
            grant: &grant,
            tool: "commands_stop",
            args: &args,
            words: &[],
            paths: &[],
        };

        assert_eq!(decide(&call), Verdict::Allow);
        assert_eq!(Verdict::Allow.word(), "allow");
        assert_eq!(Verdict::Deny("no".into()).word(), "deny");
    }
}
