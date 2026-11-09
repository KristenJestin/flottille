//! What an agent asks with `ask` and how it is answered (`docs/tools.md`, "ask"): one question or
//! up to four, each with its options when the answers are known. An `agent.asked` event keeps
//! them in `questions`, and in `question` the text a parent, a pilot or an older reader reads;
//! an event written before an ask held several questions has only `question`, read as one
//! question without options.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most questions one ask holds.
pub const MOST_QUESTIONS: usize = 4;
/// The fewest and the most options a question offers, when it offers some.
pub const OPTIONS: std::ops::RangeInclusive<usize> = 2..=4;
/// The longest label of an option, in characters: a short choice, not a sentence.
pub const LABEL_CHARS: usize = 60;

/// One question of an ask.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Choice>,
    /// The user may pick several options.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multiple: bool,
}

/// An option of a question: a short label and an optional line that says it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The answer to one question: the options chosen, the words written, or both.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

impl Answer {
    /// Neither a choice nor a word.
    pub fn is_empty(&self) -> bool {
        self.choices.is_empty() && self.text.trim().is_empty()
    }
}

/// The questions of an `agent.asked` event's data (or of an open question as `agent.tree`
/// gives it): its `questions`, or for an older one, its `question` alone.
pub fn of(data: &Value) -> Vec<Question> {
    if let Some(questions) = data["questions"].as_array()
        && let Ok(questions) = questions
            .iter()
            .map(|q| serde_json::from_value(q.clone()))
            .collect::<Result<Vec<Question>, _>>()
        && !questions.is_empty()
    {
        return questions;
    }
    vec![Question {
        question: data["question"].as_str().unwrap_or_default().to_owned(),
        ..Question::default()
    }]
}

/// The answers of an `agent.answered` event's data that answered an ask.
pub fn answers_of(data: &Value) -> Vec<Answer> {
    serde_json::from_value(data["answers"].clone()).unwrap_or_default()
}

/// Whether the question has nothing but its words: no option, so its text is the whole ask.
fn plain(questions: &[Question]) -> bool {
    matches!(questions, [only] if only.options.is_empty())
}

/// The ask as one text, for a parent or a pilot that answers it in words, and for every reader
/// that shows one line of it: a question without options is its own text; otherwise each is
/// numbered (when there are several), its options listed under it.
pub fn text(questions: &[Question]) -> String {
    if plain(questions) {
        return questions[0].question.clone();
    }
    let numbered = questions.len() > 1;
    questions
        .iter()
        .enumerate()
        .map(|(at, q)| {
            let mut said = match numbered {
                true => format!("{}. {}", at + 1, q.question),
                false => q.question.clone(),
            };
            if !q.options.is_empty() {
                let pick = match q.multiple {
                    true => "Options (several may be chosen)",
                    false => "Options",
                };
                let options: Vec<String> = q
                    .options
                    .iter()
                    .map(|o| match &o.description {
                        Some(d) => format!("{} ({d})", o.label),
                        None => o.label.clone(),
                    })
                    .collect();
                said.push_str(&format!("\n   {pick}: {}", options.join("; ")));
            }
            said
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// What the agent receives as the answer: each question quoted, its answer under it (the
/// options chosen on one line, the words after). A question without options asked alone is
/// answered by the words alone, as a message of the user always answered it.
pub fn message(questions: &[Question], answers: &[Answer]) -> String {
    let said = |answer: &Answer| {
        let mut lines = Vec::new();
        if !answer.choices.is_empty() {
            lines.push(answer.choices.join(", "));
        }
        if !answer.text.trim().is_empty() {
            lines.push(answer.text.trim().to_owned());
        }
        lines.join("\n")
    };
    if plain(questions) {
        return answers.first().map(said).unwrap_or_default();
    }
    questions
        .iter()
        .zip(answers)
        .map(|(q, answer)| {
            let quoted: Vec<String> = q.question.lines().map(|line| format!("> {line}")).collect();
            format!("{}\n\n{}", quoted.join("\n"), said(answer))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Why `questions` cannot be asked, as the agent reads it.
pub fn refusal(questions: &[Question]) -> Option<String> {
    if questions.is_empty() {
        return Some("refused: the question is empty".into());
    }
    if questions.len() > MOST_QUESTIONS {
        return Some(format!(
            "refused: {} questions in one ask, {MOST_QUESTIONS} at most: ask the rest once these \
             are answered",
            questions.len()
        ));
    }
    for (at, q) in questions.iter().enumerate() {
        let n = at + 1;
        if q.question.trim().is_empty() {
            return Some(format!("refused: question {n} is empty"));
        }
        if !q.options.is_empty() && !OPTIONS.contains(&q.options.len()) {
            return Some(format!(
                "refused: question {n} has {} options: give 2 to 4, or none for an answer in \
                 words",
                q.options.len()
            ));
        }
        if q.multiple && q.options.is_empty() {
            return Some(format!(
                "refused: question {n} is multiple without options: give its options"
            ));
        }
        for option in &q.options {
            let label = option.label.trim();
            if label.is_empty() || label.chars().count() > LABEL_CHARS || label.contains('\n') {
                return Some(format!(
                    "refused: an option of question {n} needs a short label, one line of at most \
                     {LABEL_CHARS} characters"
                ));
            }
            if option
                .description
                .as_deref()
                .is_some_and(|d| d.contains('\n'))
            {
                return Some(format!(
                    "refused: the description of an option of question {n} is one line"
                ));
            }
        }
        let mut labels: Vec<&str> = q.options.iter().map(|o| o.label.trim()).collect();
        labels.sort_unstable();
        labels.dedup();
        if labels.len() != q.options.len() {
            return Some(format!(
                "refused: two options of question {n} have the same label"
            ));
        }
    }
    None
}

/// Why `answers` do not answer `questions`, as the user reads it.
pub fn answers_refusal(questions: &[Question], answers: &[Answer]) -> Option<String> {
    if answers.len() != questions.len() {
        return Some(format!(
            "{} answers for {} questions: answer each",
            answers.len(),
            questions.len()
        ));
    }
    for (at, (q, answer)) in questions.iter().zip(answers).enumerate() {
        let n = at + 1;
        if answer.is_empty() {
            return Some(format!("question {n} has no answer"));
        }
        if let Some(unknown) = answer
            .choices
            .iter()
            .find(|c| !q.options.iter().any(|o| &o.label == *c))
        {
            return Some(format!("question {n} has no option {unknown}"));
        }
        if !q.multiple && answer.choices.len() > 1 {
            return Some(format!("question {n} takes one option"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn three() -> Vec<Question> {
        serde_json::from_value(json!([
            {"question": "Push `feat/42-export` and open the pull request?",
             "options": [{"label": "Yes"}, {"label": "Not yet", "description": "Keep it local"}]},
            {"question": "Title of the pull request?"},
            {"question": "Who reviews it?", "multiple": true,
             "options": [{"label": "Alice"}, {"label": "Bob"}, {"label": "Carol"}]}
        ]))
        .unwrap()
    }

    #[test]
    fn an_older_ask_reads_as_one_question_without_options() {
        let read = of(&json!({"agent": "a", "question": "Keep the minimum?", "to": "user"}));
        assert_eq!(
            read,
            [Question {
                question: "Keep the minimum?".into(),
                ..Question::default()
            }]
        );
    }

    #[test]
    fn several_questions_read_to_a_parent_numbered_with_their_options() {
        assert_eq!(
            text(&three()),
            "1. Push `feat/42-export` and open the pull request?\n   Options: Yes; Not yet \
             (Keep it local)\n2. Title of the pull request?\n3. Who reviews it?\n   Options \
             (several may be chosen): Alice; Bob; Carol"
        );
    }

    #[test]
    fn the_message_quotes_each_question_and_says_its_answer_under_it() {
        let answers = [
            Answer {
                choices: vec!["Yes".into()],
                text: "Squash it first.".into(),
            },
            Answer {
                text: "Export orders as CSV".into(),
                ..Answer::default()
            },
            Answer {
                choices: vec!["Alice".into(), "Carol".into()],
                ..Answer::default()
            },
        ];
        assert_eq!(
            message(&three(), &answers),
            "> Push `feat/42-export` and open the pull request?\n\nYes\nSquash it first.\n\n\
             > Title of the pull request?\n\nExport orders as CSV\n\n> Who reviews it?\n\n\
             Alice, Carol"
        );
    }

    #[test]
    fn one_question_in_words_is_answered_by_the_words_alone() {
        let one = of(&json!({"question": "Which colour?"}));
        let answer = Answer {
            text: " Blue. ".into(),
            ..Answer::default()
        };
        assert_eq!(message(&one, &[answer]), "Blue.");
        assert_eq!(text(&one), "Which colour?");
    }

    #[test]
    fn an_ask_out_of_shape_is_refused_with_why() {
        let mut five = three();
        five.extend(three().into_iter().take(2));
        assert!(refusal(&five).unwrap().contains("4 at most"));
        let one_option: Vec<Question> =
            serde_json::from_value(json!([{"question": "Q?", "options": [{"label": "A"}]}]))
                .unwrap();
        assert!(refusal(&one_option).unwrap().contains("2 to 4"));
        assert_eq!(refusal(&three()), None);
    }

    #[test]
    fn answers_are_checked_against_their_questions() {
        let none = [Answer::default(), Answer::default(), Answer::default()];
        assert_eq!(
            answers_refusal(&three(), &none).as_deref(),
            Some("question 1 has no answer")
        );
        let two_yes = [
            Answer {
                choices: vec!["Yes".into(), "Not yet".into()],
                ..Answer::default()
            },
            Answer {
                text: "T".into(),
                ..Answer::default()
            },
            Answer {
                choices: vec!["Bob".into()],
                ..Answer::default()
            },
        ];
        assert_eq!(
            answers_refusal(&three(), &two_yes).as_deref(),
            Some("question 1 takes one option")
        );
    }
}
