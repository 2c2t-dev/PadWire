//! The application: the engine started, the window built, and covers
//! opened into it.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use libadwaita as adw;
use libadwaita::gtk::{self, gdk, gio, glib};

use padwire::config::{Config, LibraryEntry};
use padwire::device::profiles::DeviceInput;
use padwire::device::{DeviceListener, DeviceManager, DeviceOutput};
use padwire::engine::audio::{AudioBackend, Bank, CpalBackend};
use padwire::engine::session::{Event, Session, WindowSink};
use padwire::engine::{decode, led};
use padwire::formats::{self, Cover};
use padwire::model::LedTarget;

use crate::ui::grid::Hit;
use crate::ui::library::Library;
use crate::ui::player::Player;
use crate::ui::preferences;

/// What a cover being loaded says, from the thread loading it.
enum Loading {
    Read {
        title: String,
        author: Option<String>,
    },
    Progress(usize, usize),
    Done {
        cover: Box<Cover>,
        bank: Bank,
        failures: Vec<String>,
    },
    Failed(String),
}

pub struct App {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    navigation: adw::NavigationView,
    library: Library,
    player: Player,
    session: Session,
    config: RefCell<Config>,
    devices: RefCell<DeviceManager>,
    audio: Option<CpalBackend>,
    /// Counts the covers opened, so that one opened over another still
    /// loading wins.
    loading: Cell<u64>,
}

/// Tells the session what the Launchpad plays, and the window whether one
/// is there.
struct Listener {
    session: Session,
    events: async_channel::Sender<Event>,
}

impl DeviceListener for Listener {
    fn input(&self, input: DeviceInput) {
        self.session.input(input);
    }

    fn connected(&self, model: Option<&'static str>) {
        let _ = self
            .events
            .try_send(Event::Device(model.map(str::to_owned)));
    }
}

impl App {
    pub fn new(application: &adw::Application) -> Rc<Self> {
        let config = Config::load();

        let (audio, audio_error) = match CpalBackend::start(config.buffer_frames) {
            Ok((backend, control)) => (Some((backend, control)), None),
            Err(e) => {
                log::error!("no audio: {e}");
                (None, Some(e.to_string()))
            }
        };
        let (backend, control) = audio.map_or((None, None), |(backend, control)| {
            (Some(backend), Some(control))
        });

        let (events, events_rx) = async_channel::unbounded();
        let (notices, notices_rx) = crossbeam_channel::unbounded();
        let output = DeviceOutput::default();
        let leds = led::spawn(
            vec![
                Box::new(output.clone()),
                Box::new(WindowSink(events.clone())),
            ],
            notices,
        );
        let session = Session::new(
            control,
            leds.clone(),
            notices_rx,
            events.clone(),
            config.first_page_on_top,
        );
        let listener = Arc::new(Listener {
            session: session.clone(),
            events: events.clone(),
        });
        let devices = DeviceManager::start(output, leds, listener, config.midi_port.clone());

        let app = Rc::new_cyclic(|weak: &std::rc::Weak<App>| {
            let on_open = {
                let weak = weak.clone();
                move |path: PathBuf| {
                    if let Some(app) = weak.upgrade() {
                        app.open(path);
                    }
                }
            };
            let on_remove = {
                let weak = weak.clone();
                move |path: PathBuf| {
                    if let Some(app) = weak.upgrade() {
                        app.forget(&path);
                    }
                }
            };
            let library = Library::new(on_open, on_remove);
            let on_hit = {
                let session = session.clone();
                move |hit: Hit, pressed: bool| match hit {
                    LedTarget::Grid(x, y) if pressed => session.press(x, y),
                    LedTarget::Grid(x, y) => session.release(x, y),
                    LedTarget::Side(n) => session.input(DeviceInput::Side { n, pressed }),
                    LedTarget::Top(n) => session.input(DeviceInput::Top { n, pressed }),
                }
            };
            let on_auto = {
                let session = session.clone();
                move |mode: Option<padwire::engine::autoplay::AutoMode>| match mode {
                    Some(mode) => session.start_autoplay(mode),
                    None => session.stop_autoplay(),
                }
            };
            let player = Player::new(on_hit, on_auto);

            let navigation = adw::NavigationView::new();
            navigation.add(&library.page);
            let toasts = adw::ToastOverlay::new();
            toasts.set_child(Some(&navigation));
            let window = adw::ApplicationWindow::builder()
                .application(application)
                .title("PadWire")
                .default_width(720)
                .default_height(820)
                .width_request(360)
                .height_request(420)
                .content(&toasts)
                .build();

            App {
                window,
                toasts,
                navigation,
                library,
                player,
                session,
                config: RefCell::new(config),
                devices: RefCell::new(devices),
                audio: backend,
                loading: Cell::new(0),
            }
        });

        app.library.show(&app.config.borrow().library);
        app.setup_actions(application);
        app.setup_drop();

        // Leaving the player stops the cover and frees its sounds.
        app.player.page.connect_hidden({
            let app = Rc::downgrade(&app);
            move |_| {
                if let Some(app) = app.upgrade() {
                    app.loading.set(app.loading.get() + 1);
                    app.session.unload();
                }
            }
        });

        glib::spawn_future_local({
            let player = app.player.clone();
            async move {
                while let Ok(event) = events_rx.recv().await {
                    player.apply(&event);
                }
            }
        });

        if let Some(error) = audio_error {
            app.toast(&format!("No sound: {error}"));
        }
        app
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Stop playing, and let go of the Launchpad.
    pub fn shutdown(&self) {
        self.session.unload();
        self.devices.borrow_mut().shutdown();
    }

    fn setup_actions(self: &Rc<Self>, application: &adw::Application) {
        let action = |name: &str, run: fn(&Rc<App>)| {
            let action = gio::SimpleAction::new(name, None);
            let app = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(app) = app.upgrade() {
                    run(&app);
                }
            });
            action
        };
        self.window
            .add_action(&action("open-file", |app| app.choose(false)));
        self.window
            .add_action(&action("open-folder", |app| app.choose(true)));
        application.add_action(&action("preferences", |app| app.preferences()));
        application.add_action(&action("about", |app| app.about()));
        application.add_action(&action("quit", |app| app.window.close()));
        application.set_accels_for_action("win.open-file", &["<Control>o"]);
        application.set_accels_for_action("app.preferences", &["<Control>comma"]);
        application.set_accels_for_action("app.quit", &["<Control>q"]);
    }

    /// Covers dropped on the window are opened.
    fn setup_drop(self: &Rc<Self>) {
        let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let app = Rc::downgrade(self);
        target.connect_drop(move |_, value, _, _| {
            let (Some(app), Ok(files)) = (app.upgrade(), value.get::<gdk::FileList>()) else {
                return false;
            };
            match files.files().first().and_then(|file| file.path()) {
                Some(path) => {
                    app.open(path);
                    true
                }
                None => false,
            }
        });
        self.toasts.add_controller(target);
    }

    fn choose(self: &Rc<Self>, folder: bool) {
        let app = Rc::clone(self);
        glib::spawn_future_local(async move {
            let dialog = gtk::FileDialog::builder().modal(true).build();
            let chosen = if folder {
                dialog.set_title("Open a Cover Folder");
                dialog.select_folder_future(Some(&app.window)).await
            } else {
                dialog.set_title("Open a Cover");
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                for (name, suffixes) in [
                    ("All covers", &["zip", "als"][..]),
                    ("UniPack archives", &["zip"]),
                    ("Ableton Live Sets", &["als"]),
                ] {
                    let filter = gtk::FileFilter::new();
                    filter.set_name(Some(name));
                    for suffix in suffixes {
                        filter.add_suffix(suffix);
                    }
                    filters.append(&filter);
                }
                dialog.set_filters(Some(&filters));
                dialog.open_future(Some(&app.window)).await
            };
            if let Some(path) = chosen.ok().and_then(|file| file.path()) {
                app.open(path);
            }
        });
    }

    /// Open a cover: read it and decode its sounds on a thread, then play it.
    pub fn open(self: &Rc<Self>, path: PathBuf) {
        let generation = self.loading.get() + 1;
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        if self.navigation.visible_page().as_ref() != Some(&self.player.page) {
            self.navigation.push(&self.player.page);
        }
        // After the push, which may have hidden a cover playing.
        self.session.unload();
        self.loading.set(generation);
        self.player.show_loading(&name);

        let rate = self.session.audio_rate().unwrap_or(48_000);
        let options = formats::Options {
            ableton_libraries: self.config.borrow().ableton_libraries.clone(),
        };
        let (tx, rx) = async_channel::unbounded();
        let thread_path = path.clone();
        std::thread::Builder::new()
            .name("padwire-load".into())
            .spawn(move || {
                let cover = match formats::open_with(&thread_path, &options) {
                    Ok(cover) => cover,
                    Err(e) => {
                        let _ = tx.send_blocking(Loading::Failed(e.to_string()));
                        return;
                    }
                };
                let _ = tx.send_blocking(Loading::Read {
                    title: cover.project.title.clone(),
                    author: cover.project.author.clone(),
                });
                let progress = |done, total| {
                    let _ = tx.try_send(Loading::Progress(done, total));
                };
                let (bank, failures) =
                    decode::decode_all(&*cover.files, &cover.project.samples, rate, &progress);
                let _ = tx.send_blocking(Loading::Done {
                    cover: Box::new(cover),
                    bank,
                    failures,
                });
            })
            .expect("cannot start loading");

        let app = Rc::clone(self);
        glib::spawn_future_local(async move {
            while let Ok(message) = rx.recv().await {
                if app.loading.get() != generation {
                    return;
                }
                match message {
                    Loading::Read { title, author } => {
                        app.player.set_title(&title, author.as_deref())
                    }
                    Loading::Progress(done, total) => app.player.show_progress(done, total),
                    Loading::Failed(error) => {
                        app.navigation.pop_to_tag("library");
                        app.toast(&format!("{name} cannot be opened: {error}"));
                    }
                    Loading::Done {
                        cover,
                        bank,
                        failures,
                    } => app.play(&path, *cover, bank, failures),
                }
            }
        });
    }

    fn play(&self, path: &Path, cover: Cover, bank: Bank, failures: Vec<String>) {
        let Cover {
            project,
            format,
            warnings,
            ..
        } = cover;
        log::info!(
            "playing {}: {} pages, {} sounds",
            project.title,
            project.pages.len(),
            project.samples.len()
        );
        {
            let mut config = self.config.borrow_mut();
            config.remember(LibraryEntry {
                path: path.to_owned(),
                title: project.title.clone(),
                author: project.author.clone(),
                format,
            });
            config.save();
            self.library.show(&config.library);
        }
        self.player
            .set_title(&project.title, project.author.as_deref());
        let autoplay = project.autoplay.is_some();
        self.session.load(Arc::new(project), Arc::new(bank));
        self.player.set_autoplay(autoplay);
        self.player.show_playing();

        let problems: Vec<String> = warnings.into_iter().chain(failures).collect();
        if !problems.is_empty() {
            for problem in &problems {
                log::warn!("{problem}");
            }
            let toast = adw::Toast::builder()
                .title(if problems.len() == 1 {
                    "Part of this cover could not be read".to_owned()
                } else {
                    format!("{} parts of this cover could not be read", problems.len())
                })
                .button_label("Details")
                .timeout(8)
                .build();
            let window = self.window.clone();
            toast.connect_button_clicked(move |_| show_problems(&window, &problems));
            self.toasts.add_toast(toast);
        }
    }

    fn forget(&self, path: &Path) {
        let mut config = self.config.borrow_mut();
        config.library.retain(|entry| entry.path != path);
        config.save();
        self.library.show(&config.library);
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(
            adw::Toast::builder()
                .title(glib::markup_escape_text(message))
                .timeout(6)
                .build(),
        );
    }

    fn preferences(self: &Rc<Self>) {
        let app = Rc::downgrade(self);
        let output = self
            .audio
            .as_ref()
            .map(|audio| audio.description().to_owned());
        let dialog = preferences::dialog(&self.config.borrow(), output, move |config| {
            let Some(app) = app.upgrade() else { return };
            app.session.set_first_page_on_top(config.first_page_on_top);
            app.devices.borrow().set_port(config.midi_port.clone());
            config.save();
            *app.config.borrow_mut() = config;
        });
        dialog.present(Some(&self.window));
    }

    fn about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("PadWire")
            .application_icon(crate::APP_ID)
            .developer_name("Fabien MILLET")
            .version(env!("CARGO_PKG_VERSION"))
            .comments("Launchpad covers, played on Linux without Ableton")
            .website("https://2c2t.dev")
            .build();
        about.add_legal_section(
            "Launchpad palette",
            Some("From Apollo Studio, © 2019 mat1jaczyyy"),
            gtk::License::Bsd3,
            None,
        );
        about.present(Some(&self.window));
    }
}

/// What could not be read of a cover, in full.
fn show_problems(window: &adw::ApplicationWindow, problems: &[String]) {
    let dialog = adw::AlertDialog::new(Some("What Could Not Be Read"), None);
    let label = gtk::Label::builder()
        .label(problems.join("\n"))
        .wrap(true)
        .xalign(0.0)
        .selectable(true)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .child(&label)
        .min_content_height(120)
        .max_content_height(360)
        .propagate_natural_height(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    dialog.set_extra_child(Some(&scrolled));
    dialog.add_response("close", "_Close");
    dialog.present(Some(window));
}
