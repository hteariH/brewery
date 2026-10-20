use super::*;
use crate::data_source_modal::{TestState, WarningNotification};
use crate::test_support::*;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::{Value, json};
use settings::{DEFAULT_KEYMAP_PATH, KeymapFile};
use util::path;
use workspace::notifications::NotificationId;

fn rendered_rows(panel: &Entity<DatabasePanel>, cx: &mut VisualTestContext) -> usize {
    panel.update_in(cx, |panel, _, cx| panel.render_rows(cx).len())
}

fn select_source(fixture: &Fixture, id: &str, cx: &mut VisualTestContext) {
    fixture.panel.update(cx, |panel, cx| {
        panel.select(Some(RowKey::data_source(id)), cx)
    });
}

fn set_field(field: &Entity<ui_input::InputField>, text: &str, cx: &mut VisualTestContext) {
    field.update_in(cx, |field, window, cx| field.set_text(text, window, cx));
}

#[gpui::test]
async fn the_panel_lists_the_data_sources_in_the_left_dock(cx: &mut TestAppContext) {
    let (fixture, cx) = workspace_with_panel(cx).await;
    assert_eq!(DatabasePanel::persistent_name(), "Database Panel");
    assert!(
        fixture.state.calls.borrow().is_empty(),
        "no daemon before the panel is shown"
    );

    cx.dispatch_action(ToggleDatabasePanel);
    cx.run_until_parked();

    fixture.workspace.read_with(cx, |workspace, cx| {
        let dock = workspace.left_dock().read(cx);
        assert!(dock.is_open(), "the left dock should be open");
        let visible = dock.visible_panel().expect("a panel should be visible");
        assert_eq!(visible.panel_id(), fixture.panel.entity_id());
    });
    assert_eq!(fixture.calls_of("db/connections/list").len(), 1);
    assert_eq!(fixture.source_names(cx), ["Local Postgres", "Reports"]);
    assert_eq!(rendered_rows(&fixture.panel, cx), 2);
    assert_eq!(driver_label("postgresql"), "PostgreSQL");
}

#[gpui::test]
async fn expanding_lists_the_schema_lazily(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    assert!(fixture.calls_of("db/introspect").is_empty());

    fixture
        .panel
        .update(cx, |panel, cx| panel.toggle(RowKey::data_source("c1"), cx));
    cx.run_until_parked();
    fixture.panel.update(cx, |panel, cx| {
        panel.toggle(RowKey::new("c1", vec!["public".to_string()]), cx)
    });
    cx.run_until_parked();

    assert_eq!(
        fixture.calls_of("db/introspect"),
        [
            json!({ "connectionId": "c1", "path": [] }),
            json!({ "connectionId": "c1", "path": ["public"] }),
        ]
    );
    let names = fixture.panel.read_with(cx, |panel, _| {
        panel
            .tree()
            .rows()
            .iter()
            .map(|row| match row {
                Row::DataSource { connection, .. } => connection.name.clone(),
                Row::Node { node, depth, .. } => format!("{}{}", "  ".repeat(*depth), node.name),
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        names,
        [
            "Local Postgres",
            "  public",
            "    owners",
            "    owners_id_seq",
            "Reports"
        ]
    );
    assert_eq!(rendered_rows(&fixture.panel, cx), 5);

    // Collapsing and expanding again is served from the cache.
    fixture.panel.update(cx, |panel, cx| {
        panel.toggle(RowKey::data_source("c1"), cx);
        panel.toggle(RowKey::data_source("c1"), cx);
    });
    cx.run_until_parked();
    assert_eq!(fixture.calls_of("db/introspect").len(), 2);

    // Refresh drops the cache of the selected data source and lists what is open again.
    select_source(&fixture, "c1", cx);
    fixture.panel.update(cx, |panel, cx| panel.refresh(cx));
    cx.run_until_parked();
    assert_eq!(fixture.calls_of("db/introspect").len(), 4);
    assert_eq!(fixture.calls_of("db/connections/list").len(), 2);
    assert_eq!(rendered_rows(&fixture.panel, cx), 5);
}

#[gpui::test]
async fn a_failed_introspection_shows_an_error_row_and_badge(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture
        .panel
        .update(cx, |panel, cx| panel.toggle(RowKey::data_source("c2"), cx));
    cx.run_until_parked();
    fixture.panel.read_with(cx, |panel, _| {
        assert_eq!(
            panel.tree().root_error("c2").as_deref(),
            Some("connection refused")
        );
        assert!(panel.tree().rows().iter().any(|row| matches!(
            row,
            Row::Error { message, .. } if message == "connection refused"
        )));
    });
    assert_eq!(rendered_rows(&fixture.panel, cx), 3);
}

#[gpui::test]
async fn add_opens_the_dialog_and_save_stores_and_relists(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.add_data_source(window, cx));
    cx.run_until_parked();
    let modal = fixture.modal(cx).expect("the dialog should be open");
    assert_eq!(fixture.calls_of("db/drivers").len(), 1);

    modal.read_with(cx, |modal, cx| {
        assert!(!modal.is_editing());
        assert_eq!(modal.driver_id(), "postgresql");
        assert_eq!(
            modal.url_field().read(cx).text(cx),
            "jdbc:postgresql://localhost:5432/postgres"
        );
    });
    modal.update_in(cx, |modal, window, cx| {
        modal.select_driver("sqlite".to_string(), window, cx)
    });
    modal.read_with(cx, |modal, cx| {
        assert_eq!(
            modal.url_field().read(cx).text(cx),
            format!(
                "jdbc:sqlite:{}/db.sqlite",
                path!("/root").replace('\\', "/")
            ),
            "the untouched template follows the driver"
        );
    });
    modal.update_in(cx, |modal, window, cx| {
        modal.select_driver("postgresql".to_string(), window, cx)
    });

    let (name, url, user, password) = modal.read_with(cx, |modal, _| {
        (
            modal.name_field().clone(),
            modal.url_field().clone(),
            modal.user_field().clone(),
            modal.password_field().clone(),
        )
    });
    set_field(&name, "Billing", cx);
    set_field(&url, "jdbc:postgresql://db.internal:5432/billing", cx);
    set_field(&user, "billing", cx);
    set_field(&password, "s3cret", cx);

    modal.update(cx, |modal, cx| modal.test_connection(cx));
    cx.run_until_parked();
    modal.read_with(cx, |modal, _| {
        assert_eq!(
            modal.test_state(),
            &TestState::Succeeded("Connected (PostgreSQL 16.4)".into())
        );
    });

    modal.update(cx, |modal, cx| modal.save(cx));
    cx.run_until_parked();

    let expected_connection = json!({
        "id": "",
        "name": "Billing",
        "driverId": "postgresql",
        "url": "jdbc:postgresql://db.internal:5432/billing",
        "user": "billing",
        "hasPassword": false,
        "options": {}
    });
    assert_eq!(
        fixture.calls_of("db/connections/test"),
        [json!({ "connection": expected_connection, "password": "s3cret" })]
    );
    assert_eq!(
        fixture.calls_of("db/connections/save"),
        [json!({ "connection": expected_connection, "password": "s3cret" })]
    );
    assert_eq!(fixture.calls_of("db/connections/list").len(), 2);
    assert_eq!(
        fixture.source_names(cx),
        ["Local Postgres", "Reports", "Billing"]
    );
    fixture.panel.read_with(cx, |panel, _| {
        assert_eq!(panel.selected(), Some(&RowKey::data_source("c3")));
    });
    assert!(
        fixture.modal(cx).is_none(),
        "the dialog closes after saving"
    );
}

#[gpui::test]
async fn editing_keeps_the_id_and_can_forget_the_password(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    select_source(&fixture, "c1", cx);
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.edit_selected(window, cx));
    cx.run_until_parked();
    let modal = fixture.modal(cx).expect("the dialog should be open");
    modal.read_with(cx, |modal, cx| {
        assert!(modal.is_editing());
        assert_eq!(modal.name_field().read(cx).text(cx), "Local Postgres");
        assert_eq!(modal.user_field().read(cx).text(cx), "app");
        assert_eq!(modal.password_field().read(cx).text(cx), "", "never shown");
    });

    // Empty password keeps the stored one.
    modal.update(cx, |modal, cx| modal.save(cx));
    cx.run_until_parked();
    let saves = fixture.calls_of("db/connections/save");
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0]["connection"]["id"], "c1");
    assert_eq!(saves[0]["password"], Value::Null);

    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.edit_selected(window, cx));
    cx.run_until_parked();
    let modal = fixture.modal(cx).expect("the dialog should be open again");
    modal.update(cx, |modal, cx| {
        modal.set_forget_password(true, cx);
        modal.save(cx);
    });
    cx.run_until_parked();
    let saves = fixture.calls_of("db/connections/save");
    assert_eq!(saves.len(), 2);
    assert_eq!(saves[1]["password"], "");
}

#[gpui::test]
async fn save_without_a_name_is_refused_in_the_dialog(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.add_data_source(window, cx));
    cx.run_until_parked();
    let modal = fixture.modal(cx).expect("the dialog should be open");
    modal.update(cx, |modal, cx| modal.save(cx));
    cx.run_until_parked();
    assert!(fixture.calls_of("db/connections/save").is_empty());
    modal.read_with(cx, |modal, _| {
        assert_eq!(modal.error().map(|e| e.as_ref()), Some("Enter a name"));
    });
    assert!(fixture.modal(cx).is_some());
}

#[gpui::test]
async fn delete_asks_first_then_removes_and_relists(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    select_source(&fixture, "c2", cx);
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.delete_selected(window, cx));
    cx.run_until_parked();
    assert!(cx.has_pending_prompt(), "deleting asks for confirmation");
    assert!(fixture.calls_of("db/connections/delete").is_empty());

    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();

    assert_eq!(
        fixture.calls_of("db/connections/delete"),
        [json!({ "id": "c2" })]
    );
    assert_eq!(fixture.calls_of("db/connections/list").len(), 2);
    assert_eq!(fixture.source_names(cx), ["Local Postgres"]);
    fixture.panel.read_with(cx, |panel, _| {
        assert_eq!(panel.selected(), None);
    });
}

#[gpui::test]
async fn cancelling_the_delete_prompt_keeps_the_data_source(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    select_source(&fixture, "c2", cx);
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.delete_selected(window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(fixture.calls_of("db/connections/delete").is_empty());
    assert_eq!(fixture.source_names(cx), ["Local Postgres", "Reports"]);
}

fn load_default_keymap(cx: &mut VisualTestContext) {
    cx.update(|_, cx| {
        // Other crates' actions are not linked into this test binary, so their bindings fail to
        // load; `menu::Confirm`/`menu::Cancel` and the editor's are what matter here.
        cx.bind_keys(
            KeymapFile::load_asset_allow_partial_failure(DEFAULT_KEYMAP_PATH, cx)
                .expect("the default keymap should load"),
        );
    });
}

#[gpui::test]
async fn expanding_shows_loading_until_the_reply_arrives(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture.state.hold("db/introspect");
    fixture
        .panel
        .update(cx, |panel, cx| panel.toggle(RowKey::data_source("c1"), cx));
    cx.run_until_parked();
    assert!(fixture.panel_has_loading_row(cx));
    assert_eq!(
        rendered_rows(&fixture.panel, cx),
        3,
        "source, loading, source"
    );

    fixture.state.release("db/introspect");
    cx.run_until_parked();
    assert!(!fixture.panel_has_loading_row(cx));
    assert_eq!(
        rendered_rows(&fixture.panel, cx),
        3,
        "source, public, source"
    );
}

#[gpui::test]
async fn a_reply_from_before_a_refresh_does_not_fill_the_tree(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture.state.hold("db/introspect");
    fixture
        .panel
        .update(cx, |panel, cx| panel.toggle(RowKey::data_source("c1"), cx));
    cx.run_until_parked();
    select_source(&fixture, "c1", cx);
    fixture.panel.update(cx, |panel, cx| panel.refresh(cx));
    cx.run_until_parked();
    assert_eq!(fixture.state.held_count("db/introspect"), 2);

    fixture.state.release("db/introspect");
    cx.run_until_parked();
    assert!(
        fixture.panel_has_loading_row(cx),
        "the first reply belongs to the request before the refresh"
    );
    fixture.state.release("db/introspect");
    cx.run_until_parked();
    assert!(!fixture.panel_has_loading_row(cx));
}

#[gpui::test]
async fn refresh_without_a_selection_relists_every_open_data_source(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture.panel.update(cx, |panel, cx| {
        panel.toggle(RowKey::data_source("c1"), cx);
        panel.toggle(RowKey::data_source("c2"), cx);
    });
    cx.run_until_parked();
    assert_eq!(fixture.calls_of("db/introspect").len(), 2);
    fixture.panel.update(cx, |panel, cx| {
        panel.select(None, cx);
        panel.refresh(cx);
    });
    cx.run_until_parked();
    assert_eq!(fixture.calls_of("db/introspect").len(), 4);
}

#[gpui::test]
async fn a_save_warning_is_shown_as_a_toast(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    let modal = fixture.open_add_dialog(cx);
    let name = modal.read_with(cx, |modal, _| modal.name_field().clone());
    set_field(&name, "Billing", cx);
    modal.update(cx, |modal, cx| modal.save(cx));
    cx.run_until_parked();

    assert_eq!(fixture.calls_of("db/connections/save").len(), 1);
    fixture.workspace.read_with(cx, |workspace, _| {
        assert!(
            workspace
                .notification_ids()
                .contains(&NotificationId::unique::<WarningNotification>()),
            "the daemon's warning should be shown"
        );
    });
}

#[gpui::test]
async fn a_failed_save_keeps_the_dialog_open_with_the_error(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture
        .state
        .fail("db/connections/save", "could not write .brewery/db.json");
    let modal = fixture.open_add_dialog(cx);
    let name = modal.read_with(cx, |modal, _| modal.name_field().clone());
    set_field(&name, "Billing", cx);
    modal.update(cx, |modal, cx| modal.save(cx));
    cx.run_until_parked();

    assert!(fixture.modal(cx).is_some(), "the dialog stays open");
    modal.read_with(cx, |modal, _| {
        assert_eq!(
            modal.error().map(|error| error.as_ref()),
            Some("could not write .brewery/db.json")
        );
    });
    assert_eq!(
        fixture.calls_of("db/connections/list").len(),
        1,
        "nothing to relist"
    );
}

#[gpui::test]
async fn test_connection_shows_testing_then_failures(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    let modal = fixture.open_add_dialog(cx);
    let name = modal.read_with(cx, |modal, _| modal.name_field().clone());
    set_field(&name, "Billing", cx);

    // The daemon answers, but the connection did not work.
    *fixture.state.test_reply.borrow_mut() = Some(json!({
        "ok": false,
        "message": "password authentication failed for user \"app\""
    }));
    fixture.state.hold("db/connections/test");
    modal.update(cx, |modal, cx| modal.test_connection(cx));
    cx.run_until_parked();
    modal.read_with(cx, |modal, _| {
        assert_eq!(modal.test_state(), &TestState::Testing)
    });
    fixture.state.release("db/connections/test");
    cx.run_until_parked();
    modal.read_with(cx, |modal, _| {
        assert_eq!(
            modal.test_state(),
            &TestState::Failed("password authentication failed for user \"app\"".into())
        )
    });

    // The daemon itself reports an error.
    fixture.state.held_methods.borrow_mut().clear();
    fixture
        .state
        .fail("db/connections/test", "Connection refused: localhost:5432");
    modal.update(cx, |modal, cx| modal.test_connection(cx));
    cx.run_until_parked();
    modal.read_with(cx, |modal, _| {
        assert_eq!(
            modal.test_state(),
            &TestState::Failed("Connection refused: localhost:5432".into())
        )
    });
}

#[gpui::test]
async fn editing_a_field_drops_the_test_result_and_a_pending_test(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    let modal = fixture.open_add_dialog(cx);
    let (name, url) = modal.read_with(cx, |modal, _| {
        (modal.name_field().clone(), modal.url_field().clone())
    });
    set_field(&name, "Billing", cx);
    cx.run_until_parked();

    modal.update(cx, |modal, cx| modal.test_connection(cx));
    cx.run_until_parked();
    assert!(matches!(
        modal.read_with(cx, |modal, _| modal.test_state().clone()),
        TestState::Succeeded(_)
    ));
    set_field(&url, "jdbc:postgresql://elsewhere/app", cx);
    cx.run_until_parked();
    modal.read_with(cx, |modal, _| {
        assert_eq!(modal.test_state(), &TestState::Idle)
    });

    fixture.state.hold("db/connections/test");
    modal.update(cx, |modal, cx| modal.test_connection(cx));
    cx.run_until_parked();
    set_field(&url, "jdbc:postgresql://third/app", cx);
    cx.run_until_parked();
    fixture.state.release("db/connections/test");
    cx.run_until_parked();
    modal.read_with(cx, |modal, _| {
        assert_eq!(
            modal.test_state(),
            &TestState::Idle,
            "a result for the old URL is not shown"
        )
    });
}

#[gpui::test]
async fn add_driver_registers_the_jars_and_selects_the_driver(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    let modal = fixture.open_add_dialog(cx);
    let [name, class, jars] = modal.update(cx, |modal, cx| {
        modal.set_show_add_driver(true, cx);
        modal.new_driver_fields().map(Clone::clone)
    });

    // Missing fields are refused before anything is sent.
    modal.update_in(cx, |modal, window, cx| modal.add_driver(window, cx));
    modal.read_with(cx, |modal, _| {
        assert_eq!(
            modal.add_driver_error().map(|e| e.as_ref()),
            Some("Enter a driver name")
        )
    });

    set_field(&name, "Oracle XE", cx);
    set_field(&class, "oracle.jdbc.OracleDriver", cx);
    set_field(&jars, "/drivers/ojdbc11.jar, /drivers/orai18n.jar", cx);

    fixture.state.fail(
        "db/drivers/addJar",
        "class oracle.jdbc.OracleDriver not found",
    );
    modal.update_in(cx, |modal, window, cx| modal.add_driver(window, cx));
    cx.run_until_parked();
    modal.read_with(cx, |modal, _| {
        assert_eq!(
            modal.add_driver_error().map(|e| e.as_ref()),
            Some("class oracle.jdbc.OracleDriver not found")
        );
        assert_eq!(modal.driver_id(), "postgresql");
    });

    fixture.state.failing.borrow_mut().clear();
    modal.update_in(cx, |modal, window, cx| modal.add_driver(window, cx));
    cx.run_until_parked();
    let expected = json!({
        "driverId": "oracle-xe",
        "name": "Oracle XE",
        "className": "oracle.jdbc.OracleDriver",
        "jarPaths": ["/drivers/ojdbc11.jar", "/drivers/orai18n.jar"]
    });
    assert_eq!(
        fixture.calls_of("db/drivers/addJar"),
        [expected.clone(), expected]
    );
    modal.read_with(cx, |modal, cx| {
        assert_eq!(modal.driver_id(), "oracle-xe");
        assert_eq!(modal.add_driver_error(), None);
        assert_eq!(
            modal.url_field().read(cx).text(cx),
            "",
            "the PostgreSQL template does not fit the new driver"
        );
    });
}

#[gpui::test]
async fn escape_closes_the_dialog_and_enter_saves(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    load_default_keymap(cx);

    fixture.open_add_dialog(cx);
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(fixture.modal(cx).is_none(), "Esc closes the dialog");

    let modal = fixture.open_add_dialog(cx);
    // The dialog opens with the focus in Name, so the name can be typed straight away.
    cx.simulate_input("Billing");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    let saves = fixture.calls_of("db/connections/save");
    assert_eq!(saves.len(), 1, "Enter saves");
    assert_eq!(saves[0]["connection"]["name"], "Billing");
    assert!(fixture.modal(cx).is_none());
    drop(modal);

    // In the driver section Enter adds the driver instead.
    let modal = fixture.open_add_dialog(cx);
    let [name, class, jars] = modal.update(cx, |modal, cx| {
        modal.set_show_add_driver(true, cx);
        modal.new_driver_fields().map(Clone::clone)
    });
    set_field(&class, "oracle.jdbc.OracleDriver", cx);
    set_field(&jars, "/drivers/ojdbc11.jar", cx);
    cx.update(|window, cx| window.focus(&name.focus_handle(cx), cx));
    cx.simulate_input("Oracle");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(fixture.calls_of("db/drivers/addJar").len(), 1);
    assert_eq!(
        fixture.calls_of("db/connections/save").len(),
        1,
        "no second save"
    );
    assert!(fixture.modal(cx).is_some());
}

#[gpui::test]
async fn a_failed_delete_is_reported_in_the_panel(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture
        .state
        .fail("db/connections/delete", "unknown connection c2");
    select_source(&fixture, "c2", cx);
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.delete_selected(window, cx));
    cx.run_until_parked();
    cx.simulate_prompt_answer("Delete");
    cx.run_until_parked();

    assert_eq!(fixture.calls_of("db/connections/delete").len(), 1);
    assert_eq!(fixture.source_names(cx), ["Local Postgres", "Reports"]);
    fixture.panel.read_with(cx, |panel, _| {
        assert_eq!(
            panel.notice().map(|notice| notice.as_ref()),
            Some("Could not delete \"Reports\": unknown connection c2")
        );
    });
}

#[gpui::test]
async fn test_connection_does_not_need_a_name(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    let modal = fixture.open_add_dialog(cx);
    modal.update(cx, |modal, cx| modal.test_connection(cx));
    cx.run_until_parked();
    assert_eq!(fixture.calls_of("db/connections/test").len(), 1);
    modal.read_with(cx, |modal, _| {
        assert_eq!(modal.error(), None, "only Save asks for a name");
        assert!(matches!(modal.test_state(), TestState::Succeeded(_)));
    });
}

#[gpui::test]
async fn every_expandable_row_has_a_chevron(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture
        .panel
        .update(cx, |panel, cx| panel.toggle(RowKey::data_source("c1"), cx));
    cx.run_until_parked();
    fixture.panel.update(cx, |panel, cx| {
        panel.toggle(RowKey::new("c1", vec!["public".to_string()]), cx)
    });
    cx.run_until_parked();
    let chevrons: Vec<Option<IconName>> = fixture.panel.read_with(cx, |panel, _| {
        panel.tree().rows().iter().map(row_chevron).collect()
    });
    assert_eq!(
        chevrons,
        [
            Some(IconName::ChevronDown),  // Local Postgres, expanded
            Some(IconName::ChevronDown),  // public, expanded
            Some(IconName::ChevronRight), // owners, collapsed table
            None,                         // owners_id_seq, a leaf
            Some(IconName::ChevronRight), // Reports, collapsed data source
        ]
    );
}

#[gpui::test]
async fn the_panel_moves_to_the_right_dock_and_remembers_it(cx: &mut TestAppContext) {
    let (fixture, cx) = workspace_with_panel(cx).await;
    fixture.panel.update_in(cx, |panel, window, cx| {
        assert!(panel.position_is_valid(DockPosition::Left));
        assert!(panel.position_is_valid(DockPosition::Right));
        panel.set_position(DockPosition::Right, window, cx);
    });
    cx.run_until_parked();

    fixture.workspace.read_with(cx, |workspace, cx| {
        let left = workspace.left_dock().read(cx).panel::<DatabasePanel>();
        assert!(left.is_none(), "the panel should have left the left dock");
        let right = workspace.right_dock().read(cx).panel::<DatabasePanel>();
        assert_eq!(
            right.map(|panel| panel.entity_id()),
            Some(fixture.panel.entity_id())
        );
        assert_eq!(
            DatabasePanelSettings::get_global(cx).dock,
            settings::DockPosition::Right
        );
    });
    let fs = fixture.workspace.read_with(cx, |workspace, cx| {
        workspace.project().read(cx).fs().clone()
    });
    let saved = fs
        .load(paths::settings_file())
        .await
        .expect("the dock side is saved to the settings file");
    let saved: Value = settings::parse_json_with_comments(&saved).unwrap();
    assert_eq!(saved["database_panel"]["dock"], json!("right"));
}

fn expand_public(fixture: &Fixture, cx: &mut VisualTestContext) {
    fixture
        .panel
        .update(cx, |panel, cx| panel.toggle(RowKey::data_source("c1"), cx));
    cx.run_until_parked();
    fixture.panel.update(cx, |panel, cx| {
        panel.toggle(RowKey::new("c1", vec!["public".to_string()]), cx)
    });
    cx.run_until_parked();
}

#[gpui::test]
async fn a_table_copies_its_qualified_name_and_selects_its_top_rows(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    expand_public(&fixture, cx);
    let owners = RowKey::new("c1", vec!["public".to_string(), "owners".to_string()]);
    fixture
        .panel
        .update(cx, |panel, cx| panel.select(Some(owners), cx));
    fixture.panel.update(cx, |panel, cx| panel.copy_name(cx));
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("public.owners".to_string())
    );

    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.select_top(window, cx));
    cx.run_until_parked();
    let sql: Vec<Value> = fixture
        .calls_of("db/execute")
        .iter()
        .map(|params| params["sql"].clone())
        .collect();
    assert_eq!(sql, [json!("SELECT * FROM public.owners LIMIT 500")]);
    let console = fixture.workspace.read_with(cx, |workspace, cx| {
        workspace.active_item_as::<QueryConsole>(cx)
    });
    assert!(console.is_some(), "Select Top opens a console");

    // Other nodes copy their own name; Select Top is only for tables and views.
    let sequence = RowKey::new(
        "c1",
        vec!["public".to_string(), "owners_id_seq".to_string()],
    );
    fixture
        .panel
        .update(cx, |panel, cx| panel.select(Some(sequence), cx));
    fixture.panel.update(cx, |panel, cx| panel.copy_name(cx));
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("owners_id_seq".to_string())
    );
    fixture
        .panel
        .update_in(cx, |panel, window, cx| panel.select_top(window, cx));
    cx.run_until_parked();
    assert_eq!(fixture.calls_of("db/execute").len(), 1);
}

#[gpui::test]
async fn showing_system_objects_lists_the_open_nodes_again_with_them(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    expand_public(&fixture, cx);
    assert_eq!(
        fixture.calls_of("db/introspect"),
        [
            json!({ "connectionId": "c1", "path": [] }),
            json!({ "connectionId": "c1", "path": ["public"] }),
        ]
    );
    fixture
        .panel
        .update(cx, |panel, cx| panel.set_show_system_objects(true, cx));
    cx.run_until_parked();
    let calls = fixture.calls_of("db/introspect");
    assert_eq!(
        calls[2..],
        [
            json!({ "connectionId": "c1", "path": [], "includeSystem": true }),
            json!({ "connectionId": "c1", "path": ["public"], "includeSystem": true }),
        ]
    );
    fixture
        .panel
        .read_with(cx, |panel, _| assert!(panel.show_system_objects()));
}

#[gpui::test]
async fn an_authentication_failure_asks_for_the_password(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture.state.fail_with_code(
        "db/introspect",
        brewery_daemon_client::DB_AUTH_FAILED,
        "password authentication failed for user \"app\"",
    );
    fixture
        .panel
        .update(cx, |panel, cx| panel.toggle(RowKey::data_source("c1"), cx));
    cx.run_until_parked();
    let modal = fixture
        .modal(cx)
        .expect("the data source dialog should ask for the password");
    modal.update_in(cx, |modal, window, cx| {
        assert!(modal.is_editing());
        assert!(modal.is_asking_for_password());
        assert_eq!(modal.error().map(|e| e.as_ref()), Some(ENTER_PASSWORD));
        assert!(
            modal
                .password_field()
                .focus_handle(cx)
                .contains_focused(window, cx),
            "the password field has the focus"
        );
    });

    // Typing the password and saving lists the data source again.
    set_field(
        &modal.read_with(cx, |modal, _| modal.password_field().clone()),
        "secret",
        cx,
    );
    fixture.state.succeed("db/introspect");
    modal.update(cx, |modal, cx| modal.save(cx));
    cx.run_until_parked();
    let saves = fixture.calls_of("db/connections/save");
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0]["password"], "secret");
    assert!(fixture.modal(cx).is_none());
    let rows = fixture.panel.read_with(cx, |panel, _| panel.tree().rows());
    assert!(rows.iter().any(|row| matches!(row, Row::Node { .. })));

    // Another failure while the dialog is open does not stack a second dialog.
    fixture.state.fail_with_code(
        "db/introspect",
        brewery_daemon_client::DB_AUTH_FAILED,
        "password authentication failed for user \"app\"",
    );
    fixture.panel.update(cx, |panel, cx| panel.refresh(cx));
    cx.run_until_parked();
    assert!(fixture.modal(cx).is_some());
    fixture.panel.update(cx, |panel, cx| panel.refresh(cx));
    cx.run_until_parked();
    assert!(fixture.modal(cx).is_some(), "still the one dialog");
}

#[gpui::test]
async fn other_failures_do_not_open_a_dialog(cx: &mut TestAppContext) {
    let (fixture, cx) = shown_panel(cx).await;
    fixture
        .panel
        .update(cx, |panel, cx| panel.toggle(RowKey::data_source("c2"), cx));
    cx.run_until_parked();
    assert!(fixture.modal(cx).is_none());
}
