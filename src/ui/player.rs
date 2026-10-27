//! A cover being played: the Launchpad on screen, the page showing, and
//! whether a real Launchpad is connected.

use adw::prelude::*;
use libadwaita as adw;
use libadwaita::gtk;

use std::cell::Cell;
use std::rc::Rc;

use padwire::engine::autoplay::AutoMode;
use padwire::engine::session::Event;

use super::grid::{Grid, Hit};

#[derive(Clone)]
pub struct Player {
    pub page: adw::NavigationPage,
    title: adw::WindowTitle,
    stack: gtk::Stack,
    progress: gtk::ProgressBar,
    loading_label: gtk::Label,
    page_label: gtk::Label,
    device_icon: gtk::Image,
    device_label: gtk::Label,
    play_through: gtk::ToggleButton,
    practice: gtk::ToggleButton,
    /// Set while the buttons are set to what runs, which they are not to
    /// take for a click.
    showing: Rc<Cell<bool>>,
    pub grid: Grid,
}

impl Player {
    /// `on_auto` is told to play the cover through, to practise it, or to
    /// stop.
    pub fn new(
        on_hit: impl Fn(Hit, bool) + 'static,
        on_auto: impl Fn(Option<AutoMode>) + 'static,
    ) -> Self {
        let title = adw::WindowTitle::new("", "");

        let device_icon = gtk::Image::from_icon_name("input-gaming-symbolic");
        let device_label = gtk::Label::new(None);
        let device = gtk::Box::builder()
            .spacing(6)
            .css_classes(["dim-label"])
            .build();
        device.append(&device_icon);
        device.append(&device_label);

        // The cover played through by itself, or practised with guides.
        let play_through = gtk::ToggleButton::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text("Play Through")
            .sensitive(false)
            .build();
        let practice = gtk::ToggleButton::builder()
            .label("Practice")
            .tooltip_text("Light the pads to play next, and wait for them")
            .sensitive(false)
            .build();
        let showing = Rc::new(Cell::new(false));
        let on_auto = Rc::new(on_auto);
        for (button, other, mode) in [
            (&play_through, &practice, AutoMode::Play),
            (&practice, &play_through, AutoMode::Practice),
        ] {
            let (other, showing, on_auto) = (other.clone(), showing.clone(), on_auto.clone());
            button.connect_toggled(move |button| {
                if showing.get() {
                    return;
                }
                if button.is_active() {
                    showing.set(true);
                    other.set_active(false);
                    showing.set(false);
                    on_auto(Some(mode));
                } else {
                    on_auto(None);
                }
            });
        }

        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));
        header.pack_start(&play_through);
        header.pack_start(&practice);
        header.pack_end(&device);

        // Loading: the sounds are decoded before anything plays.
        let spinner = adw::Spinner::builder()
            .width_request(48)
            .height_request(48)
            .build();
        let loading_label = gtk::Label::builder().css_classes(["dim-label"]).build();
        let progress = gtk::ProgressBar::builder().width_request(240).build();
        let loading = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        loading.append(&spinner);
        loading.append(&loading_label);
        loading.append(&progress);

        let grid = Grid::new(on_hit);
        let page_label = gtk::Label::builder()
            .css_classes(["title-4"])
            .margin_bottom(18)
            .build();
        let playing = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        let frame = adw::Clamp::builder()
            .maximum_size(820)
            .child(&grid.widget)
            .margin_top(12)
            .margin_start(12)
            .margin_end(12)
            .vexpand(true)
            .build();
        playing.append(&frame);
        playing.append(&page_label);

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&loading, Some("loading"));
        stack.add_named(&playing, Some("playing"));

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&stack));

        let page = adw::NavigationPage::builder()
            .title("Cover")
            .tag("player")
            .child(&view)
            .build();
        let player = Player {
            page,
            title,
            stack,
            progress,
            loading_label,
            page_label,
            device_icon,
            device_label,
            play_through,
            practice,
            showing,
            grid,
        };
        player.show_device(None);
        player
    }

    pub fn show_loading(&self, name: &str) {
        self.set_title(name, None);
        self.loading_label.set_label("Reading the cover…");
        self.progress.set_fraction(0.0);
        self.grid.replace(&[]);
        self.stack.set_visible_child_name("loading");
    }

    pub fn show_progress(&self, done: usize, total: usize) {
        self.loading_label
            .set_label(&format!("Loading sounds: {done} of {total}"));
        self.progress.set_fraction(if total == 0 {
            1.0
        } else {
            done as f64 / total as f64
        });
    }

    pub fn set_title(&self, title: &str, author: Option<&str>) {
        self.title.set_title(title);
        self.title.set_subtitle(author.unwrap_or(""));
        self.page.set_title(title);
    }

    pub fn show_playing(&self) {
        self.stack.set_visible_child_name("playing");
    }

    pub fn show_device(&self, model: Option<&str>) {
        match model {
            Some(model) => {
                self.device_label.set_label(model);
                self.device_icon
                    .set_tooltip_text(Some("Launchpad connected"));
                self.device_icon.set_opacity(1.0);
            }
            None => {
                self.device_label.set_label("No Launchpad");
                self.device_icon
                    .set_tooltip_text(Some("Plug in a Launchpad to play on it"));
                self.device_icon.set_opacity(0.55);
            }
        }
    }

    pub fn apply(&self, event: &Event) {
        match event {
            Event::Leds(changes) => self.grid.change(changes),
            Event::LedsRefresh(lit) => self.grid.replace(lit),
            Event::Page { index, count } => {
                self.page_label
                    .set_label(&format!("Page {} of {count}", index + 1));
            }
            Event::Device(model) => self.show_device(model.as_deref()),
            Event::AutoPlay(mode) => self.show_auto(*mode),
        }
    }

    /// Whether the cover playing can be played through.
    pub fn set_autoplay(&self, available: bool) {
        self.show_auto(None);
        for (button, tip) in [
            (&self.play_through, "Play Through"),
            (
                &self.practice,
                "Light the pads to play next, and wait for them",
            ),
        ] {
            button.set_sensitive(available);
            let tip = if available {
                tip
            } else {
                "This cover holds no performance to follow"
            };
            button.set_tooltip_text(Some(tip));
        }
    }

    fn show_auto(&self, mode: Option<AutoMode>) {
        self.showing.set(true);
        self.play_through.set_active(mode == Some(AutoMode::Play));
        self.practice.set_active(mode == Some(AutoMode::Practice));
        self.showing.set(false);
    }
}
