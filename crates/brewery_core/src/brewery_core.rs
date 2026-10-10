//! Brewery-wide switches and glue that sit next to Zed's own crates.

use command_palette_hooks::CommandPaletteFilter;
use gpui::{App, Menu, MenuItem};

pub use brewery_cloud::ZED_CLOUD_ENABLED;

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

pub const REPOSITORY_URL: &str = "https://github.com/hteariH/brewery";

const REMOVED_MENU_ITEMS: &[&str] = &[
    "Check for Updates",
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
        assert_eq!(
            names(&menus[0]),
            vec!["About Brewery", "---", "Quit Brewery"]
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
        });
    }

    #[test]
    fn test_zed_cloud_is_disabled() {
        assert!(!ZED_CLOUD_ENABLED);
    }
}
