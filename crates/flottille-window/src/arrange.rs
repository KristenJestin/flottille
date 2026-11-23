//! How the user arranged their projects (`docs/design.md` §10, the sidebar): the order of the
//! list and the groups they made, which projects are dragged into and out of. It is the user's view
//! of their projects, kept by this machine's daemon with the rest of the sidebar's state
//! (`docs/protocol.md`, "The draft and window methods"), never in a project.
//!
//! A project is named by its key, the name of its row (a project several machines share is one
//! row, one key). A key that names no project of the list is ignored, and a project the
//! arrangement does not name yet (a new one) is at the end of the list, outside any group.

use serde_json::{Value, json};

/// A group of the user's: a name, whether it is unfolded, and the projects in it, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub open: bool,
    pub projects: Vec<String>,
}

/// The longest name a group has.
pub const NAME_LIMIT: usize = 60;

/// The user's order and groups, as the daemon keeps them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arrangement {
    /// The projects outside any group, in order.
    pub order: Vec<String>,
    /// The groups, in order: they come first in the list.
    pub groups: Vec<Group>,
}

/// What the list shows: the groups with the projects they hold now, then the projects outside any
/// group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listed {
    pub groups: Vec<Group>,
    pub loose: Vec<String>,
}

impl Listed {
    /// The group a project is in, if any.
    pub fn group_of(&self, key: &str) -> Option<&str> {
        self.groups
            .iter()
            .find(|group| group.projects.iter().any(|k| k == key))
            .map(|group| group.id.as_str())
    }

    /// Every project, groups first, in the order drawn.
    pub fn keys(&self) -> Vec<String> {
        self.groups
            .iter()
            .flat_map(|group| group.projects.iter().cloned())
            .chain(self.loose.iter().cloned())
            .collect()
    }
}

impl Arrangement {
    /// The arrangement as the daemon keeps it (`window.get`'s `state`, under `arrangement`).
    pub fn of(kept: &Value) -> Self {
        let keys = |value: &Value| -> Vec<String> {
            value
                .as_array()
                .map(|keys| {
                    keys.iter()
                        .filter_map(|key| key.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let groups = kept["groups"]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .filter_map(|group| {
                        Some(Group {
                            id: group["id"].as_str()?.to_owned(),
                            name: group["name"].as_str()?.to_owned(),
                            open: group["open"].as_bool().unwrap_or(true),
                            projects: keys(&group["projects"]),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            order: keys(&kept["order"]),
            groups,
        }
    }

    /// What the daemon keeps (`window.set`, key `arrangement`), whole.
    pub fn to_json(&self) -> Value {
        json!({
            "order": self.order,
            "groups": self.groups.iter().map(|group| {
                json!({"id": group.id, "name": group.name, "open": group.open,
                    "projects": group.projects})
            }).collect::<Vec<_>>(),
        })
    }

    /// The list for the projects `present`, in the order the daemon lists them: what the
    /// arrangement names and the list has, then the projects it does not name, at the end. What it
    /// names and the list has not (a project out of reach for now) is left out here, and kept.
    pub fn listed(&self, present: &[String]) -> Listed {
        let mut seen: Vec<String> = Vec::new();
        let mut take = |keys: &[String]| -> Vec<String> {
            let mut taken = Vec::new();
            for key in keys {
                if present.contains(key) && !seen.contains(key) {
                    seen.push(key.clone());
                    taken.push(key.clone());
                }
            }
            taken
        };
        let groups = self
            .groups
            .iter()
            .map(|group| Group {
                projects: take(&group.projects),
                ..group.clone()
            })
            .collect();
        let mut loose = take(&self.order);
        loose.extend(take(present));
        Listed { groups, loose }
    }

    /// The projects of `present` the arrangement does not name yet, put at the end of the list:
    /// a move counts places among the projects it names.
    fn adopt(&mut self, present: &[String]) {
        for key in present {
            let named = self.order.contains(key)
                || self.groups.iter().any(|group| group.projects.contains(key));
            if !named {
                self.order.push(key.clone());
            }
        }
    }

    /// A project dragged to `index` among the projects of `group` (`None`: outside any group),
    /// counted once it has left its own place and among the projects `present`: those that are
    /// not (out of reach for now) keep their places, and the project is put by those that are.
    pub fn move_project(
        &mut self,
        present: &[String],
        key: &str,
        group: Option<&str>,
        index: usize,
    ) {
        self.adopt(present);
        if !present.iter().any(|k| k == key) {
            return;
        }
        if group.is_some_and(|id| !self.groups.iter().any(|held| held.id == id)) {
            return;
        }
        self.order.retain(|k| k != key);
        for held in &mut self.groups {
            held.projects.retain(|k| k != key);
        }
        let place = match group.and_then(|id| self.groups.iter_mut().find(|held| held.id == id)) {
            Some(held) => &mut held.projects,
            None => &mut self.order,
        };
        let there: Vec<usize> = place
            .iter()
            .enumerate()
            .filter(|(_, k)| present.contains(k))
            .map(|(at, _)| at)
            .collect();
        let at = match there.get(index) {
            Some(at) => *at,
            None => there.last().map_or(place.len(), |last| last + 1),
        };
        place.insert(at, key.to_owned());
    }

    /// A group dragged to `index` among the groups, counted once it has left its own.
    pub fn move_group(&mut self, id: &str, index: usize) {
        let Some(at) = self.groups.iter().position(|group| group.id == id) else {
            return;
        };
        let group = self.groups.remove(at);
        self.groups.insert(index.min(self.groups.len()), group);
    }

    /// A new, empty group at the end of the groups, unfolded: its id. An id is never given twice
    /// while a group holds it; a group removed gives its up, which is harmless, for an id is read
    /// where the arrangement is, and nothing outside it keeps one.
    pub fn add_group(&mut self, name: &str) -> String {
        let next = self
            .groups
            .iter()
            .filter_map(|group| group.id.strip_prefix('g')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let id = format!("g{next}");
        self.groups.push(Group {
            id: id.clone(),
            name: name.trim().to_owned(),
            open: true,
            projects: Vec::new(),
        });
        id
    }

    /// A group given another name.
    pub fn rename_group(&mut self, id: &str, name: &str) {
        if let Some(group) = self.groups.iter_mut().find(|group| group.id == id) {
            group.name = name.trim().to_owned();
        }
    }

    /// A group folded or unfolded.
    pub fn open_group(&mut self, id: &str, open: bool) {
        if let Some(group) = self.groups.iter_mut().find(|group| group.id == id) {
            group.open = open;
        }
    }

    /// A group removed: its projects go back to the list, at the end, in their order.
    pub fn remove_group(&mut self, id: &str) {
        let Some(at) = self.groups.iter().position(|group| group.id == id) else {
            return;
        };
        let group = self.groups.remove(at);
        self.order.extend(group.projects);
    }

    /// Whether a name is already a group's, whatever the case (beyond ASCII too).
    pub fn names(&self, name: &str) -> bool {
        let name = name.trim().to_lowercase();
        self.groups
            .iter()
            .any(|group| group.name.to_lowercase() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn projects_the_arrangement_does_not_name_go_at_the_end() {
        let arrangement = Arrangement {
            order: keys(&["b", "a"]),
            groups: Vec::new(),
        };
        let listed = arrangement.listed(&keys(&["a", "b", "c"]));
        assert_eq!(listed.loose, keys(&["b", "a", "c"]));
    }

    #[test]
    fn a_key_that_names_no_project_is_ignored() {
        let arrangement = Arrangement {
            order: keys(&["gone", "a"]),
            groups: vec![Group {
                id: "g1".into(),
                name: "Work".into(),
                open: true,
                projects: keys(&["b", "gone"]),
            }],
        };
        let listed = arrangement.listed(&keys(&["a", "b"]));
        assert_eq!(listed.groups[0].projects, keys(&["b"]));
        assert_eq!(listed.loose, keys(&["a"]));
    }

    #[test]
    fn a_project_is_moved_into_a_group_and_out_of_it() {
        let present = keys(&["a", "b", "c"]);
        let mut arrangement = Arrangement::default();
        let id = arrangement.add_group("Work");
        arrangement.move_project(&present, "b", Some(&id), 0);
        let listed = arrangement.listed(&present);
        assert_eq!(listed.groups[0].projects, keys(&["b"]));
        assert_eq!(listed.loose, keys(&["a", "c"]));
        arrangement.move_project(&present, "b", None, 1);
        let listed = arrangement.listed(&present);
        assert!(listed.groups[0].projects.is_empty());
        assert_eq!(listed.loose, keys(&["a", "b", "c"]));
    }

    #[test]
    fn removing_a_group_puts_its_projects_back_at_the_end() {
        let present = keys(&["a", "b", "c"]);
        let mut arrangement = Arrangement::default();
        let id = arrangement.add_group("Work");
        arrangement.move_project(&present, "a", Some(&id), 0);
        arrangement.remove_group(&id);
        assert!(arrangement.groups.is_empty());
        assert_eq!(arrangement.listed(&present).loose, keys(&["b", "c", "a"]));
    }

    #[test]
    fn groups_are_moved_among_themselves() {
        let mut arrangement = Arrangement::default();
        let first = arrangement.add_group("One");
        let second = arrangement.add_group("Two");
        assert_ne!(first, second);
        arrangement.move_group(&second, 0);
        let names: Vec<_> = arrangement.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Two", "One"]);
    }

    #[test]
    fn a_project_absent_while_another_is_moved_returns_to_its_place_and_group() {
        let all = keys(&["a", "b", "c", "d"]);
        let mut arrangement = Arrangement::default();
        let id = arrangement.add_group("Work");
        arrangement.move_project(&all, "b", Some(&id), 0);
        arrangement.move_project(&all, "c", Some(&id), 1);
        arrangement.move_project(&all, "d", None, 0);
        let before = arrangement.listed(&all);
        assert_eq!(before.groups[0].projects, keys(&["b", "c"]));
        assert_eq!(before.loose, keys(&["d", "a"]));

        // `b` is out of reach (a machine that is off): the user moves others meanwhile.
        let present = keys(&["a", "c", "d"]);
        arrangement.move_project(&present, "a", None, 0);
        arrangement.move_group(&id, 0);
        arrangement.open_group(&id, true);
        assert_eq!(arrangement.listed(&present).loose, keys(&["a", "d"]));

        // It is back: in its group, before `c`, as it was.
        let listed = arrangement.listed(&all);
        assert_eq!(listed.groups[0].projects, keys(&["b", "c"]));
        assert_eq!(listed.loose, keys(&["a", "d"]));
    }

    #[test]
    fn a_name_is_taken_whatever_its_case_beyond_ascii() {
        let mut arrangement = Arrangement::default();
        arrangement.add_group("Équipe");
        assert!(arrangement.names("équipe"));
        assert!(arrangement.names(" ÉQUIPE "));
        assert!(!arrangement.names("Equipe"));
    }

    #[test]
    fn it_is_what_the_daemon_keeps() {
        let present = keys(&["a", "b"]);
        let mut arrangement = Arrangement::default();
        let id = arrangement.add_group("Work");
        arrangement.move_project(&present, "b", Some(&id), 0);
        assert_eq!(Arrangement::of(&arrangement.to_json()), arrangement);
    }
}
