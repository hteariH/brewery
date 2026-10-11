//! IntelliJ "New UI" tool-window stripes: a narrow vertical column on each
//! side of the workspace holding one icon button per dock panel. They replace
//! Zed's panel buttons in the status bar.

use super::{Dock, DockPosition, PanelHandle};
use crate::Workspace;
use gpui::{
    Action, Anchor, AnyElement, App, Context, Div, Entity, FocusHandle, IntoElement, ParentElement,
    Render, SharedString, Styled, Subscription, Window, div, px,
};
use settings::SettingsStore;
use std::sync::Arc;
use ui::{ContextMenu, CountBadge, IconButton, IconName, Tooltip, prelude::*, right_click_menu};

const STRIPE_WIDTH: f32 = 40.;

/// Everything needed to draw one stripe button, computed from a dock's state.
pub(super) struct StripeButton {
    pub name: &'static str,
    pub icon: IconName,
    pub tooltip: SharedString,
    pub active: bool,
    pub action: Box<dyn Action>,
    pub icon_label: Option<String>,
    pub panel: Arc<dyn PanelHandle>,
}

/// One button per panel that has an icon, in dock order. A button is active
/// when its panel is the dock's visible panel; clicking an active button
/// closes the dock, any other button opens its panel.
pub(super) fn stripe_buttons(dock: &Dock, window: &Window, cx: &App) -> Vec<StripeButton> {
    dock.panel_entries
        .iter()
        .enumerate()
        .filter_map(|(ix, entry)| {
            let icon = entry.panel.icon(window, cx)?;
            let tooltip = entry.panel.icon_tooltip(window, cx)?;
            let active = dock.is_open && dock.active_panel_index == Some(ix);
            let action = if active {
                dock.toggle_action()
            } else {
                entry.panel.toggle_action(window, cx)
            };
            Some(StripeButton {
                name: entry.panel.persistent_name(),
                icon,
                tooltip: tooltip.into(),
                active,
                action,
                icon_label: entry.panel.icon_label(window, cx),
                panel: entry.panel.clone(),
            })
        })
        .collect()
}

/// A vertical stripe showing the panels of `top` at its top and, optionally,
/// the panels of `bottom` at its bottom (IntelliJ puts the bottom tool
/// windows on the lower part of the left stripe).
pub struct ToolWindowStripe {
    top: Entity<Dock>,
    bottom: Option<Entity<Dock>>,
    _subscriptions: Vec<Subscription>,
}

impl ToolWindowStripe {
    pub fn new(top: Entity<Dock>, bottom: Option<Entity<Dock>>, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![
            cx.observe(&top, |_, _, cx| cx.notify()),
            // Panels decide from settings whether they show a button, and a
            // settings change does not always notify the dock.
            cx.observe_global::<SettingsStore>(|_, cx| cx.notify()),
        ];
        if let Some(bottom) = &bottom {
            subscriptions.push(cx.observe(bottom, |_, _, cx| cx.notify()));
        }
        Self {
            top,
            bottom,
            _subscriptions: subscriptions,
        }
    }

    fn render_group(&self, dock: &Entity<Dock>, window: &Window, cx: &App) -> Div {
        let dock = dock.read(cx);
        let dock_position = dock.position;
        let focus_handle = dock.focus_handle.clone();
        // The context menu opens away from the window edge the stripe sits on.
        let (menu_anchor, menu_attach) = match self.top.read(cx).position {
            DockPosition::Right => (Anchor::TopRight, Anchor::TopLeft),
            DockPosition::Left | DockPosition::Bottom => (Anchor::TopLeft, Anchor::TopRight),
        };
        v_flex()
            .gap_1()
            .items_center()
            .children(stripe_buttons(dock, window, cx).into_iter().map(|button| {
                render_button(
                    button,
                    dock_position,
                    focus_handle.clone(),
                    menu_anchor,
                    menu_attach,
                )
            }))
    }
}

fn render_button(
    button: StripeButton,
    dock_position: DockPosition,
    focus_handle: FocusHandle,
    menu_anchor: Anchor,
    menu_attach: Anchor,
) -> AnyElement {
    let StripeButton {
        name,
        icon,
        tooltip,
        active,
        action,
        icon_label,
        panel,
    } = button;
    right_click_menu(name)
        .menu(move |window, cx| {
            let panel = panel.clone();
            ContextMenu::build(window, cx, move |mut menu, _, cx| {
                for position in [
                    DockPosition::Left,
                    DockPosition::Right,
                    DockPosition::Bottom,
                ] {
                    if !panel.position_is_valid(position, cx) {
                        continue;
                    }
                    let is_current = position == dock_position;
                    let panel = panel.clone();
                    menu = menu.toggleable_entry(
                        format!("Dock {}", position.label()),
                        is_current,
                        IconPosition::Start,
                        None,
                        move |window, cx| {
                            if !is_current {
                                panel.set_position(position, window, cx);
                            }
                        },
                    );
                }
                menu
            })
        })
        .anchor(menu_anchor)
        .attach(menu_attach)
        .trigger(move |menu_open, _, _| {
            // The active state is part of the id so a cached tooltip does not
            // outlive a state change made from the keyboard.
            let button = IconButton::new((name, active as u64), icon)
                .tab_index(0isize)
                .icon_size(IconSize::Medium)
                .toggle_state(active)
                .aria_label(tooltip.clone())
                .on_click({
                    let action = action.boxed_clone();
                    move |_, window, cx| {
                        window.focus(&focus_handle, cx);
                        window.dispatch_action(action.boxed_clone(), cx)
                    }
                })
                .when(!menu_open, |this| {
                    this.tooltip(move |_, cx| Tooltip::for_action(tooltip.clone(), &*action, cx))
                });
            div().relative().child(button).when_some(
                icon_label
                    .filter(|_| !active)
                    .and_then(|label| label.parse::<usize>().ok()),
                |this, count| this.child(CountBadge::new(count)),
            )
        })
        .into_any_element()
}

impl Render for ToolWindowStripe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let top = self.render_group(&self.top, window, cx);
        let bottom = self
            .bottom
            .as_ref()
            .map(|bottom| self.render_group(bottom, window, cx));
        let on_left = self.top.read(cx).position != DockPosition::Right;
        let colors = cx.theme().colors();
        v_flex()
            .flex_none()
            .w(px(STRIPE_WIDTH))
            .h_full()
            .py_1()
            .items_center()
            .bg(colors.panel_background)
            .border_color(colors.border)
            // Same top and bottom rules as the workspace area the stripes flank.
            .border_t_1()
            .border_b_1()
            .map(|this| {
                if on_left {
                    this.border_r_1()
                } else {
                    this.border_l_1()
                }
            })
            .child(top)
            .child(div().flex_1())
            .children(bottom)
    }
}

impl Workspace {
    /// Places the tool-window stripes on both sides of the workspace area.
    ///
    /// The stripes sit outside the `workspace` element on purpose: its bounds
    /// drive dock resizing and the zoom overlay, which must not include them.
    pub(crate) fn brewery_with_stripes(&self, workspace_area: impl IntoElement) -> Div {
        div()
            .flex()
            .flex_row()
            .flex_1()
            .w_full()
            .min_h_0()
            .child(self.brewery_left_stripe.clone())
            .child(workspace_area)
            .child(self.brewery_right_stripe.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::{Panel, PanelEvent};
    use crate::{MultiWorkspace, tests::init_test};
    use fs::FakeFs;
    use gpui::{EventEmitter, Focusable, Pixels, TestAppContext, actions};
    use project::Project;

    actions!(brewery_stripes_test, [ToggleStripeTestPanel]);

    /// Two distinct panel types are needed because the stripe identifies a
    /// button by `persistent_name`, which is a per-type constant.
    struct StripeTestPanel<const N: u32> {
        focus_handle: FocusHandle,
        position: DockPosition,
    }

    impl<const N: u32> EventEmitter<PanelEvent> for StripeTestPanel<N> {}

    impl<const N: u32> Focusable for StripeTestPanel<N> {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.focus_handle.clone()
        }
    }

    impl<const N: u32> Render for StripeTestPanel<N> {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().track_focus(&self.focus_handle)
        }
    }

    impl<const N: u32> Panel for StripeTestPanel<N> {
        fn persistent_name() -> &'static str {
            match N {
                1 => "StripeFirst",
                2 => "StripeSecond",
                _ => "StripeThird",
            }
        }
        fn panel_key() -> &'static str {
            Self::persistent_name()
        }
        fn position(&self, _: &Window, _: &App) -> DockPosition {
            self.position
        }
        fn position_is_valid(&self, _: DockPosition) -> bool {
            true
        }
        fn set_position(&mut self, _: DockPosition, _: &mut Window, _: &mut Context<Self>) {}
        fn default_size(&self, _: &Window, _: &App) -> Pixels {
            px(300.)
        }
        fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
            Some(IconName::FileTree)
        }
        fn icon_tooltip(&self, _: &Window, _: &App) -> Option<&'static str> {
            Some(Self::persistent_name())
        }
        fn toggle_action(&self) -> Box<dyn Action> {
            ToggleStripeTestPanel.boxed_clone()
        }
        fn activation_priority(&self) -> u32 {
            N
        }
    }

    fn summary(buttons: &[StripeButton]) -> Vec<(&'static str, bool)> {
        buttons.iter().map(|b| (b.name, b.active)).collect()
    }

    #[gpui::test]
    async fn test_stripe_buttons_follow_left_dock(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        workspace.update_in(cx, |workspace, window, cx| {
            let first = cx.new(|cx| StripeTestPanel::<1> {
                focus_handle: cx.focus_handle(),
                position: DockPosition::Left,
            });
            let second = cx.new(|cx| StripeTestPanel::<2> {
                focus_handle: cx.focus_handle(),
                position: DockPosition::Left,
            });
            workspace.add_panel(first, window, cx);
            workspace.add_panel(second, window, cx);
            workspace.left_dock().update(cx, |dock, cx| {
                dock.activate_panel(1, window, cx);
                dock.set_open(true, window, cx);
            });

            let dock = workspace.left_dock().read(cx);
            assert_eq!(
                summary(&stripe_buttons(dock, window, cx)),
                vec![("StripeFirst", false), ("StripeSecond", true)]
            );
        });

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.left_dock().update(cx, |dock, cx| {
                dock.set_open(false, window, cx);
            });
            let dock = workspace.left_dock().read(cx);
            assert_eq!(
                summary(&stripe_buttons(dock, window, cx)),
                vec![("StripeFirst", false), ("StripeSecond", false)]
            );
        });
    }

    #[gpui::test]
    async fn test_stripe_buttons_follow_bottom_and_right_docks(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        workspace.update_in(cx, |workspace, window, cx| {
            let left = cx.new(|cx| StripeTestPanel::<1> {
                focus_handle: cx.focus_handle(),
                position: DockPosition::Left,
            });
            let bottom = cx.new(|cx| StripeTestPanel::<2> {
                focus_handle: cx.focus_handle(),
                position: DockPosition::Bottom,
            });
            let right = cx.new(|cx| StripeTestPanel::<3> {
                focus_handle: cx.focus_handle(),
                position: DockPosition::Right,
            });
            workspace.add_panel(left, window, cx);
            workspace.add_panel(bottom, window, cx);
            workspace.add_panel(right, window, cx);
            workspace.bottom_dock().update(cx, |dock, cx| {
                dock.activate_panel(0, window, cx);
                dock.set_open(true, window, cx);
            });

            // The left stripe's lower group is the bottom dock; the right
            // stripe shows only the right dock. Each reports its own panels.
            let bottom_dock = workspace.bottom_dock().read(cx);
            assert_eq!(
                summary(&stripe_buttons(bottom_dock, window, cx)),
                vec![("StripeSecond", true)]
            );
            let right_dock = workspace.right_dock().read(cx);
            assert_eq!(
                summary(&stripe_buttons(right_dock, window, cx)),
                vec![("StripeThird", false)]
            );
            let left_dock = workspace.left_dock().read(cx);
            assert_eq!(
                summary(&stripe_buttons(left_dock, window, cx)),
                vec![("StripeFirst", false)]
            );
        });
    }
}
