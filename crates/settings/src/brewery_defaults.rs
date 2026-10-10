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
