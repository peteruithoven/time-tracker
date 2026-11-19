//! Build-time configuration.
//!
//! Meson passes these as `TT_*` environment variables when it invokes cargo
//! (see `src/meson.build`). The fallbacks match a development Flatpak build,
//! so `cargo check` and rust-analyzer work without Meson.

macro_rules! config_value {
    ($name:literal, $default:literal) => {
        match option_env!($name) {
            Some(value) => value,
            None => $default,
        }
    };
}

pub const APP_ID: &str = config_value!("TT_APP_ID", "nl.peteruithoven.TimeTracker.Devel");
pub const PROFILE: &str = config_value!("TT_PROFILE", "Devel");
pub const VERSION: &str = config_value!("TT_VERSION", "0.1.0-devel");
pub const GETTEXT_PACKAGE: &str = config_value!("TT_GETTEXT_PACKAGE", "time-tracker");
pub const LOCALEDIR: &str = config_value!("TT_LOCALEDIR", "/app/share/locale");
pub const PKGDATADIR: &str = config_value!("TT_PKGDATADIR", "/app/share/time-tracker");

/// Resource path all UI files are registered under, independent of the
/// (possibly `.Devel`-suffixed) application ID.
pub const RESOURCE_BASE_PATH: &str = "/nl/peteruithoven/TimeTracker/";
