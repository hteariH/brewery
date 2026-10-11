use super::*;
use brewery_daemon_client::{
    BuildTool, Daemon, DaemonFactory, ProjectModel, RunConfigurationKind, StartDaemon,
};
use futures::FutureExt as _;
use futures::future::LocalBoxFuture;
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use util::path;
use workspace::{AppState, MultiWorkspace};

const SPRING_MODEL: &str =
    include_str!("../../../brewery-daemon/protocol/fixtures/spring-model.json");
const RUN_CONFIGURATIONS: &str =
    include_str!("../../../brewery-daemon/protocol/fixtures/run-configurations.json");

fn fixture_model() -> SpringModel {
    serde_json::from_str(SPRING_MODEL).unwrap()
}

fn fixture_configurations() -> Vec<RunConfiguration> {
    serde_json::from_str(RUN_CONFIGURATIONS).unwrap()
}

fn endpoint(method: HttpMethod, path: &str, handler: &str) -> SpringEndpoint {
    SpringEndpoint {
        method,
        path: path.to_string(),
        handler: handler.to_string(),
        file: "/src/X.java".to_string(),
        line: 1,
    }
}

fn bean(name: &str, kind: SpringBeanKind) -> SpringBean {
    SpringBean {
        name: name.to_string(),
        type_name: name.to_string(),
        kind,
        file: "/src/X.java".to_string(),
        line: 1,
    }
}

#[test]
fn controller_is_the_part_before_the_hash() {
    assert_eq!(controller_name("OwnerController#list"), "OwnerController");
    assert_eq!(controller_name("Orphan"), "Orphan");
}

#[test]
fn endpoints_are_grouped_by_controller_and_sorted_by_path() {
    let endpoints = vec![
        endpoint(HttpMethod::Post, "/pets", "PetController#create"),
        endpoint(HttpMethod::Get, "/owners/{id}", "OwnerController#get"),
        endpoint(HttpMethod::Get, "/pets", "PetController#list"),
        endpoint(HttpMethod::Get, "/owners", "OwnerController#list"),
    ];
    assert_eq!(
        endpoints_by_controller(&endpoints),
        vec![
            ("OwnerController", vec![3, 1]),
            ("PetController", vec![2, 0])
        ]
    );
}

#[test]
fn beans_are_grouped_by_kind_and_sorted_by_name() {
    let beans = vec![
        bean("ownerService", SpringBeanKind::Service),
        bean("clock", SpringBeanKind::Bean),
        bean("auditService", SpringBeanKind::Service),
        bean("ownerRepository", SpringBeanKind::Repository),
    ];
    assert_eq!(
        beans_by_kind(&beans),
        vec![
            (SpringBeanKind::Service, vec![2, 0]),
            (SpringBeanKind::Repository, vec![3]),
            (SpringBeanKind::Bean, vec![1]),
        ]
    );
    assert_eq!(
        kind_label(SpringBeanKind::RestController),
        "REST controllers"
    );
}

#[test]
fn method_labels_are_http_verbs() {
    let labels: Vec<_> = [
        HttpMethod::Get,
        HttpMethod::Post,
        HttpMethod::Put,
        HttpMethod::Delete,
        HttpMethod::Patch,
        HttpMethod::Any,
    ]
    .into_iter()
    .map(method_label)
    .collect();
    assert_eq!(labels, ["GET", "POST", "PUT", "DELETE", "PATCH", "ANY"]);
}

#[test]
fn run_template_comes_from_the_matching_configuration() {
    let model = fixture_model();
    let configurations = fixture_configurations();
    let app = &model.applications[0];
    let configuration =
        run_configuration_for(app, &configurations).expect("the app has a configuration");
    assert_eq!(configuration.kind, RunConfigurationKind::SpringBoot);

    let template = run_template(configuration);
    assert_eq!(template.label, "PetshopApplication (Spring Boot)");
    assert_eq!(template.command, "/path/to/spring-petshop/gradlew");
    assert_eq!(template.args, vec!["bootRun".to_string()]);
    assert_eq!(template.cwd.as_deref(), Some("/path/to/spring-petshop"));

    let other = SpringApp {
        name: "Elsewhere".to_string(),
        ..app.clone()
    };
    assert!(run_configuration_for(&other, &configurations).is_none());
}

type Handlers = Arc<Mutex<Vec<Box<dyn Fn() + Send>>>>;

/// Serves a fixed Spring model and counts how often it was asked.
struct FakeDaemon {
    model: SpringModel,
    configurations: Vec<RunConfiguration>,
    spring_requests: Rc<RefCell<usize>>,
    spring_changed: Handlers,
}

impl Daemon for FakeDaemon {
    fn project_model(&self) -> LocalBoxFuture<'_, Result<ProjectModel>> {
        async {
            Ok(ProjectModel {
                build_tool: BuildTool::Gradle,
                root_dir: "/root".to_string(),
                modules: Vec::new(),
            })
        }
        .boxed_local()
    }

    fn spring_model(&self) -> LocalBoxFuture<'_, Result<SpringModel>> {
        *self.spring_requests.borrow_mut() += 1;
        let model = self.model.clone();
        async move { Ok(model) }.boxed_local()
    }

    fn run_configurations(&self) -> LocalBoxFuture<'_, Result<Vec<RunConfiguration>>> {
        let configurations = self.configurations.clone();
        async move { Ok(configurations) }.boxed_local()
    }

    fn on_project_changed(&self, _: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        Box::new(())
    }

    fn on_spring_changed(&self, f: Box<dyn Fn() + Send>) -> Box<dyn Any> {
        self.spring_changed.lock().unwrap().push(f);
        Box::new(())
    }

    fn on_exit(&self, _: Box<dyn FnOnce() + Send>) {}
}

struct Fixture {
    workspace: Entity<Workspace>,
    panel: Entity<ServicesPanel>,
    spring_requests: Rc<RefCell<usize>>,
    spring_changed: Handlers,
}

async fn workspace_with_panel(
    model: SpringModel,
    files: serde_json::Value,
    cx: &mut TestAppContext,
) -> (Fixture, &mut VisualTestContext) {
    let app_state = cx.update(|cx| {
        let state = AppState::test(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        editor::init(cx);
        super::init(cx);
        state
    });
    app_state
        .fs
        .as_fake()
        .insert_tree(path!("/root"), files)
        .await;
    let project = Project::test(app_state.fs.clone(), [path!("/root").as_ref()], cx).await;
    let (multi_workspace, cx) =
        cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
    let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

    let spring_requests = Rc::new(RefCell::new(0));
    let spring_changed: Handlers = Default::default();
    let start_daemon: StartDaemon = {
        let spring_requests = spring_requests.clone();
        let spring_changed = spring_changed.clone();
        Rc::new(move |_, _| {
            let daemon: Rc<dyn Daemon> = Rc::new(FakeDaemon {
                model: model.clone(),
                configurations: fixture_configurations(),
                spring_requests: spring_requests.clone(),
                spring_changed: spring_changed.clone(),
            });
            async move { Ok(daemon) }.boxed_local()
        })
    };
    cx.update(|_, cx| DaemonFactory::set(start_daemon, cx));

    let panel = workspace.update_in(cx, |workspace, window, cx| {
        let weak = workspace.weak_handle();
        let project = workspace.project().clone();
        let panel = cx.new(|cx| ServicesPanel::new(weak, project, window, cx));
        workspace.add_panel(panel.clone(), window, cx);
        panel
    });
    (
        Fixture {
            workspace,
            panel,
            spring_requests,
            spring_changed,
        },
        cx,
    )
}

fn rendered_rows(panel: &Entity<ServicesPanel>, cx: &mut VisualTestContext) -> usize {
    panel.update_in(cx, |panel, _, cx| {
        let model = panel.model.clone().expect("the model should be loaded");
        panel.render_entries(&model, cx).len()
    })
}

#[gpui::test]
async fn toggling_shows_the_fixture_model_in_the_bottom_dock(cx: &mut TestAppContext) {
    let (fixture, cx) =
        workspace_with_panel(fixture_model(), json!({ "build.gradle": "" }), cx).await;
    assert_eq!(ServicesPanel::persistent_name(), "Services Panel");
    assert_eq!(
        *fixture.spring_requests.borrow(),
        0,
        "no daemon before the panel is shown"
    );

    cx.dispatch_action(ToggleServicesPanel);
    cx.run_until_parked();

    fixture.workspace.read_with(cx, |workspace, cx| {
        let dock = workspace.bottom_dock().read(cx);
        assert!(dock.is_open(), "the bottom dock should be open");
        let visible = dock.visible_panel().expect("a panel should be visible");
        assert_eq!(visible.panel_id(), fixture.panel.entity_id());
    });
    assert_eq!(*fixture.spring_requests.borrow(), 1);
    // Spring Boot + 1 app; Endpoints + OwnerController + 4 endpoints;
    // Beans + 5 kinds with one bean each.
    assert_eq!(rendered_rows(&fixture.panel, cx), 2 + 6 + 11);

    fixture.panel.update(cx, |panel, cx| {
        panel.toggle_collapsed(Entry::Section(Section::Beans), cx);
        panel.toggle_collapsed(
            Entry::Group(Group::Controller("OwnerController".into())),
            cx,
        );
    });
    assert_eq!(rendered_rows(&fixture.panel, cx), 2 + 2 + 1);
}

#[gpui::test]
async fn an_empty_model_says_so_per_section(cx: &mut TestAppContext) {
    let (fixture, cx) =
        workspace_with_panel(SpringModel::default(), json!({ "build.gradle": "" }), cx).await;
    cx.dispatch_action(ToggleServicesPanel);
    cx.run_until_parked();

    fixture.panel.read_with(cx, |panel, _| {
        assert_eq!(
            panel.entries(),
            vec![
                Entry::Section(Section::SpringBoot),
                Entry::Empty(Section::SpringBoot),
                Entry::Section(Section::Endpoints),
                Entry::Empty(Section::Endpoints),
                Entry::Section(Section::Beans),
                Entry::Empty(Section::Beans),
            ]
        );
    });
    assert_eq!(
        Section::SpringBoot.empty_message(),
        "No Spring Boot application found"
    );
}

#[gpui::test]
async fn changed_sources_are_scanned_again(cx: &mut TestAppContext) {
    let (fixture, cx) =
        workspace_with_panel(fixture_model(), json!({ "build.gradle": "" }), cx).await;
    cx.dispatch_action(ToggleServicesPanel);
    cx.run_until_parked();
    assert_eq!(*fixture.spring_requests.borrow(), 1);

    for handler in fixture.spring_changed.lock().unwrap().iter() {
        handler();
    }
    cx.run_until_parked();
    assert_eq!(*fixture.spring_requests.borrow(), 2);
}

#[gpui::test]
async fn activating_an_endpoint_opens_its_file_at_the_line(cx: &mut TestAppContext) {
    let source =
        "package demo;\n\nclass OwnerController {\n    @GetMapping\n    void list() {}\n}\n";
    let file = path!("/root/src/OwnerController.java");
    let model = SpringModel {
        applications: Vec::new(),
        beans: Vec::new(),
        endpoints: vec![SpringEndpoint {
            method: HttpMethod::Get,
            path: "/owners".to_string(),
            handler: "OwnerController#list".to_string(),
            file: file.to_string(),
            line: 4,
        }],
    };
    let (fixture, cx) = workspace_with_panel(
        model,
        json!({ "src": { "OwnerController.java": source } }),
        cx,
    )
    .await;
    cx.dispatch_action(ToggleServicesPanel);
    cx.run_until_parked();

    let ix = fixture.panel.read_with(cx, |panel, _| {
        panel
            .entries()
            .iter()
            .position(|entry| *entry == Entry::Endpoint(0))
            .expect("the endpoint row should be listed")
    });
    fixture.panel.update_in(cx, |panel, window, cx| {
        panel.selected = Some(ix);
        panel.confirm(&menu::Confirm, window, cx);
    });
    cx.run_until_parked();

    let editor = fixture.workspace.read_with(cx, |workspace, cx| {
        workspace
            .active_item(cx)
            .and_then(|item| item.downcast::<Editor>())
            .expect("the file should be open in an editor")
    });
    editor.update(cx, |editor, cx| {
        let snapshot = editor.display_snapshot(cx);
        assert_eq!(
            editor.selections.newest::<Point>(&snapshot).head(),
            Point::new(3, 0)
        );
    });
}
