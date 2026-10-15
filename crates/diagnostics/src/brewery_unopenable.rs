// brewery: diagnostics for project paths that cannot be shown as excerpts.
//
// The project diagnostics view renders excerpts of buffers. Language servers also
// publish diagnostics for paths that cannot be opened as a buffer: jdtls reports build
// path and Gradle import problems on the project folder itself. Those are counted in
// the status bar and marked in the project panel, but the view used to show a blank
// editor for them. They are listed here as plain rows above the excerpts instead.

use collections::BTreeSet;
use gpui::{App, Context, Div, Entity, SharedString};
use project::{Project, ProjectPath};
use ui::{Icon, IconName, IconSize, Label, Tooltip, prelude::*};

use crate::ProjectDiagnosticsEditor;

/// Paths with diagnostics that failed to open as a buffer.
#[derive(Default)]
pub(crate) struct UnopenablePaths(BTreeSet<ProjectPath>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnopenableRow {
    pub path: ProjectPath,
    pub path_label: SharedString,
    pub severity: lsp::DiagnosticSeverity,
    pub message: SharedString,
    pub source: Option<SharedString>,
}

impl UnopenableRow {
    /// One line of plain text, e.g. `app (project): The project cannot be built [Java]`.
    pub fn text(&self) -> String {
        let mut text = format!("{}: {}", self.path_label, self.message);
        if let Some(source) = &self.source {
            text.push_str(&format!(" [{source}]"));
        }
        text
    }
}

impl UnopenablePaths {
    pub fn record(&mut self, path: &ProjectPath, opened: bool) {
        if opened {
            self.0.remove(path);
        } else {
            self.0.insert(path.clone());
        }
    }

    pub fn rows(
        &self,
        project: &Entity<Project>,
        include_warnings: bool,
        cx: &App,
    ) -> Vec<UnopenableRow> {
        let max_severity = if include_warnings {
            lsp::DiagnosticSeverity::WARNING
        } else {
            lsp::DiagnosticSeverity::ERROR
        };
        let project = project.read(cx);
        let lsp_store = project.lsp_store().read(cx);
        let mut rows = Vec::new();
        for path in &self.0 {
            // Same paths as the status bar count: ignored entries are not counted there.
            if project
                .entry_for_path(path, cx)
                .is_none_or(|entry| entry.is_ignored)
            {
                continue;
            }
            let root_name = project
                .worktree_for_id(path.worktree_id, cx)
                .map(|worktree| worktree.read(cx).root_name_str().to_string())
                .unwrap_or_default();
            let path_label: SharedString = if path.path.is_empty() {
                format!("{root_name} (project)").into()
            } else {
                format!("{root_name}/{}", path.path.as_unix_str()).into()
            };
            for (_, entry) in lsp_store.worktree_diagnostics_for_path(path) {
                let diagnostic = entry.diagnostic;
                if !diagnostic.is_primary || diagnostic.severity > max_severity {
                    continue;
                }
                let row = UnopenableRow {
                    path: path.clone(),
                    path_label: path_label.clone(),
                    severity: diagnostic.severity,
                    message: diagnostic.message.as_str().trim().to_string().into(),
                    source: diagnostic.source.map(SharedString::from),
                };
                if !rows.contains(&row) {
                    rows.push(row);
                }
            }
        }
        rows.sort_by_key(|row| row.severity);
        rows
    }
}

/// The editor, with the rows of unopenable paths above it when there are any.
pub(crate) fn render_with_editor(
    this: &ProjectDiagnosticsEditor,
    cx: &mut Context<ProjectDiagnosticsEditor>,
) -> Div {
    let rows = this
        .brewery_unopenable
        .rows(&this.project, this.include_warnings, cx);
    if rows.is_empty() {
        return div().size_full().child(this.editor.clone());
    }
    let editor_is_empty = this.multibuffer.read(cx).is_empty();
    let colors = cx.theme().colors();
    let list = v_flex()
        .w_full()
        .py_1()
        .border_b_1()
        .border_color(colors.border_variant)
        .children(rows.into_iter().enumerate().map(|(ix, row)| {
            let (icon, color) = if row.severity == lsp::DiagnosticSeverity::ERROR {
                (IconName::XCircle, Color::Error)
            } else {
                (IconName::Warning, Color::Warning)
            };
            let first_line = row.message.lines().next().unwrap_or_default().to_string();
            let entry_id = this
                .project
                .read(cx)
                .entry_for_path(&row.path, cx)
                .map(|e| e.id);
            let project = this.project.clone();
            h_flex()
                .id(("brewery-unopenable-diagnostic", ix))
                .w_full()
                .px_2()
                .py_0p5()
                .gap_2()
                .cursor_pointer()
                .hover(|style| style.bg(colors.element_hover))
                .tooltip(Tooltip::text(row.text()))
                .on_click(move |_, _, cx| {
                    if let Some(entry_id) = entry_id {
                        project.update(cx, |_, cx| {
                            cx.emit(project::Event::RevealInProjectPanel(entry_id))
                        });
                    }
                })
                .child(Icon::new(icon).size(IconSize::Small).color(color))
                .child(Label::new(row.path_label.clone()))
                .child(Label::new(first_line).truncate())
                .children(
                    row.source
                        .map(|source| Label::new(format!("[{source}]")).color(Color::Muted)),
                )
        }));
    v_flex()
        .size_full()
        .bg(colors.editor_background)
        .child(list)
        .when(!editor_is_empty, |this_div| {
            this_div.child(div().flex_1().min_h_0().child(this.editor.clone()))
        })
}

#[cfg(test)]
mod tests {
    use crate::{DIAGNOSTICS_UPDATE_DEBOUNCE, ProjectDiagnosticsEditor};
    use editor::test::editor_content_with_blocks;
    use gpui::{TestAppContext, VisualTestContext};
    use language::DiagnosticSourceKind;
    use lsp::LanguageServerId;
    use project::{FakeFs, Project};
    use serde_json::json;
    use settings::SettingsStore;
    use std::time::Duration;
    use util::path;
    use workspace::MultiWorkspace;

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            zlog::init_test();
            let settings = SettingsStore::test(cx);
            cx.set_global(settings);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            crate::init(cx);
            editor::init(cx);
        });
    }

    fn publish(
        lsp_store: &gpui::Entity<project::LspStore>,
        uri: &str,
        message: &str,
        cx: &mut VisualTestContext,
    ) {
        let uri = lsp::Uri::from_file_path(uri).unwrap();
        lsp_store.update(cx, |lsp_store, cx| {
            lsp_store
                .update_diagnostics(
                    LanguageServerId(0),
                    lsp::PublishDiagnosticsParams {
                        uri,
                        diagnostics: vec![lsp::Diagnostic {
                            range: lsp::Range::new(
                                lsp::Position::new(0, 0),
                                lsp::Position::new(0, 0),
                            ),
                            severity: Some(lsp::DiagnosticSeverity::ERROR),
                            source: Some("Java".to_string()),
                            message: lsp::DiagnosticMessage::String(message.to_string()),
                            ..Default::default()
                        }],
                        version: None,
                    },
                    None,
                    DiagnosticSourceKind::Pushed,
                    &[],
                    cx,
                )
                .unwrap();
        });
    }

    fn rows(
        diagnostics: &gpui::Entity<ProjectDiagnosticsEditor>,
        cx: &mut VisualTestContext,
    ) -> Vec<String> {
        diagnostics.read_with(cx, |diagnostics, cx| {
            diagnostics
                .brewery_unopenable
                .rows(&diagnostics.project, diagnostics.include_warnings, cx)
                .iter()
                .map(|row| row.text())
                .collect()
        })
    }

    #[gpui::test]
    async fn test_directory_diagnostic_is_listed(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/EvaAssistantAIBot"),
            json!({
                "build.gradle": "plugins { id 'java' }\n",
                "src": { "Main.java": "class Main {}\n" },
            }),
        )
        .await;
        let project = Project::test(fs.clone(), [path!("/EvaAssistantAIBot").as_ref()], cx).await;
        let lsp_store = project.read_with(cx, |project, _| project.lsp_store());
        let window =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        let workspace = window
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();

        // jdtls reports build path problems on the project folder itself.
        publish(
            &lsp_store,
            path!("/EvaAssistantAIBot"),
            "The project cannot be built until build path errors are resolved",
            cx,
        );
        // A path outside every worktree is dropped by the project and never counted.
        publish(&lsp_store, path!("/elsewhere/Other.java"), "outside", cx);

        let diagnostics = window.build_entity(cx, |window, cx| {
            ProjectDiagnosticsEditor::new(true, project.clone(), workspace.downgrade(), window, cx)
        });
        let editor = diagnostics.update(cx, |diagnostics, _| diagnostics.editor.clone());
        diagnostics
            .next_notification(DIAGNOSTICS_UPDATE_DEBOUNCE + Duration::from_millis(10), cx)
            .await;
        cx.run_until_parked();

        let summary = project.read_with(cx, |project, cx| project.diagnostic_summary(false, cx));
        assert_eq!(
            summary.error_count, 1,
            "only the in-worktree directory counts"
        );
        assert_eq!(
            rows(&diagnostics, cx),
            vec![
                "EvaAssistantAIBot (project): The project cannot be built until build path \
                 errors are resolved [Java]"
                    .to_string()
            ]
        );
        assert_eq!(editor_content_with_blocks(&editor, cx), "");
        // Render the rows-only view (no excerpts) once.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_active_pane(Box::new(diagnostics.clone()), None, true, window, cx)
        });
        cx.run_until_parked();

        // Ordinary file diagnostics still render as excerpts next to the rows.
        publish(
            &lsp_store,
            path!("/EvaAssistantAIBot/src/Main.java"),
            "cannot find symbol",
            cx,
        );
        cx.executor()
            .advance_clock(DIAGNOSTICS_UPDATE_DEBOUNCE + Duration::from_millis(10));
        cx.run_until_parked();
        assert_eq!(rows(&diagnostics, cx).len(), 1);
        assert!(
            editor_content_with_blocks(&editor, cx).contains("cannot find symbol"),
            "{}",
            editor_content_with_blocks(&editor, cx)
        );

        // Once the server clears the folder's diagnostics, the row goes away.
        let uri = lsp::Uri::from_file_path(path!("/EvaAssistantAIBot")).unwrap();
        lsp_store.update(cx, |lsp_store, cx| {
            lsp_store
                .update_diagnostics(
                    LanguageServerId(0),
                    lsp::PublishDiagnosticsParams {
                        uri,
                        diagnostics: Vec::new(),
                        version: None,
                    },
                    None,
                    DiagnosticSourceKind::Pushed,
                    &[],
                    cx,
                )
                .unwrap();
        });
        cx.executor()
            .advance_clock(DIAGNOSTICS_UPDATE_DEBOUNCE + Duration::from_millis(10));
        cx.run_until_parked();
        assert!(rows(&diagnostics, cx).is_empty());
    }
}
