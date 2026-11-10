//! A settings-like page for the `Subpage` scenarios: two tabs, the first a list of features that
//! opens the page of one in place of itself.
#![allow(dead_code)]

use flottille_ui::{Icon, Subpage, Tabs, icon};
use gpui_kit::{
    AnyView, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement, Render,
    SharedString, Styled, TestSupportExt as _, Window, div, px,
};

/// The page of one feature, a view of its own as a screen's page is.
pub struct Page(pub SharedString);

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(SharedString::from(format!("{}-page", self.0)))
            .h(px(200.))
            .child(icon(Icon::Check))
            .child(format!("The page of {}", self.0))
            .test_support()
    }
}

pub struct Settings {
    pub tab: usize,
    pub open: Option<SharedString>,
    pub page: Option<AnyView>,
    /// What the list told it to open: a key, or `None` for back.
    pub told: Vec<Option<SharedString>>,
}

impl Settings {
    pub fn new() -> Self {
        Self {
            tab: 0,
            open: None,
            page: None,
            told: Vec::new(),
        }
    }
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let tabs = {
            let this = this.clone();
            Tabs::new("tabs", "Settings", self.tab)
                .tab("features", "Features")
                .tab("other", "Other")
                .on_choose(move |index, _, cx| {
                    this.update(cx, |this, cx| {
                        this.tab = index;
                        cx.notify();
                    })
                    .ok();
                })
        };
        let body = if self.tab == 0 {
            let mut subpage = Subpage::new("feat", "Features")
                .entry(
                    "review",
                    Icon::Check,
                    "Review",
                    "On",
                    "Reads the changes before they are kept",
                )
                .entry(
                    "specs",
                    Icon::Text,
                    "Specs",
                    "Off",
                    "Writes the spec before building",
                )
                .entry(
                    "connections",
                    Icon::Plus,
                    "Connections",
                    "2 on",
                    "Reaches other services",
                )
                .open(self.open.as_deref())
                .on_open(move |key, _, cx| {
                    this.update(cx, |this, cx| {
                        this.told.push(key.clone());
                        this.page = key.clone().map(|key| cx.new(|_| Page(key)).into());
                        this.open = key;
                        cx.notify();
                    })
                    .ok();
                });
            if let Some(page) = &self.page {
                subpage = subpage.page(page.clone());
            }
            div().child(subpage).into_any_element()
        } else {
            div()
                .id("other-panel")
                .child("Other")
                .test_support()
                .into_any_element()
        };
        div().size_full().flex().flex_col().child(tabs).child(body)
    }
}
