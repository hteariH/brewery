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
