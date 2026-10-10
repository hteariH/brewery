//! Brewery-wide switches and glue that sit next to Zed's own crates.

use command_palette_hooks::CommandPaletteFilter;
use gpui::App;

/// Brewery never talks to Zed's cloud: no Zed account sign-in, collaboration,
/// Zed AI, telemetry upload or Zed update checks. Hook-ins in Zed's crates
/// check this instead of each carrying its own switch.
pub const ZED_CLOUD_ENABLED: bool = false;

/// Command palette namespaces whose actions only work with a Zed account.
pub const HIDDEN_CLOUD_NAMESPACES: &[&str] = &["client", "collab", "collab_panel", "auto_update"];

/// Hides Zed-cloud actions. Call after `command_palette::init`.
pub fn init(cx: &mut App) {
    CommandPaletteFilter::update_global(cx, |filter, _| {
        for namespace in HIDDEN_CLOUD_NAMESPACES {
            filter.hide_namespace(namespace);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use command_palette_hooks::CommandPaletteFilter;
    use gpui::TestAppContext;

    #[gpui::test]
    fn test_cloud_actions_are_hidden(cx: &mut TestAppContext) {
        cx.update(|cx| {
            command_palette_hooks::init(cx);
            init(cx);
            let filter = CommandPaletteFilter::try_global(cx).expect("filter installed");
            assert!(filter.is_hidden(&client::SignIn));
            assert!(filter.is_hidden(&client::SignOut));
            assert!(!filter.is_hidden(&menu::Confirm));
        });
    }

    #[test]
    fn test_zed_cloud_is_disabled() {
        assert!(!ZED_CLOUD_ENABLED);
    }
}
