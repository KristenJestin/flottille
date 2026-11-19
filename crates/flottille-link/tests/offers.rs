//! What a runner offers its peers (`docs/remote.md`, "Roles"): its projects with their free
//! slots and the agents signed in, and which runner takes a session.

use flottille_link::{Capabilities, MachineMessage, Offered};
use serde_json::json;

fn server() -> Capabilities {
    Capabilities {
        projects: vec![
            Offered {
                name: "app".to_owned(),
                slots: 2,
            },
            Offered {
                name: "site".to_owned(),
                slots: 0,
            },
        ],
        agents: vec!["claude".to_owned()],
    }
}

#[test]
fn a_runner_with_the_project_and_the_agent_takes_a_session() {
    let server = server();
    assert_eq!(server.refusal("server", "app", Some("claude")), None);
    // No agent named: any agent signed in.
    assert_eq!(server.refusal("server", "app", None), None);
    assert_eq!(
        server.refusal("server", "shop", Some("claude")).unwrap(),
        "the project shop is not on server"
    );
    assert_eq!(
        server.refusal("server", "app", Some("codex")).unwrap(),
        "codex is not signed in on server"
    );
    let none = Capabilities {
        agents: Vec::new(),
        ..server.clone()
    };
    assert_eq!(
        none.refusal("server", "app", None).unwrap(),
        "no agent is signed in on server"
    );
}

#[test]
fn a_session_for_the_user_is_taken_with_no_slot_free() {
    // The slots bound the children launched from another machine (#816): a session for the
    // user counts in no children cap.
    let server = server();
    assert_eq!(server.refusal("server", "site", Some("claude")), None);
    assert_eq!(server.refusal("server", "site", None), None);
}

#[test]
fn capabilities_travel_as_a_machine_message() {
    let message = MachineMessage::Capabilities(server());
    let text = serde_json::to_value(&message).unwrap();
    assert_eq!(
        text,
        json!({"capabilities": {
            "projects": [{"name": "app", "slots": 2}, {"name": "site", "slots": 0}],
            "agents": ["claude"],
        }})
    );
    assert_eq!(
        serde_json::from_value::<MachineMessage>(text).unwrap(),
        message
    );
    // Nothing else rides with them.
    assert!(
        serde_json::from_value::<MachineMessage>(json!({"capabilities": {
            "projects": [], "agents": [], "method": "daemon.stop",
        }}))
        .is_err()
    );
}

#[test]
fn capabilities_beyond_the_bounds_are_refused() {
    assert!(server().check().is_ok());
    let many = Capabilities {
        projects: (0..1025)
            .map(|n| Offered {
                name: format!("p{n}"),
                slots: 1,
            })
            .collect(),
        agents: Vec::new(),
    };
    assert!(many.check().is_err());
    let long = Capabilities {
        projects: Vec::new(),
        agents: vec!["c".repeat(65)],
    };
    assert!(long.check().is_err());
    let odd = Capabilities {
        projects: vec![Offered {
            name: "app\u{1b}[31m".to_owned(),
            slots: 1,
        }],
        agents: Vec::new(),
    };
    assert!(odd.check().is_err());
}
