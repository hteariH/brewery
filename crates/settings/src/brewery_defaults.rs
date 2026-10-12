//! Guards Brewery's cloud-off defaults against upstream merges.

use crate::default_settings;
use serde_json::Value;

fn defaults() -> Value {
    settings_json::parse_json_with_comments::<Value>(&default_settings())
        .expect("assets/settings/default.json parses")
}

#[test]
fn test_brewery_cloud_defaults_are_off() {
    let d = defaults();
    let off = Value::Bool(false);
    assert_eq!(d["telemetry"]["diagnostics"], off);
    assert_eq!(d["telemetry"]["metrics"], off);
    assert_eq!(d["disable_ai"], Value::Bool(true));
    assert_eq!(d["auto_update"], off);
    assert_eq!(d["collaboration_panel"]["button"], off);
    assert_eq!(d["title_bar"]["show_sign_in"], off);
    assert_eq!(d["title_bar"]["show_user_menu"], off);
    assert_eq!(d["title_bar"]["show_user_picture"], off);
    assert_eq!(d["title_bar"]["show_onboarding_banner"], off);
}

#[test]
fn test_brewery_fonts_are_default() {
    let d = defaults();
    assert_eq!(d["buffer_font_family"], Value::String("JetBrains Mono".into()));
    assert_eq!(d["ui_font_family"], Value::String("Inter".into()));
}

#[test]
fn test_brewery_theme_is_default() {
    let d = defaults();
    assert_eq!(d["theme"]["mode"], Value::String("system".into()));
    assert_eq!(d["theme"]["dark"], Value::String("Brewery Dark".into()));
    assert_eq!(d["theme"]["light"], Value::String("Brewery Light".into()));
}

#[test]
fn test_brewery_icon_theme_is_default() {
    let d = defaults();
    assert_eq!(d["icon_theme"]["dark"], Value::String("Brewery Icons Dark".into()));
    assert_eq!(d["icon_theme"]["light"], Value::String("Brewery Icons Light".into()));
}

#[test]
fn test_jetbrains_keymap_is_default() {
    let d = defaults();
    assert_eq!(d["base_keymap"], Value::String("JetBrains".into()));
}

#[test]
fn test_brewery_theme_selection_default_is_brewery() {
    // The Settings UI inserts `ThemeSelection::default()` when the user has no
    // theme key yet, so it must not fall back to Zed's One Light/One Dark.
    match crate::ThemeSelection::default() {
        crate::ThemeSelection::Dynamic { light, dark, .. } => {
            assert_eq!(light.0.as_ref(), "Brewery Light");
            assert_eq!(dark.0.as_ref(), "Brewery Dark");
        }
        other => panic!("expected dynamic theme selection, got {other:?}"),
    }
}

#[test]
fn test_intellij_tool_window_placement() {
    let d = defaults();
    for panel in ["project_panel", "outline_panel", "git_panel"] {
        assert_eq!(d[panel]["dock"], Value::String("left".into()), "{panel}");
    }
    for panel in ["terminal", "debugger"] {
        assert_eq!(d[panel]["dock"], Value::String("bottom".into()), "{panel}");
    }
}

#[test]
fn test_brewery_first_run_files_parse() {
    // Their headers were rebranded by hand, so make sure they still load as JSONC.
    settings_json::parse_json_with_comments::<Value>(&crate::initial_user_settings_content())
        .expect("initial_user_settings.json parses");
    settings_json::parse_json_with_comments::<Value>(&crate::initial_keymap_content())
        .expect("keymaps/initial.json parses");
    settings_json::parse_json_with_comments::<Value>(&crate::initial_project_settings_content())
        .expect("initial_local_settings.json parses");
}

#[test]
fn test_java_support_is_preinstalled() {
    let d = defaults();
    assert_eq!(d["auto_install_extensions"]["java"], Value::Bool(true));
    assert_eq!(d["lsp"]["jdtls"]["settings"]["jdk_auto_download"], Value::Bool(true));
}

#[test]
fn test_sql_highlighting_is_preinstalled() {
    // The query console of the Database tool window takes its "SQL" language from this extension.
    let d = defaults();
    assert_eq!(d["auto_install_extensions"]["sql"], Value::Bool(true));
}
