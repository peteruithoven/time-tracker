//! Screenshot mode for the AppStream screenshots (see
//! `build-aux/screenshots.sh`). Controlled by environment variables:
//!
//! - `TIME_TRACKER_SCREENSHOT`: PNG to write; the app quits afterwards
//! - `TIME_TRACKER_SCREENSHOT_SIZE`: window size, e.g. `400x640`
//! - `TIME_TRACKER_SCREENSHOT_VIEW`: `track` or `review`
//! - `TIME_TRACKER_DEMO`: fill an empty database with demo data

use gtk::prelude::*;
use gtk::{glib, graphene, gsk};

/// Rendered at twice the size, for crisp screenshots.
const SCALE: f32 = 2.0;

pub fn output() -> Option<String> {
    std::env::var("TIME_TRACKER_SCREENSHOT").ok().filter(|s| !s.is_empty())
}

pub fn enabled() -> bool {
    output().is_some()
}

pub fn size() -> Option<(i32, i32)> {
    let value = std::env::var("TIME_TRACKER_SCREENSHOT_SIZE").ok()?;
    let (w, h) = value.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

pub fn view() -> Option<String> {
    std::env::var("TIME_TRACKER_SCREENSHOT_VIEW").ok()
}

pub fn demo() -> bool {
    std::env::var_os("TIME_TRACKER_DEMO").is_some()
}

/// Sets up GNOME's defaults (no host settings are available headless).
pub fn configure() {
    if !enabled() {
        return;
    }
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_decoration_layout(Some(":close"));
        settings.set_gtk_font_name(Some("Adwaita Sans 11"));
    }
}

/// Waits until the window is drawn, writes the PNG and quits. Retries for
/// a while, as a cold start can take a few seconds.
pub fn take(window: &gtk::Window, app: &gtk::Application) {
    let Some(path) = output() else {
        return;
    };
    let (window, app) = (window.clone(), app.clone());
    let mut attempts = 0;
    // Give animations and the first data load time to finish.
    glib::timeout_add_local(std::time::Duration::from_millis(1500), move || {
        attempts += 1;
        let ready = window.is_mapped() && window.width() > 0;
        let result = if ready { render(&window, &path) } else { Err("The window isn't shown yet".into()) };
        match result {
            Ok(()) => println!("Saved {path}"),
            Err(_) if attempts < 10 => return glib::ControlFlow::Continue,
            Err(err) => eprintln!("Could not save {path}: {err}"),
        }
        app.quit();
        glib::ControlFlow::Break
    });
}

fn render(window: &gtk::Window, path: &str) -> Result<(), String> {
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let width = paintable.intrinsic_width().max(window.width()) as f32;
    let height = paintable.intrinsic_height().max(window.height()) as f32;

    let snapshot = gtk::Snapshot::new();
    snapshot.scale(SCALE, SCALE);
    paintable.snapshot(&snapshot, width as f64, height as f64);
    let node = snapshot.to_node().ok_or("Nothing was rendered")?;

    // A software renderer works on any display, including Broadway.
    let renderer = gsk::CairoRenderer::new();
    renderer
        .realize_for_display(&gtk::prelude::WidgetExt::display(window))
        .map_err(|e| e.to_string())?;
    let bounds = graphene::Rect::new(0.0, 0.0, width * SCALE, height * SCALE);
    let texture = renderer.render_texture(&node, Some(&bounds));
    renderer.unrealize();
    texture.save_to_png(path).map_err(|e| e.to_string())
}
