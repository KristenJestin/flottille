//! Type: Geist for the interface, Geist Mono for paths, branches, models, tool names and
//! arguments, times, counts, pills and section labels. Both are bundled (SIL OFL 1.1, the licence
//! is beside the files) and set with tabular figures everywhere.

use std::borrow::Cow;
use std::sync::Arc;

use gpui_kit::{App, FontFeatures, FontWeight, Styled, px};

pub const UI: &str = "Geist";
pub const MONO: &str = "Geist Mono";

/// Body text size, in pixels.
pub const BODY: f32 = 13.5;

const FONTS: [&[u8]; 6] = [
    include_bytes!("../assets/fonts/Geist-Regular.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
    include_bytes!("../assets/fonts/GeistMono-SemiBold.ttf"),
];

pub(crate) fn register(cx: &mut App) {
    let fonts = FONTS.iter().map(|bytes| Cow::Borrowed(*bytes)).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("flottille: the bundled fonts could not be loaded: {e}");
    }
}

fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// The two faces, applied to any styled element.
pub trait Type: Styled + Sized {
    /// The interface face at body size.
    fn ui_text(self) -> Self {
        self.font_family(UI)
            .font_features(tabular())
            .text_size(px(BODY))
            .line_height(px(BODY * 1.5))
    }

    /// The mono face at `size` pixels.
    fn mono(self, size: f32) -> Self {
        self.font_family(MONO)
            .font_features(tabular())
            .text_size(px(size))
            .line_height(px((size * 1.4).round()))
    }

    fn medium(self) -> Self {
        self.font_weight(FontWeight::MEDIUM)
    }

    fn semibold(self) -> Self {
        self.font_weight(FontWeight::SEMIBOLD)
    }
}

impl<T: Styled> Type for T {}
