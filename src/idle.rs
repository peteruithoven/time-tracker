//! Notices when you were away while the timer ran, so the app can ask what
//! to do with that time.
//!
//! While a timer runs, the app polls the time since the last keyboard or
//! mouse input from Mutter's idle monitor on the session bus
//! (`org.gnome.Mutter.IdleMonitor`, also offered by elementary's Gala).
//! Polling instead of the monitor's watches, because Gala never fires
//! those. The idle clock doesn't run during suspend, so a gap between polls
//! counts as away too.
//!
//! Settings: `idle-detection` and `idle-minutes`. For testing,
//! `TIME_TRACKER_DEBUG_AWAY=<minutes>` reports an away period ending at
//! startup.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::application::TimeTrackerApplication;
use crate::config::APP_ID;
use crate::format;

const BUS_NAME: &str = "org.gnome.Mutter.IdleMonitor";
const OBJECT_PATH: &str = "/org/gnome/Mutter/IdleMonitor/Core";
const INTERFACE: &str = "org.gnome.Mutter.IdleMonitor";
/// Seconds between polls while a timer runs.
const POLL_SECS: u32 = 5;
/// A gap between polls this long (or the threshold, if longer) means the
/// computer was suspended.
const MIN_GAP_SECS: i64 = 60;

/// A period without input while `entry_id` was running.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Away {
    /// When input stopped.
    pub since: i64,
    /// When input came back.
    pub back: i64,
    pub entry_id: i64,
}

/// Tracks when you went away and came back, from polls.
#[derive(Debug, Default)]
struct Detector {
    since: Option<i64>,
    last_poll: Option<i64>,
}

impl Detector {
    /// A poll at `now`, with the seconds since the last input when known.
    /// Returns the away period (since, back) once input is back.
    fn poll(&mut self, now: i64, idle: Option<i64>, threshold: i64) -> Option<(i64, i64)> {
        if let Some(previous) = self.last_poll.replace(now)
            && now - previous >= threshold.max(MIN_GAP_SECS)
        {
            self.mark(previous);
        }
        match idle {
            Some(idle) if idle >= threshold => {
                self.mark(now - idle);
                None
            }
            Some(idle) => self.since.take().map(|since| (since, now - idle)),
            // Without idle times, only suspends count, and you're back
            // once the computer runs again.
            None => self.since.take().map(|since| (since, now)),
        }
    }

    fn mark(&mut self, since: i64) {
        self.since = Some(self.since.map_or(since, |s| s.min(since)));
    }

    /// Not tracking: forget everything.
    fn reset(&mut self) {
        self.since = None;
        self.last_poll = None;
    }
}

/// Away period for the running entry, when it's long enough to ask about.
fn away_for_entry(since: i64, back: i64, entry_start: i64, entry_id: i64, threshold: i64) -> Option<Away> {
    let since = since.max(entry_start);
    (back - since >= threshold).then_some(Away { since, back, entry_id })
}

pub struct Monitor {
    settings: gio::Settings,
    proxy: RefCell<Option<gio::DBusProxy>>,
    /// The idle monitor answered; warn only once when it doesn't.
    available: Cell<Option<bool>>,
    polling: Cell<bool>,
    detector: RefCell<Detector>,
    on_away: Box<dyn Fn(Away)>,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor").finish_non_exhaustive()
    }
}

impl Monitor {
    /// Starts watching; `on_away` is called when you come back.
    pub fn start(on_away: impl Fn(Away) + 'static) -> Rc<Self> {
        let monitor = Rc::new(Self {
            settings: gio::Settings::new(APP_ID),
            proxy: RefCell::default(),
            available: Cell::new(None),
            polling: Cell::new(false),
            detector: RefCell::default(),
            on_away: Box::new(on_away),
        });

        let weak = Rc::downgrade(&monitor);
        glib::spawn_future_local(async move {
            let proxy = gio::DBusProxy::for_bus_future(
                gio::BusType::Session,
                gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES
                    | gio::DBusProxyFlags::DO_NOT_CONNECT_SIGNALS
                    | gio::DBusProxyFlags::DO_NOT_AUTO_START,
                None,
                BUS_NAME,
                OBJECT_PATH,
                INTERFACE,
            )
            .await;
            if let Some(monitor) = weak.upgrade() {
                match proxy {
                    Ok(proxy) => {
                        monitor.proxy.replace(Some(proxy));
                    }
                    Err(err) => monitor.unavailable(&err),
                }
            }
        });

        let weak = Rc::downgrade(&monitor);
        glib::timeout_add_seconds_local(POLL_SECS, move || {
            let Some(monitor) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !monitor.polling.replace(true) {
                glib::spawn_future_local(async move {
                    monitor.poll().await;
                    monitor.polling.set(false);
                });
            }
            glib::ControlFlow::Continue
        });

        if let Some(minutes) = std::env::var("TIME_TRACKER_DEBUG_AWAY").ok().and_then(|m| m.parse::<i64>().ok()) {
            let weak = Rc::downgrade(&monitor);
            glib::timeout_add_seconds_local_once(2, move || {
                if let Some(monitor) = weak.upgrade() {
                    let now = format::now();
                    monitor.report(now - minutes * 60, now, 60);
                }
            });
        }
        monitor
    }

    /// Seconds without input before you count as away; `None` when off.
    fn threshold(&self) -> Option<i64> {
        self.settings
            .boolean("idle-detection")
            .then(|| i64::from(self.settings.int("idle-minutes").max(1)) * 60)
    }

    async fn poll(&self) {
        let running = TimeTrackerApplication::get().store().running_entry().ok().flatten().is_some();
        let Some(threshold) = self.threshold().filter(|_| running) else {
            self.detector.borrow_mut().reset();
            return;
        };
        let idle = self.idle_secs().await;
        let away = self.detector.borrow_mut().poll(format::now(), idle, threshold);
        if let Some((since, back)) = away {
            self.report(since, back, threshold);
        }
    }

    /// Seconds since the last input, when the idle monitor answers.
    async fn idle_secs(&self) -> Option<i64> {
        let proxy = self.proxy.borrow().clone()?;
        let reply = proxy
            .call_future("GetIdletime", None, gio::DBusCallFlags::NONE, 2000)
            .await;
        match reply {
            Ok(reply) => {
                self.available.set(Some(true));
                reply.get::<(u64,)>().map(|(ms,)| (ms / 1000) as i64)
            }
            Err(err) => {
                self.unavailable(&err);
                None
            }
        }
    }

    fn unavailable(&self, err: &glib::Error) {
        if self.available.replace(Some(false)).is_none() {
            glib::g_warning!("time-tracker", "Idle detection is not available: {err}");
        }
    }

    fn report(&self, since: i64, back: i64, threshold: i64) {
        let running = TimeTrackerApplication::get().store().running_entry().ok().flatten();
        let away = running.and_then(|entry| {
            away_for_entry(since, back, entry.start_at.unwrap_or(since), entry.id, threshold)
        });
        if let Some(away) = away {
            (self.on_away)(away);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn away_from_last_input_until_input_returns() {
        let mut detector = Detector::default();
        assert_eq!(detector.poll(10_595, Some(595), 600), None);
        assert_eq!(detector.poll(10_600, Some(600), 600), None);
        assert_eq!(detector.poll(10_605, Some(605), 600), None);
        // Input came back 3 seconds ago.
        assert_eq!(detector.poll(10_610, Some(3), 600), Some((10_000, 10_607)));
        assert_eq!(detector.poll(10_615, Some(8), 600), None);
    }

    #[test]
    fn short_breaks_are_not_away() {
        let mut detector = Detector::default();
        detector.poll(10_000, Some(300), 600);
        assert_eq!(detector.poll(10_005, Some(1), 600), None);
    }

    #[test]
    fn suspend_gap_counts_as_away_since_the_last_poll() {
        let mut detector = Detector::default();
        detector.poll(1_000, Some(10), 600);
        // The idle clock stood still during suspend.
        assert_eq!(detector.poll(5_000, Some(2), 600), Some((1_000, 4_998)));
    }

    #[test]
    fn suspend_counts_without_idle_times() {
        let mut detector = Detector::default();
        detector.poll(1_000, None, 600);
        assert_eq!(detector.poll(1_005, None, 600), None);
        assert_eq!(detector.poll(5_000, None, 600), Some((1_005, 5_000)));
    }

    #[test]
    fn earliest_away_moment_wins() {
        let mut detector = Detector::default();
        detector.poll(1_000, Some(500), 600);
        detector.poll(1_200, Some(700), 600);
        // Suspended while already away since 500.
        assert_eq!(detector.poll(9_000, Some(5), 600), Some((500, 8_995)));
    }

    #[test]
    fn away_is_clamped_to_the_entry_start() {
        assert_eq!(
            away_for_entry(1_000, 3_000, 1_500, 7, 600),
            Some(Away { since: 1_500, back: 3_000, entry_id: 7 })
        );
        // Started shortly before coming back: not worth asking.
        assert_eq!(away_for_entry(1_000, 3_000, 2_800, 7, 600), None);
    }
}
