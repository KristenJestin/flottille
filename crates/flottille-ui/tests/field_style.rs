//! A text field takes its colours from the palette of now, and keeps taking them: a field made
//! in the light theme is drawn in the dark one after a switch, caret included.

mod common;

use common::{Desktop, advance, init};
use flottille_ui::theme::{Shade, set_palette};
use flottille_ui::{Palette, StyledField, field_style, follow_theme, palette};
use gpui_kit::base::input::InputEditorStyle;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, Render, TestAppContext, Window, div,
};

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A field that keeps the last style it was given, as gpui-kit's keeps its own.
#[derive(Default)]
struct Probe {
    style: Option<InputEditorStyle>,
}

impl StyledField for Probe {
    fn set_field_style(&mut self, style: InputEditorStyle) {
        self.style = Some(style);
    }
}

fn made(cx: &mut TestAppContext) -> Entity<Probe> {
    cx.update(|cx| {
        cx.new(|cx: &mut Context<Probe>| {
            let mut field = Probe::default();
            follow_theme(&mut field, cx);
            field
        })
    })
}

fn style_of(field: &Entity<Probe>, cx: &mut impl gpui_kit::AppContext) -> InputEditorStyle {
    field.read_with(cx, |field, _| {
        field.style.clone().expect("a style was given")
    })
}

fn light(cx: &mut TestAppContext) {
    cx.update(|cx| set_palette(Palette::flottille(Shade::Light), cx));
    cx.run_until_parked();
}

fn dark(cx: &mut TestAppContext) {
    cx.update(|cx| set_palette(Palette::flottille(Shade::Dark), cx));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_field_made_in_light_takes_the_dark_colours_after_a_switch(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    cx.update(|cx| cx.set_reduce_motion(true));
    light(cx);
    let field = made(cx);
    let in_light = Palette::flottille(Shade::Light);
    let style = style_of(&field, cx);
    assert_eq!(style.foreground, in_light.text);
    assert_eq!(style.muted_foreground, in_light.faint);

    dark(cx);
    let in_dark = Palette::flottille(Shade::Dark);
    let style = style_of(&field, cx);
    assert_eq!(style.foreground, in_dark.text, "the text");
    assert_eq!(style.muted_foreground, in_dark.faint, "the placeholder");
    assert_eq!(style.selection, in_dark.run.opacity(0.35), "the selection");
    assert_eq!(style.caret, in_dark.acc, "the caret");
}

#[gpui_kit::test]
fn a_field_follows_the_palette_through_its_cross_fade(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    cx.update(|cx| cx.set_reduce_motion(true));
    light(cx);
    cx.update(|cx| cx.set_reduce_motion(false));
    let field = made(cx);
    let (_, cx) = cx.add_window_view(|_, _| Blank);
    cx.update(|_, cx| set_palette(Palette::flottille(Shade::Dark), cx));
    advance(cx, std::time::Duration::from_millis(100));
    let midway = style_of(&field, cx);
    assert_ne!(
        midway.caret,
        Palette::flottille(Shade::Light).acc,
        "it left light"
    );
    assert_ne!(
        midway.caret,
        Palette::flottille(Shade::Dark).acc,
        "it is on its way"
    );

    advance(cx, std::time::Duration::from_secs(1));
    assert_eq!(
        style_of(&field, cx).caret,
        Palette::flottille(Shade::Dark).acc
    );
    assert_eq!(
        style_of(&field, cx).caret,
        cx.update(|_, cx| palette(cx)).acc
    );
}

#[gpui_kit::test]
fn the_caret_is_the_accent_colour_on_both_themes(cx: &mut TestAppContext) {
    init(cx, Desktop::new(false).env());
    cx.update(|cx| cx.set_reduce_motion(true));
    for shade in [Shade::Light, Shade::Dark] {
        cx.update(|cx| set_palette(Palette::flottille(shade), cx));
        cx.run_until_parked();
        let p = Palette::flottille(shade);
        let style = cx.update(|cx| field_style(cx));
        assert_eq!(style.caret, p.acc, "{shade:?}");
        assert_ne!(
            style.caret, p.field,
            "the caret is not the field's own colour"
        );
    }
}
