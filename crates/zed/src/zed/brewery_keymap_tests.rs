//! Brewery's JetBrains bindings resolved the way Zed layers keymaps: the platform default
//! keymap, the JetBrains base keymap, then the platform's specific overrides. Each platform's
//! files are checked on every host, since the assets are the same everywhere.

use gpui::TestAppContext;

struct Platform {
    name: &'static str,
    default: &'static str,
    base: &'static str,
    overrides: &'static str,
    /// The primary modifier of the IntelliJ shortcut (ctrl on Linux and Windows, cmd on macOS).
    primary: &'static str,
    /// The modifier of the tool-window shortcuts (alt on Linux and Windows, cmd on macOS).
    tool_window: &'static str,
}

const PLATFORMS: [Platform; 3] = [
    Platform {
        name: "Linux",
        default: "keymaps/default-linux.json",
        base: "keymaps/linux/jetbrains.json",
        overrides: "keymaps/specific-overrides.json",
        primary: "ctrl",
        tool_window: "alt",
    },
    Platform {
        name: "Windows",
        default: "keymaps/default-windows.json",
        base: "keymaps/linux/jetbrains.json",
        overrides: "keymaps/specific-overrides.json",
        primary: "ctrl",
        tool_window: "alt",
    },
    Platform {
        name: "macOS",
        default: "keymaps/default-macos.json",
        base: "keymaps/macos/jetbrains.json",
        overrides: "keymaps/specific-overrides-macos.json",
        primary: "cmd",
        tool_window: "cmd",
    },
];

/// The actions `platform`'s keymaps resolve for `keystroke` along `contexts` (root first), most
/// specific binding first.
fn bindings_for(
    platform: &Platform,
    keystroke: &str,
    contexts: &[&str],
    cx: &mut TestAppContext,
) -> Vec<String> {
    cx.update(|cx| {
        let mut bindings = Vec::new();
        for (path, source) in [
            (platform.default, settings::KeybindSource::Default),
            (platform.base, settings::KeybindSource::Base),
            (platform.overrides, settings::KeybindSource::Base),
        ] {
            let mut loaded =
                settings::KeymapFile::load_asset_allow_partial_failure(path, cx).unwrap();
            for binding in &mut loaded {
                binding.set_meta(source.meta());
            }
            bindings.extend(loaded);
        }
        let contexts = contexts
            .iter()
            .map(|context| gpui::KeyContext::parse(context).unwrap())
            .collect::<Vec<_>>();
        gpui::Keymap::new(bindings)
            .bindings_for_input(&[gpui::Keystroke::parse(keystroke).unwrap()], &contexts)
            .0
            .iter()
            .map(|binding| binding.action().name().to_string())
            .collect()
    })
}

#[track_caller]
fn assert_first(
    platform: &Platform,
    keystroke: &str,
    contexts: &[&str],
    expected: &str,
    cx: &mut TestAppContext,
) {
    let bindings = bindings_for(platform, keystroke, contexts, cx);
    assert_eq!(
        bindings.first().map(String::as_str),
        Some(expected),
        "{}: {keystroke} in {contexts:?} resolves to {bindings:?}",
        platform.name
    );
}

const IN_EDITOR: [&str; 3] = ["Workspace", "Pane", "Editor"];

/// Ctrl+K / Cmd+K opens the Commit tool window and, inside it, focuses the commit message;
/// Alt+0 / Cmd+0 toggles the tool window.
#[gpui::test]
fn commit_tool_window_keys(cx: &mut TestAppContext) {
    let in_panel = ["Workspace", "Dock", "GitPanel", "ChangesList"];
    for platform in &PLATFORMS {
        let commit = format!("{}-k", platform.primary);
        let toggle = format!("{}-0", platform.tool_window);
        assert_first(platform, &commit, &in_panel, "git_panel::FocusEditor", cx);
        assert_first(platform, &commit, &IN_EDITOR, "git_panel::ToggleFocus", cx);
        assert_first(platform, &toggle, &IN_EDITOR, "git_panel::ToggleFocus", cx);
        assert_first(platform, &toggle, &in_panel, "workspace::CloseActiveDock", cx);
    }
}

/// Ctrl+~ / Cmd+~ opens the branch popup, as IntelliJ's Branches.
#[gpui::test]
fn branches_key(cx: &mut TestAppContext) {
    for platform in &PLATFORMS {
        let key = format!("{}-~", platform.primary);
        assert_first(platform, &key, &IN_EDITOR, "git::Branch", cx);
    }
}

/// The query console's run and cancel keys beat the base keymaps' `Editor` bindings inside the
/// console's editor.
#[gpui::test]
fn query_console_keys(cx: &mut TestAppContext) {
    for platform in &PLATFORMS {
        let run = format!("{}-enter", platform.primary);
        let cancel = format!("{}-f2", platform.primary);
        for contexts in [
            &["Workspace", "Pane", "QueryConsole", "Editor"][..],
            &["Workspace", "Pane", "QueryConsole"][..],
        ] {
            assert_first(platform, &run, contexts, "brewery::RunStatement", cx);
            assert_first(platform, &cancel, contexts, "brewery::CancelStatement", cx);
        }
    }
}
