//! What the scenario suites share: a desktop of temporary files, and ways to count frames.
#![allow(dead_code)]

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use flottille_ui::omarchy::Environment;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

/// A desktop of temporary files: an os-release and a state folder.
pub struct Desktop {
    pub dir: tempfile::TempDir,
}

impl Desktop {
    pub fn new(omarchy: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let id = if omarchy { "omarchy" } else { "arch" };
        std::fs::write(
            dir.path().join("os-release"),
            format!("NAME=Test\nID={id}\n"),
        )
        .unwrap();
        Self { dir }
    }

    pub fn env(&self) -> Environment {
        Environment {
            os_release: self.dir.path().join("os-release"),
            state_home: self.dir.path().join("state"),
        }
    }

    pub fn write_colors(&self, text: &str) {
        let path = self.env().colors_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
}

pub fn init(cx: &mut TestAppContext, env: Environment) {
    cx.update(|cx| flottille_ui::init_with(env, cx));
    cx.run_until_parked();
}

/// Counts every time `entity` asks to be redrawn: a timer tick, a state change, an animation
/// frame all go through `notify`.
pub fn count_redraws<T: 'static>(
    entity: &Entity<T>,
    cx: &mut VisualTestContext,
) -> Rc<Cell<usize>> {
    let count = Rc::new(Cell::new(0));
    let seen = count.clone();
    cx.update(|_, cx| {
        cx.observe(entity, move |_, _| seen.set(seen.get() + 1))
            .detach();
    });
    count
}

pub fn advance(cx: &mut VisualTestContext, by: Duration) {
    let step = Duration::from_millis(16);
    let mut gone = Duration::ZERO;
    while gone < by {
        cx.executor().advance_clock(step);
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.run_until_parked();
        gone += step;
    }
}

pub fn pending_frames(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
}
