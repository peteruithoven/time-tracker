//! Dialog to create a project, optionally together with a new client.

use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::glib::clone;

use crate::application::TimeTrackerApplication;
use crate::format;
use crate::store::Client;
use crate::widgets::{PROJECT_COLORS, color_label};

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/nl/peteruithoven/TimeTracker/new_project_dialog.ui")]
    pub struct NewProjectDialog {
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub create_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub name_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub client_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub new_client_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub color_box: TemplateChild<gtk::Box>,

        pub clients: RefCell<Vec<Client>>,
        pub color: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NewProjectDialog {
        const NAME: &'static str = "NewProjectDialog";
        type Type = super::NewProjectDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for NewProjectDialog {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // client id, project id
                    glib::subclass::Signal::builder("project-created")
                        .param_types([i64::static_type(), i64::static_type()])
                        .build(),
                ]
            })
        }
    }

    impl WidgetImpl for NewProjectDialog {}
    impl AdwDialogImpl for NewProjectDialog {}
}

glib::wrapper! {
    pub struct NewProjectDialog(ObjectSubclass<imp::NewProjectDialog>)
        @extends gtk::Widget, adw::Dialog,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl NewProjectDialog {
    pub fn new(client_id: Option<i64>) -> Self {
        let dialog: Self = glib::Object::new();
        dialog.setup(client_id);
        dialog
    }

    pub fn connect_project_created<F: Fn(&Self, i64, i64) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("project-created", false, move |args| {
            let dialog = args[0].get::<Self>().expect("Emitted by NewProjectDialog");
            f(&dialog, args[1].get().expect("Client id"), args[2].get().expect("Project id"));
            None
        })
    }

    fn setup(&self, client_id: Option<i64>) {
        let imp = self.imp();
        // Projects of clients connected to Fibery come from Fibery.
        let store = TimeTrackerApplication::get().store();
        let connected: Vec<i64> = store.accounts().unwrap_or_default().iter().map(|a| a.client_id).collect();
        let clients: Vec<Client> = store
            .clients()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| !connected.contains(&c.id))
            .collect();

        let names = gtk::StringList::new(&[]);
        for client in &clients {
            names.append(&client.name);
        }
        names.append(&gettext("New Client…"));
        imp.client_row.set_model(Some(&names));
        let selected = client_id
            .and_then(|id| clients.iter().position(|c| c.id == id))
            .unwrap_or(0);
        imp.client_row.set_selected(selected as u32);
        imp.clients.replace(clients);

        // Color swatches, defaulting to the first color.
        imp.color.replace(PROJECT_COLORS[0].to_string());
        let mut group: Option<gtk::ToggleButton> = None;
        for &color in PROJECT_COLORS {
            let swatch = gtk::ToggleButton::builder()
                .tooltip_text(color_label(color))
                .css_classes(["color-swatch", color])
                .active(group.is_none())
                .build();
            swatch.set_group(group.as_ref());
            swatch.connect_toggled(clone!(
                #[weak(rename_to = dialog)]
                self,
                move |swatch| {
                    if swatch.is_active() {
                        dialog.imp().color.replace(color.to_string());
                    }
                }
            ));
            imp.color_box.append(&swatch);
            group.get_or_insert(swatch);
        }

        imp.client_row.connect_selected_notify(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.update()
        ));
        for row in [&*imp.name_row, &*imp.new_client_row] {
            row.connect_changed(clone!(
                #[weak(rename_to = dialog)]
                self,
                move |_| dialog.update()
            ));
            row.connect_entry_activated(clone!(
                #[weak(rename_to = dialog)]
                self,
                move |_| dialog.create()
            ));
        }
        imp.cancel_button.connect_clicked(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| {
                dialog.close();
            }
        ));
        imp.create_button.connect_clicked(clone!(
            #[weak(rename_to = dialog)]
            self,
            move |_| dialog.create()
        ));
        self.set_focus(Some(&*imp.name_row));
        self.update();
    }

    fn is_new_client(&self) -> bool {
        let imp = self.imp();
        imp.client_row.selected() as usize >= imp.clients.borrow().len()
    }

    fn update(&self) {
        let imp = self.imp();
        let new_client = self.is_new_client();
        imp.new_client_row.set_visible(new_client);
        let valid = !imp.name_row.text().trim().is_empty()
            && (!new_client || !imp.new_client_row.text().trim().is_empty());
        imp.create_button.set_sensitive(valid);
    }

    fn create(&self) {
        let imp = self.imp();
        if !imp.create_button.is_sensitive() {
            return;
        }
        let app = TimeTrackerApplication::get();
        let store = app.store();
        let now = format::now();

        let client_id = if self.is_new_client() {
            store.create_client(&imp.new_client_row.text(), now)
        } else {
            Ok(imp.clients.borrow()[imp.client_row.selected() as usize].id)
        };
        let result = client_id.and_then(|client_id| {
            store
                .create_project(client_id, &imp.name_row.text(), &imp.color.borrow(), now)
                .map(|project_id| (client_id, project_id))
        });
        match result {
            Ok((client_id, project_id)) => {
                app.data_changed();
                self.emit_by_name::<()>("project-created", &[&client_id, &project_id]);
                self.close();
            }
            Err(err) => glib::g_critical!("time-tracker", "Could not create the project: {err}"),
        }
    }
}
