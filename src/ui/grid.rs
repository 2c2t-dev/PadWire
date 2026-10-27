//! The Launchpad on screen: its pads and round buttons, lit as the real
//! one is, and played with the mouse.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::cairo;
use libadwaita as adw;
use libadwaita::gtk;

use padwire::model::{LedColor, LedTarget};

/// What was clicked: the same lights the Launchpad has.
pub type Hit = LedTarget;

/// Cells across and down: the grid, the side column and the top row.
const CELLS: f64 = 9.0;
/// The gap between cells, as a part of a cell.
const GAP: f64 = 0.12;

#[derive(Default)]
struct State {
    leds: HashMap<LedTarget, LedColor>,
    pressed: Option<Hit>,
}

#[derive(Clone)]
pub struct Grid {
    pub widget: gtk::DrawingArea,
    state: Rc<RefCell<State>>,
}

impl Grid {
    /// `on_hit` is told of each press, then of its release.
    pub fn new(on_hit: impl Fn(Hit, bool) + 'static) -> Self {
        let widget = gtk::DrawingArea::builder()
            .hexpand(true)
            .vexpand(true)
            .content_width(360)
            .content_height(360)
            .build();
        let state = Rc::new(RefCell::new(State::default()));
        let grid = Grid { widget, state };

        let draw_state = grid.state.clone();
        grid.widget.set_draw_func(move |_, cr, width, height| {
            draw(
                cr,
                f64::from(width),
                f64::from(height),
                &draw_state.borrow(),
            );
        });
        adw::StyleManager::default().connect_dark_notify({
            let widget = grid.widget.clone();
            move |_| widget.queue_draw()
        });

        // A drag rather than a click: it tells of the release however far
        // the pointer went meanwhile.
        let on_hit = Rc::new(on_hit);
        let drag = gtk::GestureDrag::new();
        drag.set_button(gtk::gdk::BUTTON_PRIMARY);
        drag.connect_drag_begin({
            let grid = grid.clone();
            let on_hit = on_hit.clone();
            move |_, x, y| {
                let size = (
                    f64::from(grid.widget.width()),
                    f64::from(grid.widget.height()),
                );
                let Some(hit) = hit_test(x, y, size) else {
                    return;
                };
                grid.state.borrow_mut().pressed = Some(hit);
                grid.widget.queue_draw();
                on_hit(hit, true);
            }
        });
        let release = {
            let grid = grid.clone();
            move || {
                let Some(hit) = grid.state.borrow_mut().pressed.take() else {
                    return;
                };
                grid.widget.queue_draw();
                on_hit(hit, false);
            }
        };
        let release = Rc::new(release);
        drag.connect_drag_end({
            let release = release.clone();
            move |_, _, _| release()
        });
        drag.connect_cancel(move |_, _| release());
        grid.widget.add_controller(drag);
        grid
    }

    pub fn change(&self, changes: &[(LedTarget, LedColor)]) {
        let mut state = self.state.borrow_mut();
        for &(target, color) in changes {
            if color.is_off() {
                state.leds.remove(&target);
            } else {
                state.leds.insert(target, color);
            }
        }
        drop(state);
        self.widget.queue_draw();
    }

    pub fn replace(&self, lit: &[(LedTarget, LedColor)]) {
        self.state.borrow_mut().leds.clear();
        self.change(lit);
    }
}

/// The size of a cell, and where the grid starts, to be centred.
fn geometry(width: f64, height: f64) -> (f64, f64, f64) {
    let cell = (width.min(height) / CELLS).floor();
    let left = ((width - cell * CELLS) / 2.0).floor();
    let top = ((height - cell * CELLS) / 2.0).floor();
    (cell, left, top)
}

/// What is at a cell: row 0 is the top row of round buttons, and column 8
/// the side.
fn at_cell(column: u8, row: u8) -> Option<LedTarget> {
    match (column, row) {
        (8, 0) => None,
        (c, 0) => Some(LedTarget::Top(c + 1)),
        (8, r) => Some(LedTarget::Side(9 - r)),
        (c, r) => Some(LedTarget::Grid(c + 1, 9 - r)),
    }
}

fn hit_test(x: f64, y: f64, (width, height): (f64, f64)) -> Option<Hit> {
    let (cell, left, top) = geometry(width, height);
    if cell <= 0.0 {
        return None;
    }
    let (column, row) = ((x - left) / cell, (y - top) / cell);
    if !(0.0..CELLS).contains(&column) || !(0.0..CELLS).contains(&row) {
        return None;
    }
    at_cell(column as u8, row as u8)
}

fn draw(cr: &cairo::Context, width: f64, height: f64, state: &State) {
    let (cell, left, top) = geometry(width, height);
    let dark = adw::StyleManager::default().is_dark();
    let unlit = if dark {
        (1.0, 1.0, 1.0, 0.09)
    } else {
        (0.0, 0.0, 0.0, 0.09)
    };
    let outline = if dark {
        (1.0, 1.0, 1.0, 0.85)
    } else {
        (0.0, 0.0, 0.0, 0.7)
    };
    let gap = cell * GAP;
    for row in 0..9u8 {
        for column in 0..9u8 {
            let Some(target) = at_cell(column, row) else {
                continue;
            };
            let x = left + f64::from(column) * cell + gap / 2.0;
            let y = top + f64::from(row) * cell + gap / 2.0;
            let size = cell - gap;
            let round = !matches!(target, LedTarget::Grid(..));
            if round {
                let radius = size * 0.32;
                cr.arc(
                    x + size / 2.0,
                    y + size / 2.0,
                    radius,
                    0.0,
                    std::f64::consts::TAU,
                );
            } else {
                rounded_rect(cr, x, y, size, size, size * 0.12);
            }
            match state
                .leds
                .get(&target)
                .and_then(|color| color.display_rgb())
            {
                Some([r, g, b]) => cr.set_source_rgb(
                    f64::from(r) / 255.0,
                    f64::from(g) / 255.0,
                    f64::from(b) / 255.0,
                ),
                None => cr.set_source_rgba(unlit.0, unlit.1, unlit.2, unlit.3),
            }
            if state.pressed == Some(target) {
                let _ = cr.fill_preserve();
                cr.set_source_rgba(outline.0, outline.1, outline.2, outline.3);
                cr.set_line_width((cell * 0.05).max(1.5));
                let _ = cr.stroke();
            } else {
                let _ = cr.fill();
            }
        }
    }
}

fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    use std::f64::consts::FRAC_PI_2;
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -FRAC_PI_2, 0.0);
    cr.arc(
        x + width - radius,
        y + height - radius,
        radius,
        0.0,
        FRAC_PI_2,
    );
    cr.arc(
        x + radius,
        y + height - radius,
        radius,
        FRAC_PI_2,
        2.0 * FRAC_PI_2,
    );
    cr.arc(
        x + radius,
        y + radius,
        radius,
        2.0 * FRAC_PI_2,
        3.0 * FRAC_PI_2,
    );
    cr.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells() {
        assert_eq!(at_cell(0, 8), Some(LedTarget::Grid(1, 1)));
        assert_eq!(at_cell(7, 1), Some(LedTarget::Grid(8, 8)));
        assert_eq!(at_cell(8, 1), Some(LedTarget::Side(8)));
        assert_eq!(at_cell(0, 0), Some(LedTarget::Top(1)));
        assert_eq!(at_cell(8, 0), None);
        // 90 pixels a cell, centred across.
        assert_eq!(
            hit_test(105.0, 5.0, (900.0, 810.0)),
            Some(LedTarget::Top(1))
        );
        assert_eq!(hit_test(10.0, 5.0, (900.0, 810.0)), None);
    }
}
