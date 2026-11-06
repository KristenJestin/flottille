//! YAML as the forges read it. `serde_norway` refuses a key repeated in a mapping, as the YAML
//! rule says; Bitbucket and GitHub run such a file and keep the last one. This reads a file that
//! way and says which keys repeated, and where, so the answer can tell the user.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_norway::Value as Yaml;
use serde_norway::value::{Tag, TaggedValue};

/// A key met again in the mapping that holds it: where, as the ordinal of each of its two
/// appearances among all the keys of the file.
struct Repeat {
    path: String,
    first: usize,
    second: usize,
}

/// One read of the file, shared by the values it builds.
struct Reading {
    /// How many keys were met.
    keys: Cell<usize>,
    /// Fail on the key of this ordinal, to learn its line from the error.
    stop_at: Option<usize>,
    path: RefCell<Vec<String>>,
    repeats: RefCell<Vec<Repeat>>,
}

impl Reading {
    fn new(stop_at: Option<usize>) -> Self {
        Reading {
            keys: Cell::new(0),
            stop_at,
            path: RefCell::new(Vec::new()),
            repeats: RefCell::new(Vec::new()),
        }
    }

    /// The path of a key, `pipelines.custom.check-all`, a sequence's item `steps[0].name`.
    fn path_to(&self, key: &str) -> String {
        let mut path = String::new();
        for part in self.path.borrow().iter().map(String::as_str).chain([key]) {
            if !path.is_empty() && !part.starts_with('[') {
                path.push('.');
            }
            path.push_str(part);
        }
        path
    }
}

/// A value built the way `serde_norway::Value` builds itself, but a repeated key keeps its last
/// value and is recorded.
#[derive(Clone, Copy)]
struct Lenient<'a>(&'a Reading);

impl<'de> DeserializeSeed<'de> for Lenient<'_> {
    type Value = Yaml;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Yaml, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Lenient<'_> {
    type Value = Yaml;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any YAML value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Yaml, E> {
        Ok(Yaml::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Yaml, E> {
        Ok(Yaml::from(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Yaml, E> {
        Ok(Yaml::from(value))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Yaml, E> {
        Ok(Yaml::from(value))
    }

    fn visit_str<E>(self, value: &str) -> Result<Yaml, E> {
        Ok(Yaml::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Yaml, E> {
        Ok(Yaml::String(value))
    }

    fn visit_unit<E>(self) -> Result<Yaml, E> {
        Ok(Yaml::Null)
    }

    fn visit_none<E>(self) -> Result<Yaml, E> {
        Ok(Yaml::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Yaml, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Yaml, A::Error> {
        let mut sequence = Vec::new();
        loop {
            self.0
                .path
                .borrow_mut()
                .push(format!("[{}]", sequence.len()));
            let item = items.next_element_seed(self);
            self.0.path.borrow_mut().pop();
            match item? {
                Some(item) => sequence.push(item),
                None => return Ok(Yaml::Sequence(sequence)),
            }
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, mut entries: A) -> Result<Yaml, A::Error> {
        let mut mapping = serde_norway::Mapping::new();
        let mut firsts: HashMap<Yaml, usize> = HashMap::new();
        loop {
            let ordinal = self.0.keys.get();
            let key = match self.0.stop_at == Some(ordinal) {
                // The parser puts the line of what it was reading on the error: the key's own.
                true => match entries.next_key_seed(Refused)? {
                    Some(never) => match never {},
                    None => None,
                },
                false => entries.next_key::<Yaml>()?,
            };
            let Some(key) = key else { break };
            self.0.keys.set(ordinal + 1);
            let name = super::pipelines::scalar(&key).unwrap_or_default();
            match firsts.get(&key) {
                Some(&first) => self.0.repeats.borrow_mut().push(Repeat {
                    path: self.0.path_to(&name),
                    first,
                    second: ordinal,
                }),
                None => {
                    firsts.insert(key.clone(), ordinal);
                }
            }
            self.0.path.borrow_mut().push(name);
            let value = entries.next_value_seed(self);
            self.0.path.borrow_mut().pop();
            mapping.insert(key, value?);
        }
        Ok(Yaml::Mapping(mapping))
    }

    fn visit_enum<A: de::EnumAccess<'de>>(self, data: A) -> Result<Yaml, A::Error> {
        use de::VariantAccess;
        let (tag, contents) = data.variant::<String>()?;
        let value = contents.newtype_variant_seed(self)?;
        Ok(Yaml::Tagged(Box::new(TaggedValue {
            tag: Tag::new(tag),
            value,
        })))
    }
}

/// A key that fails when it is read, for the line the parser puts on the error.
struct Refused;

impl<'de> DeserializeSeed<'de> for Refused {
    type Value = std::convert::Infallible;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Refused {
    type Value = std::convert::Infallible;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("no key: this read stops here")
    }
}

/// The file as the forges read it, and a note for each key it defines more than once, the lines
/// being the file's. A syntax error is the parser's own error, with its line and column.
pub(super) fn parse(text: &str, path: &str) -> Result<(Yaml, Vec<String>), serde_norway::Error> {
    let reading = Reading::new(None);
    let yaml = Lenient(&reading).deserialize(serde_norway::Deserializer::from_str(text))?;
    let repeats = reading.repeats.into_inner();
    if repeats.is_empty() {
        return Ok((yaml, Vec::new()));
    }
    let mut ordinals: Vec<usize> = repeats.iter().flat_map(|r| [r.first, r.second]).collect();
    ordinals.sort_unstable();
    ordinals.dedup();
    let lines: HashMap<usize, usize> = ordinals
        .into_iter()
        .filter_map(|ordinal| Some((ordinal, line_of(text, ordinal)?)))
        .collect();
    // One note per key: its appearances in the order of the file, three or more said as such.
    let mut notes: Vec<(usize, String)> = Vec::new();
    for repeat in &repeats {
        if notes.iter().any(|(first, _)| *first == repeat.first) {
            continue;
        }
        let ordinals: Vec<usize> = std::iter::once(repeat.first)
            .chain(
                repeats
                    .iter()
                    .filter(|r| r.first == repeat.first)
                    .map(|r| r.second),
            )
            .collect();
        let said: Vec<String> = ordinals
            .iter()
            .filter_map(|o| lines.get(o))
            .map(usize::to_string)
            .collect();
        let times = match ordinals.len() {
            2 => "twice".to_owned(),
            n => format!("{n} times"),
        };
        let at = match said.split_last() {
            Some((last, rest)) if !rest.is_empty() => {
                format!(" (lines {} and {last})", rest.join(", "))
            }
            _ => String::new(),
        };
        notes.push((
            repeat.first,
            format!(
                "{path}: {} is defined {times}{at}; the last one is listed",
                repeat.path
            ),
        ));
    }
    Ok((yaml, notes.into_iter().map(|(_, note)| note).collect()))
}

/// The line of the key of this ordinal: the line the parser reports when the read stops there.
fn line_of(text: &str, ordinal: usize) -> Option<usize> {
    let reading = Reading::new(Some(ordinal));
    let stopped = Lenient(&reading)
        .deserialize(serde_norway::Deserializer::from_str(text))
        .err()?;
    stopped.location().map(|at| at.line())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_met_three_times_is_one_note_with_its_three_lines() {
        let text = "a:\n  x: 1\n  y: 2\n  x: 3\n  x: 4\n";

        let (yaml, notes) = parse(text, "f.yml").unwrap();

        assert_eq!(yaml["a"]["x"], Yaml::from(4));
        assert_eq!(
            notes,
            ["f.yml: a.x is defined 3 times (lines 2, 4 and 5); the last one is listed"]
        );
    }

    #[test]
    fn a_repeated_key_in_a_sequence_item_is_told_by_its_position() {
        let text = "steps:\n  - name: a\n  - name: b\n    name: c\n";

        let (yaml, notes) = parse(text, "f.yml").unwrap();

        assert_eq!(yaml["steps"][1]["name"], Yaml::from("c"));
        assert_eq!(
            notes,
            ["f.yml: steps[1].name is defined twice (lines 3 and 4); the last one is listed"]
        );
    }

    #[test]
    fn anchors_merge_keys_and_tags_are_kept() {
        let text = "base: &b\n  k: 1\nuse:\n  <<: *b\n  t: !custom v\n";

        let (mut yaml, notes) = parse(text, "f.yml").unwrap();

        assert!(notes.is_empty(), "{notes:?}");
        yaml.apply_merge().unwrap();
        assert_eq!(yaml["use"]["k"], Yaml::from(1));
        assert!(matches!(yaml["use"]["t"], Yaml::Tagged(_)));
    }

    #[test]
    fn a_syntax_error_keeps_the_parsers_line_and_column() {
        let why = parse("a: 1\nb: c: d\n", "f.yml").unwrap_err().to_string();

        assert!(why.contains("line 2 column 5"), "{why}");
    }
}
