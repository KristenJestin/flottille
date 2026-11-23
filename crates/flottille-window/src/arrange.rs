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
    /// The arrangement as the daemon keeps it (`window.get`'s `state`, under `sidebar`).
    pub fn of(sidebar: &Value) -> Self {
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
        let groups = sidebar["groups"]
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
            order: keys(&sidebar["order"]),
            groups,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty() && self.groups.is_empty()
    }

    /// What is kept of it, as the sidebar's keys (`order`, `groups`).
    pub fn put(&self, sidebar: &mut Value) {
        if !self.order.is_empty() {
            sidebar["order"] = json!(self.order);
        }
        if !self.groups.is_empty() {
            sidebar["groups"] = self
                .groups
                .iter()
                .map(|group| {
                    json!({"id": group.id, "name": group.name, "open": group.open,
                        "projects": group.projects})
                })
                .collect();
        }
    }

    /// The list for the projects `present`, in the order the daemon lists them: what the
    /// arrangement names and the list has, then the projects it does not name, at the end.
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

    /// The arrangement of `listed`, whole: what a change writes, the new order of every project.
    fn from_listed(listed: Listed) -> Self {
        Self {
            order: listed.loose,
            groups: listed.groups,
        }
    }

    /// A project dragged to `index` among the projects of `group` (`None`: outside any group),
    /// counted once it has left its own place.
    pub fn move_project(
        &mut self,
        present: &[String],
        key: &str,
        group: Option<&str>,
        index: usize,
    ) {
        let mut listed = self.listed(present);
        if !listed.keys().iter().any(|k| k == key) {
            return;
        }
        listed.loose.retain(|k| k != key);
        for held in &mut listed.groups {
            held.projects.retain(|k| k != key);
        }
        let place = match group {
            Some(id) => listed
                .groups
                .iter_mut()
                .find(|held| held.id == id)
                .map(|held| &mut held.projects),
            None => Some(&mut listed.loose),
        };
        if let Some(place) = place {
            place.insert(index.min(place.len()), key.to_owned());
        } else {
            return;
        }
        *self = Self::from_listed(listed);
    }

    /// A group dragged to `index` among the groups, counted once it has left its own.
    pub fn move_group(&mut self, present: &[String], id: &str, index: usize) {
        let mut listed = self.listed(present);
        let Some(at) = listed.groups.iter().position(|group| group.id == id) else {
            return;
        };
        let group = listed.groups.remove(at);
        listed.groups.insert(index.min(listed.groups.len()), group);
        *self = Self::from_listed(listed);
    }

    /// A new, empty group at the end of the groups, unfolded: its id.
    pub fn add_group(&mut self, present: &[String], name: &str) -> String {
        let mut listed = self.listed(present);
        let next = listed
            .groups
            .iter()
            .filter_map(|group| group.id.strip_prefix('g')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let id = format!("g{next}");
        listed.groups.push(Group {
            id: id.clone(),
            name: name.trim().to_owned(),
            open: true,
            projects: Vec::new(),
        });
        *self = Self::from_listed(listed);
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
    pub fn remove_group(&mut self, present: &[String], id: &str) {
        let mut listed = self.listed(present);
        let Some(at) = listed.groups.iter().position(|group| group.id == id) else {
            return;
        };
        let group = listed.groups.remove(at);
        listed.loose.extend(group.projects);
        *self = Self::from_listed(listed);
    }

    /// Whether a name is already a group's.
    pub fn names(&self, name: &str) -> bool {
        self.groups
            .iter()
            .any(|group| group.name.eq_ignore_ascii_case(name.trim()))
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
        let id = arrangement.add_group(&present, "Work");
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
        let id = arrangement.add_group(&present, "Work");
        arrangement.move_project(&present, "a", Some(&id), 0);
        arrangement.remove_group(&present, &id);
        assert!(arrangement.groups.is_empty());
        assert_eq!(arrangement.listed(&present).loose, keys(&["b", "c", "a"]));
    }

    #[test]
    fn groups_are_moved_among_themselves() {
        let present = keys(&["a"]);
        let mut arrangement = Arrangement::default();
        let first = arrangement.add_group(&present, "One");
        let second = arrangement.add_group(&present, "Two");
        assert_ne!(first, second);
        arrangement.move_group(&present, &second, 0);
        let names: Vec<_> = arrangement.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Two", "One"]);
    }

    #[test]
    fn it_is_what_the_daemon_keeps() {
        let present = keys(&["a", "b"]);
        let mut arrangement = Arrangement::default();
        let id = arrangement.add_group(&present, "Work");
        arrangement.move_project(&present, "b", Some(&id), 0);
        let mut kept = json!({});
        arrangement.put(&mut kept);
        assert_eq!(Arrangement::of(&kept), arrangement);
    }
}
