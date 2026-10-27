//! PadWire: Launchpad covers played on Linux, without Ableton.

mod app;
mod signals;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use libadwaita as adw;
use libadwaita::gtk::{gio, glib};

pub const APP_ID: &str = "dev._2c2t.padwire";

fn main() -> glib::ExitCode {
    // Symphonia talks of every format it tries on each file: PadWire says
    // itself what it could not read.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,symphonia=off"),
    )
    .init();
    signals::block();

    let application = adw::Application::builder()
        // Another id runs a second PadWire beside the first, as tests do;
        // the same one hands what it is given to the PadWire running.
        .application_id(std::env::var("PADWIRE_APP_ID").unwrap_or_else(|_| APP_ID.to_owned()))
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    // One window, made on first use; later activations bring it back.
    let app: Rc<RefCell<Option<Rc<app::App>>>> = Rc::default();
    let window = {
        let app = app.clone();
        move |application: &adw::Application| {
            app.borrow_mut()
                .get_or_insert_with(|| app::App::new(application))
                .clone()
        }
    };
    application.connect_activate({
        let window = window.clone();
        move |application| window(application).present()
    });
    // `padwire cover.zip`, or a cover opened from the file manager.
    application.connect_open(move |application, files, _| {
        let app = window(application);
        app.present();
        if let Some(path) = files.first().and_then(|file| file.path()) {
            app.open(path);
        }
    });
    // However PadWire ends, the Launchpad is given back as it was found.
    application.connect_shutdown(move |_| {
        if let Some(app) = app.borrow().as_ref() {
            app.shutdown();
        }
    });
    let (quit, quit_rx) = async_channel::bounded::<()>(1);
    signals::watch(move || {
        let _ = quit.try_send(());
    });
    glib::spawn_future_local({
        let application = application.clone();
        async move {
            if quit_rx.recv().await.is_ok() {
                application.quit();
            }
        }
    });
    application.run()
}
