//! A pasted text of the conversation read (#848): the file a large paste became, opened by the
//! click on its chip in the large modal, read-only, in the mono face, line by line.
//!
//! One viewer is held by the session's page and drawn once, at the end of it: the modal takes no
//! room where it is drawn. A chip's click calls [`TextViewer::show`].

use gpui_design_system::{Modal, Type, palette, space};
use gpui_kit::{
    Context, ElementId, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt as _, Window, div, px,
};

/// The lines drawn: a text of a million lines is not a million rows. What follows is counted.
const MAX_LINES: usize = 2_000;
/// The bytes drawn, whatever the lines: a single line of a mebibyte is not laid out whole.
const MAX_BYTES: usize = 256 * 1024;

pub struct TextViewer {
    name: String,
    lines: Vec<SharedString>,
    /// How many lines the text has, those not drawn included.
    total: usize,
    /// Lines were cut at [`MAX_BYTES`].
    cut: bool,
    bytes: usize,
    open: bool,
}

impl TextViewer {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            lines: Vec::new(),
            total: 0,
            cut: false,
            bytes: 0,
            open: false,
        }
    }

    /// Opens `text`. What it showed last stays in it while it fades out.
    pub fn show(&mut self, name: &str, text: &str, cx: &mut Context<Self>) {
        self.read(name, text);
        self.open = true;
        cx.notify();
    }

    fn read(&mut self, name: &str, text: &str) {
        self.name = name.to_owned();
        self.bytes = text.len();
        self.total = text.lines().count();
        // An empty line keeps its height: a row with no text has none.
        let mut room = MAX_BYTES;
        self.cut = false;
        self.lines = Vec::new();
        for line in text.lines().take(MAX_LINES) {
            if room == 0 {
                self.cut = true;
                break;
            }
            let mut end = line.len().min(room);
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            // No whole character fits: the cut is reached, and the rest is not drawn as empty rows.
            if end == 0 && !line.is_empty() {
                self.cut = true;
                break;
            }
            self.cut |= end < line.len();
            room -= end;
            let line = &line[..end];
            self.lines.push(SharedString::from(
                if line.is_empty() { " " } else { line }.to_owned(),
            ));
            // A line cut short ends the text drawn: a line after it would not follow it.
            if self.cut {
                break;
            }
        }
    }

    /// The modal's sentence: how long the text is, and what of it is not drawn.
    fn sentence(&self) -> String {
        let mut sentence = format!(
            "{} line{} · {}",
            self.total,
            if self.total == 1 { "" } else { "s" },
            crate::tokens::size(self.bytes)
        );
        if self.cut {
            sentence.push_str(&format!(
                " · the first {} are shown",
                crate::tokens::size(MAX_BYTES)
            ));
        } else if self.total > self.lines.len() {
            sentence.push_str(&format!(" · the first {} are shown", self.lines.len()));
        }
        sentence
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
    }

    /// Whether it is open.
    pub fn is_open(&self) -> bool {
        self.open
    }
}

impl Default for TextViewer {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for TextViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let viewer = cx.entity().downgrade();
        let sentence = self.sentence();
        let modal = Modal::new("pasted-text-modal", SharedString::from(self.name.clone()))
            .sentence(sentence)
            .large()
            .takes_focus()
            .open(self.open)
            .on_close(move |_, cx| {
                viewer.update(cx, |viewer, cx| viewer.close(cx)).ok();
            });
        if self.lines.is_empty() {
            return modal;
        }
        modal.child(
            div()
                .id("pasted-text-body")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .py(space::SM)
                .px(space::MD)
                .rounded(px(8.))
                .bg(p.field)
                .border_1()
                .border_color(p.line)
                .children(self.lines.iter().enumerate().map(|(n, line)| {
                    div()
                        .id(ElementId::Name(format!("pasted-text-line-{n}").into()))
                        .mono(12.)
                        .text_color(p.text)
                        .aria_label(line.clone())
                        .child(line.clone())
                        .test_support()
                })),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_line_of_a_mebibyte_is_drawn_cut_and_the_sentence_says_so() {
        let mut viewer = TextViewer::new();
        viewer.read("pasted-text.txt", &"x".repeat(1024 * 1024));
        let drawn: usize = viewer.lines.iter().map(|line| line.len()).sum();
        assert!(drawn <= 256 * 1024, "{drawn} characters drawn");
        assert!(drawn > 0);
        assert_eq!(
            viewer.sentence(),
            "1 line · 1.0 MiB · the first 256 KiB are shown"
        );
    }

    #[test]
    fn a_cut_one_to_three_bytes_before_a_multi_byte_character_draws_no_empty_row() {
        for character in ["é", "€", "🙂"] {
            for room in 1..character.len() {
                let mut viewer = TextViewer::new();
                let text = format!("{}\n{character}b\nz\nz\n", "a".repeat(MAX_BYTES - room));
                viewer.read("pasted-text.txt", &text);
                assert_eq!(
                    viewer.lines.len(),
                    1,
                    "{room} byte(s) left before {character:?}: the rows drawn"
                );
                assert!(viewer.cut);
            }
        }
    }

    #[test]
    fn a_line_cut_by_the_cap_is_the_last_line_drawn() {
        let mut viewer = TextViewer::new();
        viewer.read(
            "pasted-text.txt",
            &format!("{}\nshort\n", "a".repeat(MAX_BYTES + 10)),
        );
        assert_eq!(viewer.lines.len(), 1);
        assert!(viewer.cut);
    }

    #[test]
    fn a_line_cut_inside_a_character_is_the_last_line_drawn() {
        let mut viewer = TextViewer::new();
        let text = format!("{}é tail\nshort\n", "a".repeat(MAX_BYTES - 1));
        viewer.read("pasted-text.txt", &text);
        assert_eq!(viewer.lines.len(), 1, "the rows drawn after a cut line");
        assert!(viewer.cut);
    }

    #[test]
    fn a_text_drawn_whole_says_its_size_only() {
        let mut viewer = TextViewer::new();
        viewer.read("pasted-text.txt", "one\ntwo\n");
        assert_eq!(viewer.lines.len(), 2);
        assert_eq!(viewer.sentence(), "2 lines · 8 B");
    }
}
