//! The sentences a request's card says, shared by the window and the phone's core: one text, so
//! that the two never say different things of the same request.

use flottille_protocol::words::{
    BASE_SAYS, LEAVE_SAYS, always_says, forwarded, judged, sentences, source,
};

#[test]
fn the_reasons_are_sentences_of_their_own() {
    assert_eq!(
        sentences(["it deletes files", " outside the project. ", ""]),
        "It deletes files. Outside the project."
    );
    assert_eq!(sentences(Vec::<&str>::new()), "");
    assert_eq!(sentences(["émile asked"]), "Émile asked.");
}

#[test]
fn a_base_card_says_it_asks_in_ask_and_in_auto() {
    assert!(BASE_SAYS.contains("in Ask and in Auto"));
    assert_eq!(always_says("it deletes files"), BASE_SAYS);
}

#[test]
fn the_first_message_to_another_session_says_what_it_leaves() {
    let why = "this session has not written to that one before: the user lets it";
    assert_eq!(always_says(why), LEAVE_SAYS);
    assert_eq!(always_says(&sentences([why])), LEAVE_SAYS);
}

#[test]
fn who_forwarded_it_and_who_asked_are_said() {
    assert_eq!(
        forwarded("Check the tests", "it needs the key"),
        "Check the tests forwards it: it needs the key"
    );
    assert_eq!(
        source(7, "the conversation", ["Check the tests", "agent a2"]),
        "asked by the conversation · forwarded by Check the tests · forwarded by agent a2 · #7"
    );
    assert_eq!(
        judged("claude haiku", "ask", "it reads secrets"),
        "The judge (claude haiku) asks you: it reads secrets"
    );
    assert_eq!(
        judged("claude haiku", "failed", "no answer"),
        "The judge (claude haiku) could not decide: no answer"
    );
}
