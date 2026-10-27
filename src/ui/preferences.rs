//! The preferences: the Launchpad, its page buttons, and the audio buffer.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use libadwaita as adw;
use libadwaita::gtk;

use padwire::config::Config;
use padwire::device;

/// The buffer sizes offered, in frames.
const BUFFERS: [u32; 5] = [64, 128, 256, 512, 1024];

/// The dialog, which hands every change to `changed` as the whole config.
pub fn dialog(
    config: &Config,
    output: Option<String>,
    changed: impl Fn(Config) + 'static,
) -> adw::PreferencesDialog {
    let config = Rc::new(RefCell::new(config.clone()));
    let changed = Rc::new(move |config: &Rc<RefCell<Config>>| changed(config.borrow().clone()));

    // The Launchpad.
    let mut ports = device::port_names();
    if let Some(wanted) = &config.borrow().midi_port
        && !ports.contains(wanted)
    {
        ports.push(wanted.clone());
    }
    let mut labels = vec!["Automatic".to_owned()];
    labels.extend(ports.iter().cloned());
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let port = adw::ComboRow::builder()
        .title("MIDI Port")
        .subtitle("Automatic finds a Launchpad PadWire knows")
        .model(&gtk::StringList::new(&labels))
        .build();
    let selected = config
        .borrow()
        .midi_port
        .as_ref()
        .and_then(|wanted| ports.iter().position(|p| p == wanted))
        .map_or(0, |i| i + 1);
    port.set_selected(selected as u32);
    port.connect_selected_notify({
        let (config, changed) = (config.clone(), changed.clone());
        move |row| {
            let selected = row.selected() as usize;
            config.borrow_mut().midi_port =
                selected.checked_sub(1).and_then(|i| ports.get(i).cloned());
            changed(&config);
        }
    });

    let on_top = adw::SwitchRow::builder()
        .title("First Page at the Top")
        .subtitle("The side buttons count pages from the top, as in UniPad")
        .active(config.borrow().first_page_on_top)
        .build();
    on_top.connect_active_notify({
        let (config, changed) = (config.clone(), changed.clone());
        move |row| {
            config.borrow_mut().first_page_on_top = row.is_active();
            changed(&config);
        }
    });

    let launchpad = adw::PreferencesGroup::builder().title("Launchpad").build();
    launchpad.add(&port);
    launchpad.add(&on_top);

    // The audio.
    let labels: Vec<String> = BUFFERS
        .iter()
        .map(|frames| format!("{frames} frames"))
        .collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let buffer = adw::ComboRow::builder()
        .title("Buffer Size")
        .subtitle("Smaller plays sooner, larger is safer. Applies when PadWire starts again")
        .model(&gtk::StringList::new(&labels))
        .build();
    let current = config.borrow().buffer_frames;
    buffer.set_selected(
        BUFFERS
            .iter()
            .position(|&frames| frames == current)
            .unwrap_or(1) as u32,
    );
    buffer.connect_selected_notify({
        let (config, changed) = (config.clone(), changed.clone());
        move |row| {
            if let Some(&frames) = BUFFERS.get(row.selected() as usize) {
                config.borrow_mut().buffer_frames = frames;
                changed(&config);
            }
        }
    });
    let output = adw::ActionRow::builder()
        .title("Output")
        .subtitle(gtk::glib::markup_escape_text(
            &output.unwrap_or_else(|| "No sound output".into()),
        ))
        .subtitle_selectable(true)
        .build();

    let audio = adw::PreferencesGroup::builder().title("Audio").build();
    audio.add(&buffer);
    audio.add(&output);

    let page = adw::PreferencesPage::new();
    page.add(&launchpad);
    page.add(&audio);
    page.add(&libraries(config.clone(), changed.clone()));
    let dialog = adw::PreferencesDialog::new();
    dialog.add(&page);
    dialog
}

/// What is told of a change, with the whole config.
type Changed = Rc<dyn Fn(&Rc<RefCell<Config>>)>;
/// What shows the libraries again.
type Fill = Rc<dyn Fn()>;

/// Ableton's libraries: those found where Live is installed, and folders
/// the user adds, which Ableton covers may take sounds from that they do
/// not hold.
fn libraries(config: Rc<RefCell<Config>>, changed: Changed) -> adw::PreferencesGroup {
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add a Folder of Ableton's Sounds")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let group = adw::PreferencesGroup::builder()
        .title("Ableton Libraries")
        .description(
            "Ableton covers may use sounds of Ableton's library without holding them. \
             PadWire looks for them in these folders, from your own Ableton, and copies nothing. \
             For the covers opened from now on",
        )
        .header_suffix(&add)
        .build();
    let rows: Rc<RefCell<Vec<gtk::Widget>>> = Rc::default();

    // Shown again whenever the folders change: the remove buttons reach
    // the function that shows them through this.
    let show: Rc<RefCell<Option<Fill>>> = Rc::default();
    let fill: Fill = Rc::new({
        let (group, rows, config, changed, show) = (
            group.clone(),
            rows.clone(),
            config.clone(),
            changed.clone(),
            show.clone(),
        );
        move || {
            for row in rows.borrow_mut().drain(..) {
                group.remove(&row);
            }
            let add_row = |row: adw::ActionRow| {
                group.add(&row);
                rows.borrow_mut().push(row.upcast());
            };
            let found = padwire::formats::als::ableton_libraries();
            for path in &found {
                let row = library_row(path);
                row.add_suffix(
                    &gtk::Label::builder()
                        .label("Found")
                        .css_classes(["dim-label"])
                        .build(),
                );
                add_row(row);
            }
            let added = config.borrow().ableton_libraries.clone();
            for path in added {
                let row = library_row(&path);
                let remove = gtk::Button::builder()
                    .icon_name("user-trash-symbolic")
                    .tooltip_text("Remove")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .build();
                let (config, changed, show) = (config.clone(), changed.clone(), show.clone());
                remove.connect_clicked(move |_| {
                    config
                        .borrow_mut()
                        .ableton_libraries
                        .retain(|known| *known != path);
                    changed(&config);
                    let fill = show.borrow().clone();
                    if let Some(fill) = fill {
                        fill();
                    }
                });
                row.add_suffix(&remove);
                add_row(row);
            }
            if found.is_empty() && config.borrow().ableton_libraries.is_empty() {
                add_row(
                    adw::ActionRow::builder()
                        .title("No Library Found")
                        .subtitle("Add the Core Library, User Library or Packs folder of an Ableton you have")
                        .build(),
                );
            }
        }
    });
    *show.borrow_mut() = Some(fill.clone());
    fill();

    add.connect_clicked({
        let (config, changed, fill) = (config.clone(), changed.clone(), fill.clone());
        move |button| {
            let window = button.root().and_downcast::<gtk::Window>();
            let (config, changed, fill) = (config.clone(), changed.clone(), fill.clone());
            gtk::glib::spawn_future_local(async move {
                let dialog = gtk::FileDialog::builder()
                    .title("Add a Folder of Ableton's Sounds")
                    .modal(true)
                    .build();
                let Ok(folder) = dialog.select_folder_future(window.as_ref()).await else {
                    return;
                };
                let Some(path) = folder.path() else { return };
                {
                    let mut config = config.borrow_mut();
                    if !config.ableton_libraries.contains(&path) {
                        config.ableton_libraries.push(path);
                    }
                }
                changed(&config);
                fill();
            });
        }
    });
    group
}

fn library_row(path: &std::path::Path) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(gtk::glib::markup_escape_text(&library_name(path)))
        .subtitle(gtk::glib::markup_escape_text(&path.display().to_string()))
        .build()
}

/// What a library is called: its folder, with the Live it is of.
fn library_name(path: &std::path::Path) -> String {
    let parts: Vec<String> = path
        .components()
        .rev()
        .take(3)
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    match parts.as_slice() {
        // …/Live 12 Lite/Resources/Core Library
        [library, resources, live] if resources == "Resources" => format!("{library} of {live}"),
        [library, ..] => library.clone(),
        [] => path.display().to_string(),
    }
}
