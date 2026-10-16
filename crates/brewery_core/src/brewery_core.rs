//! Brewery-wide switches and glue that sit next to Zed's own crates.

use std::any::TypeId;

use command_palette_hooks::CommandPaletteFilter;
use gpui::{App, Menu, MenuItem};

pub use brewery_cloud::ZED_CLOUD_ENABLED;

/// Command palette namespaces whose actions only work with a Zed account.
/// `auto_update` is not one: Brewery updates from its own GitHub Releases.
pub const HIDDEN_CLOUD_NAMESPACES: &[&str] = &["client", "collab", "collab_panel"];

/// Single actions hidden from the command palette, by name (their crates depend
/// on brewery_core). `auto_update::ViewReleaseNotesLocally` opens the same GitHub
/// release page as `auto_update::ViewReleaseNotes`, so "locally" would mislead.
pub const HIDDEN_ACTIONS: &[&str] = &["auto_update::ViewReleaseNotesLocally"];

/// Hides Zed-cloud actions and [`HIDDEN_ACTIONS`]. Call after `command_palette::init`.
pub fn init(cx: &mut App) {
    let hidden_actions: Vec<TypeId> = HIDDEN_ACTIONS
        .iter()
        .filter_map(|name| cx.build_action(name, None).ok())
        .map(|action| action.as_any().type_id())
        .collect();
    CommandPaletteFilter::update_global(cx, |filter, _| {
        for namespace in HIDDEN_CLOUD_NAMESPACES {
            filter.hide_namespace(namespace);
        }
        filter.hide_action_types(&hidden_actions);
    });
}

/// Brewery's own release version, from `crates/zed/BREWERY_VERSION` (one line,
/// semver). Zed's `AppVersion` stays what remote servers, extensions and LSP see.
pub const BREWERY_VERSION: &str = env!("BREWERY_VERSION");

/// `BREWERY_VERSION` as a parsed version.
pub fn brewery_version() -> semver::Version {
    semver::Version::parse(BREWERY_VERSION).unwrap_or_else(|_| semver::Version::new(0, 0, 0))
}

/// The global HTTP client's User-Agent, e.g. "Brewery/0.1.1 (windows; x86_64)".
/// No Zed-hosted service Brewery still talks to reads it: the extension gallery
/// (`crates/collab/src/api/extensions.rs`) takes versions as query parameters.
pub fn user_agent(brewery_version: &str, os: &str, arch: &str) -> String {
    format!("Brewery/{brewery_version} ({os}; {arch})")
}

/// Headline of the About window, e.g. "Brewery 0.1.0".
pub fn about_title(product_name: &str, brewery_version: &str) -> String {
    format!("{product_name} {brewery_version}")
}

/// Second About line crediting the upstream editor, e.g. "based on Zed 1.25.0".
pub fn based_on_zed(zed_version: &str) -> String {
    format!("based on Zed {zed_version}")
}

pub const REPOSITORY_URL: &str = "https://github.com/hteariH/brewery";

const REMOVED_MENU_ITEMS: &[&str] = &[
    "View Release Notes Locally",
    "Email Us...",
    "Zed Twitter",
    "Join the Team",
    "View Telemetry",
];

/// Turns Zed's app menus into Brewery's: Brewery names, no Zed-only
/// destinations, bug reports to Brewery's repository.
pub fn rebrand_menus(menus: Vec<Menu>) -> Vec<Menu> {
    menus.into_iter().map(rebrand_menu).collect()
}

fn rebrand_menu(mut menu: Menu) -> Menu {
    if menu.name.as_ref() == "Zed" {
        menu.name = "Brewery".into();
    }
    let items = std::mem::take(&mut menu.items);
    let mut rebranded: Vec<MenuItem> = Vec::with_capacity(items.len());
    for item in items {
        let item = match item {
            MenuItem::Submenu(submenu) => MenuItem::Submenu(rebrand_menu(submenu)),
            MenuItem::Action { ref name, .. } if REMOVED_MENU_ITEMS.contains(&name.as_ref()) => {
                continue;
            }
            MenuItem::Action {
                name,
                action,
                os_action,
                checked,
                disabled,
            } => {
                let (name, action) = match name.as_ref() {
                    "About Zed" => ("About Brewery".into(), action),
                    "Hide Zed" => ("Hide Brewery".into(), action),
                    "Quit Zed" => ("Quit Brewery".into(), action),
                    "Documentation" => ("Zed Documentation".into(), action),
                    "Zed Repository" => (
                        "Brewery Repository".into(),
                        open_browser(REPOSITORY_URL.to_string()),
                    ),
                    "File Bug Report..." | "Request Feature..." => (
                        name.clone(),
                        open_browser(format!("{REPOSITORY_URL}/issues/new")),
                    ),
                    _ => (name, action),
                };
                MenuItem::Action {
                    name,
                    action,
                    os_action,
                    checked,
                    disabled,
                }
            }
            other => other,
        };
        let is_separator = matches!(item, MenuItem::Separator);
        if is_separator
            && rebranded
                .last()
                .is_none_or(|last| matches!(last, MenuItem::Separator))
        {
            continue;
        }
        rebranded.push(item);
    }
    while matches!(rebranded.last(), Some(MenuItem::Separator)) {
        rebranded.pop();
    }
    menu.items = rebranded;
    menu
}

fn open_browser(url: String) -> Box<dyn gpui::Action> {
    Box::new(zed_actions::OpenBrowser { url: url.into() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use command_palette_hooks::CommandPaletteFilter;
    use gpui::{Menu, MenuItem, NoAction, TestAppContext};

    #[test]
    fn brewery_version_matches_file_and_parses() {
        let file = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zed/BREWERY_VERSION"
        ))
        .expect("BREWERY_VERSION file");
        assert_eq!(file.lines().count(), 1, "one line only");
        assert_eq!(BREWERY_VERSION, file.trim());
        let parsed = semver::Version::parse(file.trim()).expect("semver");
        assert_eq!(brewery_version(), parsed);
        // A release (0.1.0) or a pre-release (0.1.0-rc.1); no build metadata, which the
        // updater's semver comparison and the package versions would ignore.
        assert!(parsed.build.is_empty(), "no +build metadata: {parsed}");
        assert_ne!(parsed, semver::Version::new(0, 0, 0));
    }

    #[test]
    fn user_agent_names_brewery() {
        assert_eq!(
            user_agent("0.1.1", "windows", "x86_64"),
            "Brewery/0.1.1 (windows; x86_64)"
        );
    }

    #[test]
    fn about_text() {
        assert_eq!(about_title("Brewery", "0.1.0"), "Brewery 0.1.0");
        assert_eq!(based_on_zed("1.25.0"), "based on Zed 1.25.0");
    }

    /// Stand-ins for `auto_update::Check` and auto_update_ui's
    /// `auto_update::ViewReleaseNotesLocally` (brewery_core cannot depend on
    /// either crate, which depend on it); the filter finds actions by name.
    mod update_actions {
        gpui::actions!(auto_update, [Check, ViewReleaseNotesLocally]);
    }

    fn action(name: &str) -> MenuItem {
        MenuItem::action(name.to_string(), NoAction)
    }

    fn names(menu: &Menu) -> Vec<String> {
        menu.items
            .iter()
            .map(|item| match item {
                MenuItem::Action { name, .. } => name.to_string(),
                MenuItem::Separator => "---".to_string(),
                MenuItem::Submenu(menu) => format!("> {}", menu.name),
                MenuItem::SystemMenu(_) => "system".to_string(),
            })
            .collect()
    }

    #[test]
    fn test_rebrand_menus() {
        let menus = vec![
            Menu::new("Zed").items([
                action("About Zed"),
                action("Check for Updates"),
                MenuItem::separator(),
                action("Quit Zed"),
            ]),
            Menu::new("Help").items([
                action("View Release Notes Locally"),
                action("View Telemetry"),
                action("Show Welcome"),
                MenuItem::separator(),
                action("File Bug Report..."),
                action("Email Us..."),
                MenuItem::separator(),
                action("Documentation"),
                action("Zed Repository"),
                action("Zed Twitter"),
                action("Join the Team"),
            ]),
        ];
        let menus = rebrand_menus(menus);
        assert_eq!(menus[0].name.as_ref(), "Brewery");
        // Check for Updates stays: Brewery updates from its own GitHub Releases.
        assert_eq!(
            names(&menus[0]),
            vec!["About Brewery", "Check for Updates", "---", "Quit Brewery"]
        );
        assert_eq!(
            names(&menus[1]),
            vec![
                "Show Welcome",
                "---",
                "File Bug Report...",
                "---",
                "Zed Documentation",
                "Brewery Repository"
            ]
        );
        let MenuItem::Action { action, .. } = &menus[1].items[5] else {
            panic!("not an action")
        };
        let open = action
            .as_any()
            .downcast_ref::<zed_actions::OpenBrowser>()
            .expect("repository item opens a browser");
        assert_eq!(open.url.as_ref(), REPOSITORY_URL);
    }

    #[gpui::test]
    fn test_cloud_actions_are_hidden(cx: &mut TestAppContext) {
        cx.update(|cx| {
            command_palette_hooks::init(cx);
            init(cx);
            let filter = CommandPaletteFilter::try_global(cx).expect("filter installed");
            assert!(filter.is_hidden(&client::SignIn));
            assert!(filter.is_hidden(&client::SignOut));
            assert!(!filter.is_hidden(&menu::Confirm));
            // Updates come from Brewery's GitHub Releases, so `auto_update: check` stays.
            assert!(!filter.is_hidden(&update_actions::Check));
            assert!(!HIDDEN_CLOUD_NAMESPACES.contains(&"auto_update"));
            // It opens the same GitHub release page as "auto_update: view release notes".
            assert!(filter.is_hidden(&update_actions::ViewReleaseNotesLocally));
        });
    }

    #[test]
    fn test_zed_cloud_is_disabled() {
        assert!(!ZED_CLOUD_ENABLED);
    }
}
