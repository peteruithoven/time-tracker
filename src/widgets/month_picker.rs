//! Popover with a year of months and the hours tracked in each.

use std::cell::Cell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::glib::clone;

use crate::application::TimeTrackerApplication;
use crate::format;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct MonthPicker {
        pub year_label: gtk::Label,
        pub grid: gtk::Grid,
        /// Year shown in the grid.
        pub year: Cell<i32>,
        /// Selected month.
        pub selected: Cell<(i32, u32)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MonthPicker {
        const NAME: &'static str = "MonthPicker";
        type Type = super::MonthPicker;
        type ParentType = gtk::Popover;
    }

    impl ObjectImpl for MonthPicker {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // year, month
                    glib::subclass::Signal::builder("month-chosen")
                        .param_types([i32::static_type(), u32::static_type()])
                        .build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    impl WidgetImpl for MonthPicker {}
    impl PopoverImpl for MonthPicker {}
}

glib::wrapper! {
    pub struct MonthPicker(ObjectSubclass<imp::MonthPicker>)
        @extends gtk::Widget, gtk::Popover,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget,
                    gtk::Native, gtk::ShortcutManager;
}

impl Default for MonthPicker {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl MonthPicker {
    pub fn set_selected(&self, year: i32, month: u32) {
        self.imp().selected.set((year, month));
    }

    pub fn connect_month_chosen<F: Fn(&Self, i32, u32) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("month-chosen", false, move |args| {
            let picker = args[0].get::<Self>().expect("Emitted by MonthPicker");
            f(&picker, args[1].get().expect("Year"), args[2].get().expect("Month"));
            None
        })
    }

    fn setup(&self) {
        let imp = self.imp();
        let previous = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text(gettext("Previous Year"))
            .css_classes(["flat"])
            .build();
        let next = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .tooltip_text(gettext("Next Year"))
            .css_classes(["flat"])
            .build();
        previous.connect_clicked(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| picker.show_year(picker.imp().year.get() - 1)
        ));
        next.connect_clicked(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| picker.show_year(picker.imp().year.get() + 1)
        ));
        imp.year_label.add_css_class("heading");
        imp.year_label.set_hexpand(true);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.append(&previous);
        header.append(&imp.year_label);
        header.append(&next);

        imp.grid.set_row_spacing(6);
        imp.grid.set_column_spacing(6);
        imp.grid.set_column_homogeneous(true);

        let this_month = gtk::Button::with_label(&gettext("This Month"));
        this_month.connect_clicked(clone!(
            #[weak(rename_to = picker)]
            self,
            move |_| {
                if let Some((y, m, _)) = format::date_parts(&format::today()) {
                    picker.choose(y, m);
                }
            }
        ));

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .width_request(280)
            .build();
        content.append(&header);
        content.append(&imp.grid);
        content.append(&this_month);
        self.set_child(Some(&content));

        self.connect_show(|picker| picker.show_year(picker.imp().selected.get().0));
    }

    fn show_year(&self, year: i32) {
        let imp = self.imp();
        imp.year.set(year);
        imp.year_label.set_label(&year.to_string());
        while let Some(child) = imp.grid.first_child() {
            imp.grid.remove(&child);
        }

        let now = format::now();
        let totals = TimeTrackerApplication::get()
            .store()
            .month_totals(year, now)
            .unwrap_or_default();
        let (this_year, this_month, _) = format::date_parts(&format::today()).unwrap_or((year, 12, 1));
        let selected = imp.selected.get();

        for month in 1..=12u32 {
            let hours = totals[month as usize - 1];
            let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
            content.append(&gtk::Label::new(Some(&format::short_month(year, month))));
            let hours_label = gtk::Label::new(Some(&if hours > 0 {
                gettext("{}h").replace("{}", &(hours / 3600).to_string())
            } else {
                "—".to_string()
            }));
            hours_label.add_css_class("caption");
            hours_label.add_css_class("numeric");
            content.append(&hours_label);

            let button = gtk::Button::builder()
                .child(&content)
                .css_classes(["flat", "month-button"])
                .sensitive((year, month) <= (this_year, this_month))
                .build();
            if (year, month) == selected {
                button.add_css_class("suggested-action");
                button.remove_css_class("flat");
            }
            button.connect_clicked(clone!(
                #[weak(rename_to = picker)]
                self,
                move |_| picker.choose(year, month)
            ));
            let index = month as i32 - 1;
            imp.grid.attach(&button, index % 3, index / 3, 1, 1);
        }
    }

    fn choose(&self, year: i32, month: u32) {
        self.set_selected(year, month);
        self.popdown();
        self.emit_by_name::<()>("month-chosen", &[&year, &month]);
    }
}
