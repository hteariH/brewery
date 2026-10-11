//! Search Everywhere: IntelliJ's Shift Shift popup.
//!
//! One modal with a tab row (Files | Symbols | Actions) above the active tab's
//! picker. Each tab hosts one of Zed's own pickers unchanged (file finder,
//! project symbols, command palette), so their matching, previews and confirm
//! behaviour stay exactly Zed's. Switching tabs rebuilds the hosted picker and
//! carries the typed query over.

use command_palette::CommandPalette;
use file_finder::FileFinder;
use gpui::{
    AnyView, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, ManagedView,
    MouseButton, Render, Subscription, Task, WeakEntity, Window, actions,
};
use picker::Picker;
use project_symbols::ProjectSymbolsDelegate;
use ui::{ButtonLike, prelude::*};
use util::ResultExt as _;
use workspace::{ModalView, Workspace};

#[cfg(test)]
const TAB_ROW_SELECTOR: &str = "search-everywhere-tab-row";

#[cfg(test)]
mod brewery_search_everywhere_tests;

actions!(
    brewery,
    [
        /// Opens Search Everywhere: files, symbols and actions in one popup.
        SearchEverywhere,
        /// Switches Search Everywhere to the next tab.
        NextSearchTab,
        /// Switches Search Everywhere to the previous tab.
        PreviousSearchTab,
    ]
);

/// The tabs of Search Everywhere, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchTab {
    Files,
    Symbols,
    Actions,
}

impl SearchTab {
    pub const ALL: [SearchTab; 3] = [SearchTab::Files, SearchTab::Symbols, SearchTab::Actions];

    pub fn next(self) -> Self {
        match self {
            SearchTab::Files => SearchTab::Symbols,
            SearchTab::Symbols => SearchTab::Actions,
            SearchTab::Actions => SearchTab::Files,
        }
    }

    pub fn previous(self) -> Self {
        match self {
            SearchTab::Files => SearchTab::Actions,
            SearchTab::Symbols => SearchTab::Files,
            SearchTab::Actions => SearchTab::Symbols,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SearchTab::Files => "Files",
            SearchTab::Symbols => "Symbols",
            SearchTab::Actions => "Actions",
        }
    }
}

pub fn init(cx: &mut App) {
    cx.observe_new(
        |workspace: &mut Workspace, _window, _: &mut Context<Workspace>| {
            workspace.register_action(|workspace, _: &SearchEverywhere, window, cx| {
                SearchEverywhereModal::toggle(workspace, window, cx);
            });
        },
    )
    .detach();
}

/// The picker currently shown under the tab row.
struct ActiveChild {
    view: AnyView,
    focus_handle: FocusHandle,
    query: Box<dyn Fn(&App) -> String>,
    _dismiss: Subscription,
}

pub struct SearchEverywhereModal {
    workspace: WeakEntity<Workspace>,
    /// Where focus was before the popup opened: the command palette lists (and
    /// later dispatches) the actions available there, not inside the popup.
    previous_focus_handle: FocusHandle,
    focus_handle: FocusHandle,
    tab: SearchTab,
    child: Option<ActiveChild>,
    /// The query carried to the next child; the file finder child arrives
    /// asynchronously (it resolves its history first).
    query: String,
    _load_child: Option<Task<()>>,
    _focus_forward: Subscription,
}

impl ModalView for SearchEverywhereModal {}

impl EventEmitter<DismissEvent> for SearchEverywhereModal {}

impl Focusable for SearchEverywhereModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SearchEverywhereModal {
    fn toggle(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        if workspace.active_modal::<Self>(cx).is_some() {
            workspace.hide_modal(window, cx);
            return;
        }
        // Close any other modal first so focus is back where the user was.
        if workspace.has_active_modal(window, cx) && !workspace.hide_modal(window, cx) {
            return;
        }
        let previous_focus_handle = window
            .focused(cx)
            .unwrap_or_else(|| workspace.focus_handle(cx));
        // Started here because the workspace is leased while the modal is built.
        let files = FileFinder::build(workspace, false, None, cx);
        let weak_workspace = cx.entity().downgrade();
        workspace.toggle_modal(window, cx, move |window, cx| {
            Self::new(weak_workspace, previous_focus_handle, files, window, cx)
        });
    }

    fn new<B>(
        workspace: WeakEntity<Workspace>,
        previous_focus_handle: FocusHandle,
        files: Task<B>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self
    where
        B: FnOnce(Option<String>, &mut Window, &mut Context<FileFinder>) -> FileFinder + 'static,
    {
        let focus_handle = cx.focus_handle();
        // The modal layer focuses the popup itself; hand focus on to the picker.
        let focus_forward = cx.on_focus(&focus_handle, window, |this, window, cx| {
            if let Some(child) = &this.child {
                window.focus(&child.focus_handle, cx);
            }
        });
        let mut this = Self {
            workspace,
            previous_focus_handle,
            focus_handle,
            tab: SearchTab::Files,
            child: None,
            query: String::new(),
            _load_child: None,
            _focus_forward: focus_forward,
        };
        this.load_files(files, window, cx);
        this
    }

    pub fn active_tab(&self) -> SearchTab {
        self.tab
    }

    /// The query of the active picker.
    pub fn query(&self, cx: &App) -> String {
        match &self.child {
            Some(child) => (child.query)(cx),
            None => self.query.clone(),
        }
    }

    fn select_tab(&mut self, tab: SearchTab, window: &mut Window, cx: &mut Context<Self>) {
        if tab == self.tab {
            return;
        }
        self.query = self.query(cx);
        self.tab = tab;
        self.child = None;
        self._load_child = None;
        cx.notify();

        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        match tab {
            SearchTab::Files => {
                let files = workspace.update(cx, |workspace, cx| {
                    FileFinder::build(workspace, false, None, cx)
                });
                self.load_files(files, window, cx);
            }
            SearchTab::Symbols => {
                let project = workspace.read(cx).project().clone();
                let weak_workspace = self.workspace.clone();
                let picker = cx.new(|cx| {
                    let delegate = ProjectSymbolsDelegate::new(weak_workspace, project.clone());
                    let preview = picker_preview::editor_preview(project, window, cx);
                    Picker::uniform_list_with_preview(delegate, preview, window, cx)
                });
                picker.update(cx, |picker, cx| picker.set_query(&self.query, window, cx));
                let query_source = picker.clone();
                self.set_child(
                    picker,
                    move |cx| query_source.read(cx).query(cx),
                    window,
                    cx,
                );
            }
            SearchTab::Actions => {
                // The palette collects the actions available at the focused element,
                // so build it with focus back where the user was; `set_child` moves
                // focus into the popup again before the next frame.
                window.focus(&self.previous_focus_handle, cx);
                let previous_focus_handle = self.previous_focus_handle.clone();
                let query = self.query.clone();
                let weak_workspace = self.workspace.clone();
                let palette = cx.new(|cx| {
                    CommandPalette::new(previous_focus_handle, &query, weak_workspace, window, cx)
                });
                let query_source = palette.clone();
                self.set_child(
                    palette,
                    move |cx| query_source.read(cx).query(cx),
                    window,
                    cx,
                );
            }
        }
    }

    fn load_files<B>(&mut self, files: Task<B>, window: &mut Window, cx: &mut Context<Self>)
    where
        B: FnOnce(Option<String>, &mut Window, &mut Context<FileFinder>) -> FileFinder + 'static,
    {
        self._load_child = Some(cx.spawn_in(window, async move |this, cx| {
            let build = files.await;
            this.update_in(cx, |this, window, cx| {
                // A later tab switch drops this task, but stay safe if it raced.
                if this.tab != SearchTab::Files {
                    return;
                }
                let finder = cx.new(|cx| build(None, window, cx));
                let query = this.query.clone();
                finder.update(cx, |finder, cx| finder.set_query(&query, window, cx));
                let query_source = finder.clone();
                this.set_child(
                    finder,
                    move |cx| query_source.read(cx).query(cx),
                    window,
                    cx,
                );
            })
            .log_err();
        }));
    }

    fn set_child<V: ManagedView>(
        &mut self,
        view: Entity<V>,
        query: impl Fn(&App) -> String + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus_handle = view.focus_handle(cx);
        // Confirming or cancelling in the hosted picker closes the whole popup.
        let dismiss = cx.subscribe(&view, |_, _, _: &DismissEvent, cx| cx.emit(DismissEvent));
        window.focus(&focus_handle, cx);
        self.child = Some(ActiveChild {
            view: view.into(),
            focus_handle,
            query: Box::new(query),
            _dismiss: dismiss,
        });
        cx.notify();
    }

    fn render_tab_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // The wrapper owns the gap above the picker so a press there is also
        // swallowed: it would otherwise focus the popup root, blur the hosted
        // modal picker and dismiss it.
        div()
            .pb_1()
            .debug_selector(|| TAB_ROW_SELECTOR.to_string())
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .child(
                h_flex()
                    .gap_1()
                    .p_1()
                    .elevation_3(cx)
                    .children(SearchTab::ALL.into_iter().map(|tab| {
                        ButtonLike::new(tab.label())
                            .toggle_state(tab == self.tab)
                            .child(Label::new(tab.label()).size(LabelSize::Small).color(
                                if tab == self.tab {
                                    Color::Default
                                } else {
                                    Color::Muted
                                },
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_tab(tab, window, cx);
                            }))
                    })),
            )
    }
}

impl Render for SearchEverywhereModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("SearchEverywhere")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| {
                // The hosted picker handles Esc itself once it exists; before the
                // file finder has loaded nothing else would close the popup.
                cx.emit(DismissEvent);
            }))
            .on_action(cx.listener(|this, _: &NextSearchTab, window, cx| {
                this.select_tab(this.tab.next(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousSearchTab, window, cx| {
                this.select_tab(this.tab.previous(), window, cx);
            }))
            .child(self.render_tab_row(cx))
            .children(self.child.as_ref().map(|child| child.view.clone()))
    }
}
