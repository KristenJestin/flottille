//! People named the way an agent knows them (#795): a display name, a nickname, an account id or
//! an e-mail, found among the people a forge or a tracker lists, and turned into the identifier its
//! command line takes. The listing itself is each forge's and tracker's (`forge/people.rs`,
//! `tickets/people.rs`); how a name is matched is here, one way for all of them.

/// The most people one refusal or one listing names.
pub(crate) const SHOWN: usize = 10;

/// A person a forge or a tracker lists.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Person {
    /// The display name, empty when the forge says none.
    pub name: String,
    /// What a result says them by, and the agent names them back by: a login, a nickname.
    pub handle: String,
    /// What the command line takes for them.
    pub send: String,
    /// Every other way to name them: an account id, an e-mail, a slug.
    pub keys: Vec<String>,
}

impl Person {
    pub fn new(name: &str, handle: &str, send: &str, keys: &[&str]) -> Person {
        Person {
            name: name.trim().to_owned(),
            handle: handle.trim().to_owned(),
            send: send.trim().to_owned(),
            keys: keys
                .iter()
                .map(|key| key.trim().to_owned())
                .filter(|key| !key.is_empty())
                .collect(),
        }
    }

    /// Every identifier of the person, folded.
    fn identifiers(&self) -> Vec<String> {
        [&self.handle, &self.send]
            .into_iter()
            .chain(&self.keys)
            .filter(|key| !key.is_empty())
            .map(|key| fold(key))
            .collect()
    }

    /// `Jane Doe (jdoe)`, or the handle alone when the forge gave no display name.
    pub fn said(&self) -> String {
        match (self.name.is_empty(), self.handle.is_empty()) {
            (false, false) if fold(&self.name) != fold(&self.handle) => {
                format!("{} ({})", self.name, self.handle)
            }
            (false, _) => self.name.clone(),
            (true, _) => self.handle.clone(),
        }
    }

    /// What `other`, the same person read elsewhere, adds.
    fn merge(&mut self, other: Person) {
        for (mine, theirs) in [
            (&mut self.name, other.name),
            (&mut self.handle, other.handle),
            (&mut self.send, other.send),
        ] {
            if mine.is_empty() {
                *mine = theirs;
            } else if !theirs.is_empty() && fold(mine) != fold(&theirs) {
                self.keys.push(theirs);
            }
        }
        for key in other.keys {
            if !self.keys.iter().any(|known| fold(known) == fold(&key)) {
                self.keys.push(key);
            }
        }
    }
}

/// The people of one listing, in the order they were read: those seen on the pull requests first.
#[derive(Default)]
pub(crate) struct People(Vec<Person>);

impl People {
    /// Adds a person, or what a person already listed does not say yet (the same account read in
    /// two places has one of its identifiers in common).
    pub fn add(&mut self, person: Person) {
        if person.handle.is_empty() && person.send.is_empty() && person.name.is_empty() {
            return;
        }
        let ids = person.identifiers();
        match self
            .0
            .iter_mut()
            .find(|known| known.identifiers().iter().any(|id| ids.contains(id)))
        {
            Some(known) => known.merge(person),
            None => self.0.push(person),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn all(&self) -> &[Person] {
        &self.0
    }

    /// Those that hold every word of `query`, in their name, their handle or another identifier.
    pub fn matching(&self, query: &str) -> Vec<&Person> {
        let words = words(query);
        self.0
            .iter()
            .filter(|person| {
                let held = fold(&format!(
                    "{} {} {}",
                    person.name,
                    person.handle,
                    person.keys.join(" ")
                ));
                words.iter().all(|word| held.contains(word.as_str()))
            })
            .collect()
    }

    /// The one person `given` names, as `docs/tools.md` ("People") says: an exact identifier (a
    /// handle, an account id, an e-mail), else the whole display name, else a display name that
    /// holds every word given; case, accents and extra spaces do not count. Never a guess among
    /// several: they are listed.
    pub fn find(&self, given: &str) -> Result<&Person, Miss<'_>> {
        let wanted = fold(given);
        let by_identifier: Vec<&Person> = self
            .0
            .iter()
            .filter(|person| person.identifiers().contains(&wanted))
            .collect();
        let by_name: Vec<&Person> = self
            .0
            .iter()
            .filter(|person| fold(&person.name) == wanted)
            .collect();
        let given_words = words(given);
        let by_words: Vec<&Person> = self
            .0
            .iter()
            .filter(|person| {
                let name = words(&person.name);
                !given_words.is_empty() && given_words.iter().all(|word| name.contains(word))
            })
            .collect();
        for step in [by_identifier, by_name, by_words] {
            match step.as_slice() {
                [] => continue,
                [one] => return Ok(one),
                _ => return Err(Miss::Several(step)),
            }
        }
        Err(Miss::Nobody)
    }
}

/// Why a name is no one person.
#[derive(Debug)]
pub(crate) enum Miss<'a> {
    Nobody,
    Several(Vec<&'a Person>),
}

impl Miss<'_> {
    /// The refusal: what was given, where it was looked for, and for several, who they are.
    pub fn refusal(&self, key: &str, given: &str, looked: &str) -> String {
        match self {
            Miss::Nobody => format!(
                "refused: {key}: {given:?} matches nobody in {looked}: its people are listed by \
                 people, and a name is a display name, a nickname, an account id or an e-mail"
            ),
            Miss::Several(found) => {
                let listed: Vec<String> = found.iter().take(SHOWN).map(|p| p.said()).collect();
                let more = match found.len() > SHOWN {
                    true => format!(" and {} more", found.len() - SHOWN),
                    false => String::new(),
                };
                format!(
                    "refused: {key}: {given:?} matches {} people in {looked}: {}{more}; name one \
                     by their identifier",
                    found.len(),
                    listed.join(", ")
                )
            }
        }
    }
}

/// Up to this many people one `people` answer lists.
const LISTED: usize = 50;

/// `found` as a `people` answer lists them, one per line: those seen first, the others after.
pub(crate) fn lines(found: &[&Person]) -> String {
    let mut text = String::new();
    for person in found.iter().take(LISTED) {
        text.push_str(&format!("- {}\n", person.said()));
    }
    if found.len() > LISTED {
        text.push_str(&format!(
            "… {} more: narrow with query.\n",
            found.len() - LISTED
        ));
    }
    text
}

/// The words of a text, folded.
fn words(text: &str) -> Vec<String> {
    fold(text).split(' ').map(str::to_owned).collect()
}

/// A text compared: lower case, without accents, its spaces single.
pub(crate) fn fold(text: &str) -> String {
    let plain: String = text.chars().flat_map(unaccented).collect();
    plain
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A letter without its accent: the Latin letters of Western Europe and the common ligatures.
fn unaccented(letter: char) -> Vec<char> {
    let plain: &str = match letter {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' => "a",
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ą' => "A",
        'ç' | 'ć' | 'č' => "c",
        'Ç' | 'Ć' | 'Č' => "C",
        'ď' | 'đ' => "d",
        'Ď' | 'Đ' => "D",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => "e",
        'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ę' | 'Ě' => "E",
        'ì' | 'í' | 'î' | 'ï' | 'ī' => "i",
        'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' => "I",
        'ł' => "l",
        'Ł' => "L",
        'ñ' | 'ń' | 'ň' => "n",
        'Ñ' | 'Ń' | 'Ň' => "N",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' | 'Ő' => "O",
        'œ' => "oe",
        'Œ' => "OE",
        'ř' => "r",
        'Ř' => "R",
        'ś' | 'š' | 'ş' => "s",
        'Ś' | 'Š' | 'Ş' => "S",
        'ß' => "ss",
        'ť' => "t",
        'Ť' => "T",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => "u",
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' | 'Ů' | 'Ű' => "U",
        'ý' | 'ÿ' => "y",
        'Ý' | 'Ÿ' => "Y",
        'ź' | 'ž' | 'ż' => "z",
        'Ź' | 'Ž' | 'Ż' => "Z",
        'æ' => "ae",
        'Æ' => "AE",
        other => return vec![other],
    };
    plain.chars().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jane() -> Person {
        Person::new("Jane Doe", "jdoe", "{jane-uuid}", &["jane@example.org"])
    }

    fn people() -> People {
        let mut people = People::default();
        people.add(jane());
        people.add(Person::new("Janet Roe", "jroe", "{janet-uuid}", &[]));
        people.add(Person::new("José Núñez", "jose", "{jose-uuid}", &[]));
        people
    }

    #[test]
    fn an_exact_identifier_wins_over_a_display_name() {
        let mut people = people();
        // A person whose display name is another's nickname.
        people.add(Person::new("jdoe", "other", "{other-uuid}", &[]));
        assert_eq!(people.find("JDOE").unwrap().name, "Jane Doe");
        assert_eq!(people.find("jane@example.org").unwrap().handle, "jdoe");
        assert_eq!(people.find("{jane-uuid}").unwrap().handle, "jdoe");
    }

    #[test]
    fn the_whole_display_name_wins_over_the_words_it_holds() {
        // `jane` alone is Jane and Janet's first word, but `Jane Doe` is one whole name.
        let people = people();
        assert_eq!(people.find("  jane   DOE ").unwrap().handle, "jdoe");
        assert!(matches!(people.find("jane"), Ok(person) if person.handle == "jdoe"));
    }

    #[test]
    fn a_word_alone_is_one_person_or_a_list() {
        let mut people = People::default();
        people.add(Person::new("Jane Doe", "jdoe", "a", &[]));
        people.add(Person::new("John Doe", "jodoe", "b", &[]));
        assert_eq!(people.find("jane").unwrap().handle, "jdoe");
        assert!(matches!(people.find("doe"), Err(Miss::Several(found)) if found.len() == 2));
        assert!(matches!(people.find("nobody"), Err(Miss::Nobody)));
    }

    #[test]
    fn accents_and_case_do_not_count() {
        let people = people();
        assert_eq!(people.find("jose nunez").unwrap().handle, "jose");
        assert_eq!(people.find("JOSÉ").unwrap().handle, "jose");
    }

    #[test]
    fn the_same_account_read_twice_is_one_person() {
        let mut people = People::default();
        people.add(Person::new("Jane Doe", "", "{jane-uuid}", &[]));
        people.add(Person::new(
            "",
            "jdoe",
            "{jane-uuid}",
            &["jane@example.org"],
        ));
        assert_eq!(people.all().len(), 1);
        assert_eq!(people.all()[0].said(), "Jane Doe (jdoe)");
        assert_eq!(people.find("jdoe").unwrap().send, "{jane-uuid}");
    }

    #[test]
    fn a_refusal_lists_ten_people_at_most() {
        let mut people = People::default();
        for n in 0..12 {
            people.add(Person::new(
                &format!("Sam {n}"),
                &format!("sam{n}"),
                &format!("id{n}"),
                &[],
            ));
        }
        let Err(miss) = people.find("sam") else {
            panic!("twelve people")
        };
        let said = miss.refusal("reviewers", "sam", "acme/app");
        assert!(said.contains("matches 12 people"), "{said}");
        assert!(
            said.contains("Sam 9 (sam9)") && !said.contains("Sam 10"),
            "{said}"
        );
        assert!(said.contains("and 2 more"), "{said}");
    }
}
