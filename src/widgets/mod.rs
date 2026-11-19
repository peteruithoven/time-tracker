mod clients_dialog;
mod connect_dialog;
mod entry_dialog;
mod month_picker;
mod new_project_dialog;
mod project_picker;
mod review_page;
mod task_picker;
mod track_page;

pub use clients_dialog::ClientsDialog;
pub use connect_dialog::ConnectDialog;
pub use entry_dialog::EntryDialog;
pub use month_picker::MonthPicker;
pub use new_project_dialog::NewProjectDialog;
pub use project_picker::ProjectPicker;
pub use review_page::ReviewPage;
pub use task_picker::TaskPicker;
pub use track_page::{TrackPage, set_source_badge};

use gettextrs::gettext;
use gtk::prelude::*;

/// Project colors offered when creating a project. Each name is also a CSS
/// class (see `style.css`).
pub const PROJECT_COLORS: &[&str] = &[
    "blue", "green", "yellow", "orange", "red", "purple", "brown", "slate",
];

/// A small colored circle for a project; tasks without a project get slate.
pub fn project_color_dot(color: Option<&str>) -> gtk::Box {
    let dot = gtk::Box::builder()
        .valign(gtk::Align::Center)
        .css_classes(["project-dot", color.unwrap_or("slate")])
        .build();
    dot.set_accessible_role(gtk::AccessibleRole::Presentation);
    dot
}

/// Translated name of a project color.
pub fn color_label(color: &str) -> String {
    match color {
        "blue" => gettext("Blue"),
        "green" => gettext("Green"),
        "yellow" => gettext("Yellow"),
        "orange" => gettext("Orange"),
        "red" => gettext("Red"),
        "purple" => gettext("Purple"),
        "brown" => gettext("Brown"),
        _ => gettext("Slate"),
    }
}
