//! A clean end on SIGINT, SIGTERM and SIGHUP, so that the Launchpad is
//! always given back as it was found.
//!
//! glib 0.22 has no Unix signal sources any more, so the signals are blocked
//! in every thread, which inherit the mask from the main one, and waited
//! for in one. The first asks the application to quit; a second ends it at
//! once.

use std::mem::MaybeUninit;

fn termination_set() -> libc::sigset_t {
    // SAFETY: plain libc calls on a set owned here and initialised first.
    unsafe {
        let mut set = MaybeUninit::<libc::sigset_t>::uninit();
        libc::sigemptyset(set.as_mut_ptr());
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::sigaddset(set.as_mut_ptr(), signal);
        }
        set.assume_init()
    }
}

/// Block the signals: before any thread is started, for all to inherit.
pub fn block() {
    let set = termination_set();
    // SAFETY: `set` is valid; the old mask is not wanted.
    let rc = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) };
    if rc != 0 {
        log::warn!("cannot block the termination signals (error {rc})");
    }
}

/// Wait for the signals on a thread of their own, calling `on_signal` on
/// the first.
pub fn watch(on_signal: impl FnOnce() + Send + 'static) {
    let spawned = std::thread::Builder::new()
        .name("padwire-signals".into())
        .spawn(move || {
            let set = termination_set();
            let mut on_signal = Some(on_signal);
            loop {
                let mut signal: libc::c_int = 0;
                // SAFETY: `set` and `signal` live through the call.
                if unsafe { libc::sigwait(&set, &mut signal) } != 0 {
                    return;
                }
                match on_signal.take() {
                    Some(on_signal) => {
                        log::info!("signal {signal}: quitting");
                        on_signal();
                    }
                    None => std::process::exit(128 + signal),
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("cannot wait for signals: {e}");
    }
}
