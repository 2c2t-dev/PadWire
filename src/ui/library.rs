//! The covers imported, to open one again, and the way to import more.

use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use libadwaita as adw;
use libadwaita::gtk;

use padwire::config::LibraryEntry;

#[derive(Clone)]
pub struct Library {
    pub page: adw::NavigationPage,
    stack: gtk::Stack,
    list: gtk::ListBox,
    on_open: Rc<dyn Fn(PathBuf)>,
    on_remove: Rc<dyn Fn(PathBuf)>,
}

impl Library {
    pub fn new(on_open: impl Fn(PathBuf) + 'static, on_remove: impl Fn(PathBuf) + 'static) -> Self {
        let menu = gtk::gio::Menu::new();
        menu.append(Some("Open _Folder…"), Some("win.open-folder"));
        let open = adw::SplitButton::builder()
            .label("_Open")
            .use_underline(true)
            .action_name("win.open-file")
            .menu_model(&menu)
            .tooltip_text("Open a UniPack archive or an Ableton Live Set")
            .dropdown_tooltip("More ways to open")
            .build();

        let main_menu = gtk::gio::Menu::new();
        main_menu.append(Some("_Preferences"), Some("app.preferences"));
        main_menu.append(Some("_About PadWire"), Some("app.about"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&main_menu)
            .primary(true)
            .tooltip_text("Main Menu")
            .build();

        let header = adw::HeaderBar::new();
        header.pack_start(&open);
        header.pack_end(&menu_button);

        let empty = adw::StatusPage::builder()
            .icon_name("view-app-grid-symbolic")
            .title("No Covers Yet")
            .description("Open a UniPack, as a .zip archive or a folder, or an Ableton cover's .als set, or drop one here")
            .build();
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Center)
            .build();
        for (label, action, suggested) in [
            ("Open _File…", "win.open-file", true),
            ("Open _Folder…", "win.open-folder", false),
        ] {
            let button = gtk::Button::builder()
                .label(label)
                .use_underline(true)
                .action_name(action)
                .css_classes(["pill"])
                .build();
            if suggested {
                button.add_css_class("suggested-action");
            }
            buttons.append(&button);
        }
        empty.set_child(Some(&buttons));

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .valign(gtk::Align::Start)
            .build();
        let clamp = adw::Clamp::builder()
            .maximum_size(720)
            .child(&list)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();

        let stack = gtk::Stack::new();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&scrolled, Some("list"));

        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&stack));

        let page = adw::NavigationPage::builder()
            .title("PadWire")
            .tag("library")
            .child(&view)
            .build();
        Library {
            page,
            stack,
            list,
            on_open: Rc::new(on_open),
            on_remove: Rc::new(on_remove),
        }
    }

    pub fn show(&self, entries: &[LibraryEntry]) {
        self.list.remove_all();
        for entry in entries {
            let subtitle = match &entry.author {
                Some(author) => format!("{author} · {}", entry.format.label()),
                None => entry.format.label().to_owned(),
            };
            let row = adw::ActionRow::builder()
                .title(glib_escape(&entry.title))
                .subtitle(glib_escape(&subtitle))
                .tooltip_text(entry.path.display().to_string())
                .activatable(true)
                .build();
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text("Remove From Library")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let path = entry.path.clone();
            let on_remove = self.on_remove.clone();
            remove.connect_clicked(move |_| on_remove(path.clone()));
            row.add_suffix(&remove);
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let path = entry.path.clone();
            let on_open = self.on_open.clone();
            row.connect_activated(move |_| on_open(path.clone()));
            self.list.append(&row);
        }
        self.stack
            .set_visible_child_name(if entries.is_empty() { "empty" } else { "list" });
    }
}

/// Text shown as it is in a row, which reads markup.
fn glib_escape(text: &str) -> String {
    gtk::glib::markup_escape_text(text).to_string()
}
