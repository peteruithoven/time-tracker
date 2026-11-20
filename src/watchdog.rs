//! With `TIME_TRACKER_DEBUG_STALLS=1`, warns whenever the main loop was
//! blocked for more than 50 ms (the app couldn't draw or react meanwhile),
//! e.g. to check that syncing and snapshots keep the app responsive.

use std::cell::Cell;
use std::time::{Duration, Instant};

use gtk::glib;

const TICK: Duration = Duration::from_millis(10);
const STALL: Duration = Duration::from_millis(50);

pub fn start() {
    if std::env::var("TIME_TRACKER_DEBUG_STALLS").unwrap_or_default().is_empty() {
        return;
    }
    let last = Cell::new(Instant::now());
    let worst = Cell::new(Duration::ZERO);
    glib::timeout_add_local(TICK, move || {
        let now = Instant::now();
        let late = now.duration_since(last.replace(now)).saturating_sub(TICK);
        if late > STALL {
            worst.set(worst.get().max(late));
            glib::g_warning!(
                "time-tracker",
                "Main loop blocked for {} ms (worst so far {} ms)",
                late.as_millis(),
                worst.get().as_millis()
            );
        }
        glib::ControlFlow::Continue
    });
}
