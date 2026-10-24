//! Where every tool call is decided, once its arguments are decoded and its paths resolved
//! (`docs/design.md` §7).
//!
//! The gate itself comes later (#6), with its verdicts (deny, ask, allow), its never list and its
//! requests. Until then the decision is the place's alone: a call is allowed inside the place and
//! refused outside it.

use crate::place::Resolved;

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Allow,
    Refuse(String),
}

/// The verdict on a call touching `paths`.
pub fn decide(paths: &[&Resolved]) -> Verdict {
    match paths.iter().find(|path| !path.inside) {
        Some(path) => Verdict::Refuse(format!(
            "refused: {} is outside this agent's place: its tools reach only the files of the \
             folder it works in",
            path.shown
        )),
        None => Verdict::Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(shown: &str, inside: bool) -> Resolved {
        Resolved {
            shown: shown.into(),
            path: shown.into(),
            inside,
        }
    }

    #[test]
    fn a_call_is_allowed_inside_the_place_and_refused_outside() {
        let (a, b) = (path("src/a.rs", true), path("~/x", false));
        let call = decide;

        assert_eq!(call(&[&a]), Verdict::Allow);
        assert_eq!(
            call(&[&a, &b]),
            Verdict::Refuse(
                "refused: ~/x is outside this agent's place: its tools reach only the files of \
                 the folder it works in"
                    .into()
            )
        );
    }
}
