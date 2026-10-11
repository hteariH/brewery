//! IntelliJ-style Run/Debug controls for the title bar.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{Action, AnyElement, App, SharedString, Window};
use ui::{Button, ButtonStyle, Icon, IconButton, IconName, IconSize, Tooltip, prelude::*};

/// Label of the button that opens the task picker.
const PICKER_LABEL: &str = "Run…";

/// The widget's buttons in display order: label (also the tooltip) and the action each dispatches.
///
/// Debugger actions are looked up by name because `debugger_ui` is not a dependency of the title
/// bar; if the debugger is not registered the buttons are omitted rather than shown dead.
pub fn widget_actions(cx: &App) -> Vec<(SharedString, Box<dyn Action>)> {
    let mut actions: Vec<(SharedString, Box<dyn Action>)> = vec![
        (PICKER_LABEL.into(), Box::new(zed_actions::Spawn::modal())),
        ("Run".into(), Box::new(zed_actions::Rerun::default())),
    ];
    // The title bar renders this every frame; log the omission only once.
    static LOGGED_UNAVAILABLE: AtomicBool = AtomicBool::new(false);
    for (label, name) in [("Debug", "debugger::Start"), ("Stop", "debugger::Stop")] {
        match cx.build_action(name, None) {
            Ok(action) => actions.push((label.into(), action)),
            Err(error) => {
                if !LOGGED_UNAVAILABLE.swap(true, Ordering::Relaxed) {
                    log::debug!("Run widget: `{name}` unavailable, omitting: {error}");
                }
            }
        }
    }
    actions
}

fn icon_for(label: &str) -> IconName {
    match label {
        "Debug" => IconName::Debug,
        "Stop" => IconName::Stop,
        _ => IconName::PlayFilled,
    }
}

pub fn render_run_widget(_window: &mut Window, cx: &mut App) -> AnyElement {
    let mut row = h_flex().gap_0p5();
    for (index, (label, action)) in widget_actions(cx).into_iter().enumerate() {
        let id = SharedString::from(format!("brewery-run-widget-{index}"));
        if label == PICKER_LABEL {
            let tooltip_action = action.boxed_clone();
            row = row.child(
                Button::new(id, label.clone())
                    .style(ButtonStyle::Subtle)
                    .label_size(LabelSize::Small)
                    .end_icon(Icon::new(IconName::ChevronDown).size(IconSize::XSmall))
                    .tooltip(move |_, cx| {
                        Tooltip::for_action("Select task to run", &*tooltip_action, cx)
                    })
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(action.boxed_clone(), cx)
                    }),
            );
        } else {
            let tooltip_action = action.boxed_clone();
            row = row.child(
                IconButton::new(id, icon_for(&label))
                    .icon_size(IconSize::Small)
                    .tooltip(move |_, cx| Tooltip::for_action(label.clone(), &*tooltip_action, cx))
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(action.boxed_clone(), cx)
                    }),
            );
        }
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    // Stand-ins for `debugger_ui`'s actions, which this crate must not depend on.
    gpui::actions!(debugger, [Start, Stop]);

    #[gpui::test]
    fn widget_lists_run_and_debug_actions(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let actions = widget_actions(cx);
            let labels: Vec<_> = actions.iter().map(|(label, _)| label.to_string()).collect();
            let names: Vec<_> = actions.iter().map(|(_, action)| action.name()).collect();
            assert_eq!(labels, ["Run…", "Run", "Debug", "Stop"]);
            assert_eq!(
                names,
                [
                    "task::Spawn",
                    "task::Rerun",
                    "debugger::Start",
                    "debugger::Stop"
                ]
            );
        });
    }
}
