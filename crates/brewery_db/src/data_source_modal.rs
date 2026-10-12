//! The Add/Edit Data Source dialog: name, driver, URL (prefilled per driver), user and password,
//! Test Connection, and a section to register a JDBC driver from jar files.

use crate::panel::DatabasePanel;
use brewery_daemon_client::{
    AddDriverParams, ConnectionInfo, DriverInfo, ProjectDaemon, SaveConnectionParams,
    TestConnectionResult, daemon_error_message,
};
use gpui::{
    App, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString,
    Subscription, Task, WeakEntity, Window,
};
use std::collections::BTreeMap;
use ui::{
    Checkbox, ContextMenu, DropdownMenu, Modal, ModalFooter, ModalHeader, Section, ToggleState,
    prelude::*,
};
use ui_input::{ErasedEditorEvent, InputField};
use workspace::notifications::NotificationId;
use workspace::{ModalView, Toast, Workspace};

/// What a new data source of this driver most likely connects to. `None` for user drivers, whose
/// URL scheme Brewery does not know.
pub fn url_template(driver_id: &str, project_root: Option<&str>) -> Option<String> {
    match driver_id {
        "postgresql" => Some("jdbc:postgresql://localhost:5432/postgres".to_string()),
        "mysql" => Some("jdbc:mysql://localhost:3306/".to_string()),
        "mariadb" => Some("jdbc:mariadb://localhost:3306/".to_string()),
        "sqlite" => {
            let root = project_root.unwrap_or(".").replace('\\', "/");
            let root = root.trim_end_matches('/');
            Some(format!("jdbc:sqlite:{root}/db.sqlite"))
        }
        _ => None,
    }
}

/// The URL to show after switching drivers: a URL the user typed is kept, an empty one or the
/// previous driver's template is replaced by the new driver's template.
pub fn url_after_driver_change(
    current_url: &str,
    previous_driver: &str,
    next_driver: &str,
    project_root: Option<&str>,
) -> String {
    let current = current_url.trim();
    let untouched = current.is_empty()
        || url_template(previous_driver, project_root).is_some_and(|template| template == current);
    if untouched {
        url_template(next_driver, project_root).unwrap_or_default()
    } else {
        current_url.to_string()
    }
}

/// A typed password always wins; otherwise `Some("")` forgets the stored one and `None` keeps it
/// (or sets none for a new data source).
pub fn password_to_send(typed: &str, forget: bool) -> Option<String> {
    if !typed.is_empty() {
        Some(typed.to_string())
    } else if forget {
        Some(String::new())
    } else {
        None
    }
}

/// Jar paths separated by commas or newlines; blanks dropped.
pub fn parse_jar_paths(text: &str) -> Vec<String> {
    text.split([',', '\n'])
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect()
}

/// A driver id derived from the name the user gave the driver: lower case, `-` between words.
/// A name without ASCII letters or digits (e.g. only Cyrillic) gets `driver-<hash of the name>`;
/// an empty name gets an empty id.
pub fn driver_id_for_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return String::new();
    }
    let mut id = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            id.push(ch.to_ascii_lowercase());
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    let id = id.trim_end_matches('-');
    if !id.is_empty() {
        return id.to_string();
    }
    // FNV-1a: stable across runs, unlike the std hasher.
    let hash = name.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    format!("driver-{hash:08x}")
}

/// The text shown next to Test Connection for a daemon answer.
pub fn test_result_text(result: &TestConnectionResult) -> String {
    match (&result.server_version, result.ok) {
        (Some(version), true) => format!("{} ({version})", result.message),
        _ => result.message.clone(),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TestState {
    Idle,
    Testing,
    Succeeded(SharedString),
    Failed(SharedString),
}

pub(crate) struct WarningNotification;

pub struct DataSourceModal {
    workspace: WeakEntity<Workspace>,
    panel: WeakEntity<DatabasePanel>,
    daemon: Entity<ProjectDaemon>,
    project_root: Option<String>,
    /// The data source being edited; `None` adds a new one.
    editing: Option<ConnectionInfo>,
    drivers: Vec<DriverInfo>,
    driver_id: String,
    name: Entity<InputField>,
    url: Entity<InputField>,
    user: Entity<InputField>,
    password: Entity<InputField>,
    forget_password: bool,
    test_state: TestState,
    error: Option<SharedString>,
    saving: bool,
    show_add_driver: bool,
    new_driver_name: Entity<InputField>,
    new_driver_class: Entity<InputField>,
    new_driver_jars: Entity<InputField>,
    add_driver_error: Option<SharedString>,
    adding_driver: bool,
    /// Rebuilt only when the drivers or the choice change, so a re-render does not reset an open
    /// dropdown.
    driver_menu: Option<Entity<ContextMenu>>,
    _field_subscriptions: Vec<Subscription>,
    _drivers_task: Option<Task<()>>,
    test_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    add_driver_task: Option<Task<()>>,
}

impl DataSourceModal {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        panel: WeakEntity<DatabasePanel>,
        daemon: Entity<ProjectDaemon>,
        project_root: Option<String>,
        editing: Option<ConnectionInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = |label: &str,
                     placeholder: &str,
                     tab_index: isize,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            let label = label.to_string();
            let placeholder = placeholder.to_string();
            cx.new(|cx| {
                InputField::new(window, cx, &placeholder)
                    .label(label)
                    .label_size(LabelSize::Small)
                    .tab_index(tab_index)
            })
        };
        // Tab order: name, driver dropdown (2), URL, user, password, then the driver section.
        let name = field("Name", "My database", 1, window, cx);
        let url = field("URL", "jdbc:…", 3, window, cx);
        let user = field("User", "", 4, window, cx);
        let password = cx.new(|cx| {
            InputField::new(window, cx, "")
                .label("Password")
                .label_size(LabelSize::Small)
                .tab_index(5)
                .masked(true)
        });
        let new_driver_name = field("Driver name", "Oracle", 10, window, cx);
        let new_driver_class = field("Class name", "oracle.jdbc.OracleDriver", 11, window, cx);
        let new_driver_jars = field(
            "Jar files (comma separated)",
            "/path/to/ojdbc11.jar",
            12,
            window,
            cx,
        );

        let driver_id = match &editing {
            Some(connection) => {
                set_text(&name, &connection.name, window, cx);
                set_text(&url, &connection.url, window, cx);
                set_text(&user, connection.user.as_deref().unwrap_or(""), window, cx);
                connection.driver_id.clone()
            }
            None => {
                let driver_id = "postgresql".to_string();
                if let Some(template) = url_template(&driver_id, project_root.as_deref()) {
                    set_text(&url, &template, window, cx);
                }
                driver_id
            }
        };

        // A test result is about the values it was run with: editing them makes it stale. The
        // reset is deferred because `select_driver` edits the URL while the modal is updated.
        let field_subscriptions = [&url, &user, &password]
            .into_iter()
            .map(|field| {
                let modal = cx.weak_entity();
                let editor = field.read(cx).editor().clone();
                editor.subscribe(
                    Box::new(move |event, _, cx| {
                        if event == ErasedEditorEvent::BufferEdited {
                            let modal = modal.clone();
                            cx.defer(move |cx| {
                                modal
                                    .update(cx, |modal, cx| modal.forget_test_result(cx))
                                    .ok();
                            });
                        }
                    }),
                    window,
                    cx,
                )
            })
            .collect();

        let drivers = daemon.read(cx).db_drivers(cx);
        let drivers_task = cx.spawn(async move |this, cx| {
            let drivers = drivers.await;
            this.update(cx, |this, cx| {
                match drivers {
                    Ok(drivers) => {
                        this.drivers = drivers;
                        this.driver_menu = None;
                    }
                    Err(error) => this.error = Some(daemon_error_message(&error).into()),
                }
                cx.notify();
            })
            .ok();
        });

        Self {
            workspace,
            panel,
            daemon,
            project_root,
            editing,
            drivers: Vec::new(),
            driver_id,
            name,
            url,
            user,
            password,
            forget_password: false,
            test_state: TestState::Idle,
            error: None,
            saving: false,
            show_add_driver: false,
            new_driver_name,
            new_driver_class,
            new_driver_jars,
            add_driver_error: None,
            adding_driver: false,
            driver_menu: None,
            _field_subscriptions: field_subscriptions,
            _drivers_task: Some(drivers_task),
            test_task: None,
            save_task: None,
            add_driver_task: None,
        }
    }

    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    pub fn driver_id(&self) -> &str {
        &self.driver_id
    }

    pub fn test_state(&self) -> &TestState {
        &self.test_state
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn name_field(&self) -> &Entity<InputField> {
        &self.name
    }

    pub fn url_field(&self) -> &Entity<InputField> {
        &self.url
    }

    pub fn user_field(&self) -> &Entity<InputField> {
        &self.user
    }

    pub fn password_field(&self) -> &Entity<InputField> {
        &self.password
    }

    pub fn add_driver_error(&self) -> Option<&SharedString> {
        self.add_driver_error.as_ref()
    }

    /// The name, class name and jar fields of the "Add driver" section.
    pub fn new_driver_fields(&self) -> [&Entity<InputField>; 3] {
        [
            &self.new_driver_name,
            &self.new_driver_class,
            &self.new_driver_jars,
        ]
    }

    pub fn set_show_add_driver(&mut self, show: bool, cx: &mut Context<Self>) {
        self.show_add_driver = show;
        cx.notify();
    }

    pub fn set_forget_password(&mut self, forget: bool, cx: &mut Context<Self>) {
        self.forget_password = forget;
        self.forget_test_result(cx);
    }

    /// Drops a shown or pending test result once the values it was run with changed.
    fn forget_test_result(&mut self, cx: &mut Context<Self>) {
        self.test_task = None;
        self.test_state = TestState::Idle;
        cx.notify();
    }

    pub fn select_driver(
        &mut self,
        driver_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.url.read(cx).text(cx);
        let next = url_after_driver_change(
            &current,
            &self.driver_id,
            &driver_id,
            self.project_root.as_deref(),
        );
        if next != current {
            set_text(&self.url, &next, window, cx);
        }
        self.driver_id = driver_id;
        self.driver_menu = None;
        self.forget_test_result(cx);
    }

    fn params(&self, cx: &App) -> SaveConnectionParams {
        let text = |field: &Entity<InputField>| field.read(cx).text(cx).trim().to_string();
        let user = text(&self.user);
        let editing = self.editing.as_ref();
        SaveConnectionParams {
            connection: ConnectionInfo {
                id: editing.map(|c| c.id.clone()).unwrap_or_default(),
                name: text(&self.name),
                driver_id: self.driver_id.clone(),
                url: text(&self.url),
                user: (!user.is_empty()).then_some(user),
                has_password: editing.is_some_and(|c| c.has_password),
                options: editing
                    .map(|c| c.options.clone())
                    .unwrap_or_else(BTreeMap::new),
                warning: None,
            },
            // A password may legitimately start or end with spaces, so it is not trimmed.
            password: password_to_send(&self.password.read(cx).text(cx), self.forget_password),
        }
    }

    /// Saving needs a name; testing only what a connection needs.
    fn validate(&mut self, params: &SaveConnectionParams, needs_name: bool) -> bool {
        let missing = if needs_name && params.connection.name.is_empty() {
            Some("Enter a name")
        } else if params.connection.url.is_empty() {
            Some("Enter a JDBC URL")
        } else if params.connection.driver_id.is_empty() {
            Some("Choose a driver")
        } else {
            None
        };
        self.error = missing.map(SharedString::from);
        missing.is_none()
    }

    pub fn test_connection(&mut self, cx: &mut Context<Self>) {
        let params = self.params(cx);
        if !self.validate(&params, false) {
            cx.notify();
            return;
        }
        self.test_state = TestState::Testing;
        let test = self.daemon.read(cx).db_test_connection(params, cx);
        self.test_task = Some(cx.spawn(async move |this, cx| {
            let result = test.await;
            this.update(cx, |this, cx| {
                this.test_state = match result {
                    Ok(result) if result.ok => {
                        TestState::Succeeded(test_result_text(&result).into())
                    }
                    Ok(result) => TestState::Failed(test_result_text(&result).into()),
                    Err(error) => TestState::Failed(daemon_error_message(&error).into()),
                };
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let params = self.params(cx);
        if !self.validate(&params, true) {
            cx.notify();
            return;
        }
        self.saving = true;
        let save = self.daemon.read(cx).db_save_connection(params, cx);
        self.save_task = Some(cx.spawn(async move |this, cx| {
            let result = save.await;
            this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(saved) => {
                        if let Some(warning) = saved.warning.clone() {
                            this.show_warning(warning, cx);
                        }
                        this.panel
                            .update(cx, |panel, cx| panel.data_source_saved(saved, cx))
                            .ok();
                        cx.emit(DismissEvent);
                    }
                    Err(error) => this.error = Some(daemon_error_message(&error).into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn show_warning(&self, warning: String, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        // The toast lands in the workspace, which must not be in the middle of an update.
        cx.defer(move |cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace.show_toast(
                        Toast::new(NotificationId::unique::<WarningNotification>(), warning),
                        cx,
                    );
                })
                .ok();
        });
    }

    pub fn add_driver(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_driver_name.read(cx).text(cx).trim().to_string();
        let class_name = self.new_driver_class.read(cx).text(cx).trim().to_string();
        let jar_paths = parse_jar_paths(&self.new_driver_jars.read(cx).text(cx));
        let driver_id = driver_id_for_name(&name);
        let missing = if driver_id.is_empty() {
            Some("Enter a driver name")
        } else if class_name.is_empty() {
            Some("Enter the driver class name")
        } else if jar_paths.is_empty() {
            Some("Enter at least one jar file")
        } else {
            None
        };
        if let Some(missing) = missing {
            self.add_driver_error = Some(missing.into());
            cx.notify();
            return;
        }
        self.adding_driver = true;
        self.add_driver_error = None;
        let add = self.daemon.read(cx).db_add_driver(
            AddDriverParams {
                driver_id,
                name,
                class_name,
                jar_paths,
            },
            cx,
        );
        self.add_driver_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = add.await;
            this.update_in(cx, |this, window, cx| {
                this.adding_driver = false;
                match result {
                    Ok(driver) => {
                        let id = driver.id.clone();
                        this.drivers.retain(|other| other.id != id);
                        this.drivers.push(driver);
                        this.driver_menu = None;
                        this.show_add_driver = false;
                        this.select_driver(id, window, cx);
                    }
                    Err(error) => this.add_driver_error = Some(daemon_error_message(&error).into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    /// Enter in the "Add driver" fields adds the driver; anywhere else it saves.
    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let in_driver_section = self.show_add_driver
            && self
                .new_driver_fields()
                .iter()
                .any(|field| field.focus_handle(cx).contains_focused(window, cx));
        if in_driver_section {
            self.add_driver(window, cx);
        } else {
            self.save(cx);
        }
    }

    fn driver_label(&self) -> SharedString {
        self.drivers
            .iter()
            .find(|driver| driver.id == self.driver_id)
            .map(|driver| driver.name.clone())
            .unwrap_or_else(|| self.driver_id.clone())
            .into()
    }

    fn render_driver_dropdown(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DropdownMenu {
        let modal = cx.weak_entity();
        let drivers = self.drivers.clone();
        let current = self.driver_id.clone();
        if let Some(menu) = self.driver_menu.clone() {
            return DropdownMenu::new("data-source-driver", self.driver_label(), menu)
                .full_width(true)
                .tab_index(2isize);
        }
        let menu = ContextMenu::build(window, cx, move |mut menu, _, _| {
            for driver in drivers {
                let modal = modal.clone();
                let id = driver.id.clone();
                let label = if driver.bundled {
                    driver.name.clone()
                } else {
                    format!("{} (user driver)", driver.name)
                };
                menu = menu.toggleable_entry(
                    label,
                    driver.id == current,
                    IconPosition::Start,
                    None,
                    move |window, cx| {
                        modal
                            .update(cx, |modal, cx| modal.select_driver(id.clone(), window, cx))
                            .ok();
                    },
                );
            }
            menu
        });
        self.driver_menu = Some(menu.clone());
        DropdownMenu::new("data-source-driver", self.driver_label(), menu)
            .full_width(true)
            .tab_index(2isize)
    }

    fn render_password(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let stored = self.editing.as_ref().is_some_and(|c| c.has_password);
        v_flex()
            .gap_1()
            .child(self.password.clone())
            .when(stored, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .justify_between()
                        .child(
                            Label::new("Stored; leave empty to keep it")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .child(
                            Checkbox::new(
                                "data-source-forget-password",
                                ToggleState::from(self.forget_password),
                            )
                            .label("Forget password")
                            .on_click(cx.listener(
                                |this, state: &ToggleState, _, cx| {
                                    this.set_forget_password(state.selected(), cx)
                                },
                            )),
                        ),
                )
            })
    }

    fn render_add_driver(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.show_add_driver {
            return v_flex().child(
                Button::new("data-source-show-add-driver", "Add driver…")
                    .style(ButtonStyle::Subtle)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_add_driver = true;
                        cx.notify();
                    })),
            );
        }
        v_flex()
            .gap_2()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().colors().border_variant)
            .child(Label::new("Add driver").size(LabelSize::Small))
            .child(self.new_driver_name.clone())
            .child(self.new_driver_class.clone())
            .child(self.new_driver_jars.clone())
            .children(
                self.add_driver_error
                    .clone()
                    .map(|error| Label::new(error).size(LabelSize::Small).color(Color::Error)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("data-source-hide-add-driver", "Cancel").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.show_add_driver = false;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        Button::new("data-source-add-driver", "Add Driver")
                            .style(ButtonStyle::Filled)
                            .disabled(self.adding_driver)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_driver(window, cx)),
                            ),
                    ),
            )
    }

    fn render_test_state(&self) -> Option<Label> {
        match &self.test_state {
            TestState::Idle => None,
            TestState::Testing => Some(Label::new("Testing…").color(Color::Muted)),
            TestState::Succeeded(text) => Some(Label::new(text.clone()).color(Color::Success)),
            TestState::Failed(text) => Some(Label::new(text.clone()).color(Color::Error)),
        }
        .map(|label| label.size(LabelSize::Small))
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> ModalFooter {
        ModalFooter::new()
            // The result goes on its own line above the footer: a long driver message here
            // would push Cancel and Save out of the dialog.
            .start_slot(
                Button::new("data-source-test", "Test Connection")
                    .disabled(self.test_state == TestState::Testing)
                    .on_click(cx.listener(|this, _, _, cx| this.test_connection(cx))),
            )
            .end_slot(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("data-source-cancel", "Cancel")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    )
                    .child(
                        Button::new("data-source-save", "Save")
                            .style(ButtonStyle::Filled)
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
    }
}

fn set_text(field: &Entity<InputField>, text: &str, window: &mut Window, cx: &mut App) {
    field.update(cx, |field, cx| field.set_text(text, window, cx));
}

impl EventEmitter<DismissEvent> for DataSourceModal {}

impl ModalView for DataSourceModal {}

impl Focusable for DataSourceModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.name.focus_handle(cx)
    }
}

impl Render for DataSourceModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let headline = if self.is_editing() {
            "Edit Data Source"
        } else {
            "New Data Source"
        };
        let driver = v_flex()
            .gap_1()
            .child(Label::new("Driver").size(LabelSize::Small))
            .child(self.render_driver_dropdown(window, cx));
        let form = v_flex()
            .gap_2()
            .child(self.name.clone())
            .child(driver)
            .child(self.url.clone())
            .child(self.user.clone())
            .child(self.render_password(cx))
            .children(
                self.render_test_state()
                    .map(|label| div().w_full().child(label)),
            )
            .children(
                self.error
                    .clone()
                    .map(|error| Label::new(error).size(LabelSize::Small).color(Color::Error)),
            )
            .child(self.render_add_driver(cx));
        div()
            .elevation_3(cx)
            .w(rems(34.))
            .key_context("DataSourceModal")
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            .child(
                Modal::new("data-source-modal", None)
                    .header(ModalHeader::new().headline(headline))
                    .section(Section::new().child(form))
                    .footer(self.render_footer(cx)),
            )
    }
}

#[cfg(test)]
#[path = "data_source_modal_tests.rs"]
mod tests;
