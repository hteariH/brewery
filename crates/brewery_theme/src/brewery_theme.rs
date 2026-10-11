//! Brewery's look: the IntelliJ-style theme family and icon theme.

pub const DARK_THEME: &str = "Brewery Dark";
pub const LIGHT_THEME: &str = "Brewery Light";

use std::path::Path;

use anyhow::Context as _;
use gpui::App;
use rust_embed::RustEmbed;

pub const DARK_ICON_THEME: &str = "Brewery Icons Dark";
pub const LIGHT_ICON_THEME: &str = "Brewery Icons Light";

const ICON_THEME_JSON: &str = include_str!("../icons/icon_theme.json");

#[derive(RustEmbed)]
#[folder = "icons/"]
#[include = "dark/*.svg"]
#[include = "light/*.svg"]
struct BreweryIcons;

/// Installs the IntelliJ icons to disk and registers the Brewery icon themes.
/// Colour icons must be files: GPUI renders embedded `icons/...` SVGs as
/// single-colour masks.
pub fn init(cx: &mut App) {
    let root = paths::data_dir().join("brewery").join("icon_theme");
    if let Err(error) = install_icons(&root) {
        log::error!("failed to install Brewery icons: {error:#}");
        return;
    }
    let registry = theme::ThemeRegistry::global(cx);
    if let Err(error) = registry.load_icon_theme(
        &root.join("icon_theme.json"),
        &root,
        ICON_THEME_JSON.as_bytes().to_vec(),
    ) {
        log::error!("failed to load Brewery icon theme: {error:#}");
        return;
    }
    theme_settings::reload_icon_theme(cx);
}

fn install_icons(root: &Path) -> anyhow::Result<()> {
    for path in BreweryIcons::iter() {
        let file = BreweryIcons::get(&path).context("embedded icon vanished")?;
        let target = root.join(path.as_ref());
        if std::fs::read(&target).ok().as_deref() == Some(file.data.as_ref()) {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&target, &file.data).with_context(|| format!("writing {}", target.display()))?;
    }
    std::fs::write(root.join("icon_theme.json"), ICON_THEME_JSON).context("writing icon_theme.json")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEME_JSON: &str = include_str!("../../../assets/themes/brewery/brewery.json");

    #[test]
    fn test_default_settings_name_the_bundled_themes() {
        // assets/settings/default.json cannot reference these consts, so tie them here.
        let defaults = settings_json::parse_json_with_comments::<serde_json::Value>(include_str!(
            "../../../assets/settings/default.json"
        ))
        .expect("default.json parses");
        assert_eq!(defaults["theme"]["dark"], DARK_THEME);
        assert_eq!(defaults["theme"]["light"], LIGHT_THEME);
        assert_eq!(defaults["icon_theme"]["dark"], DARK_ICON_THEME);
        assert_eq!(defaults["icon_theme"]["light"], LIGHT_ICON_THEME);
    }

    #[test]
    fn test_brewery_theme_family_parses() {
        let family: theme_settings::ThemeFamilyContent =
            serde_json::from_str(THEME_JSON).expect("brewery.json is a valid theme family");
        let names: Vec<_> = family.themes.iter().map(|theme| theme.name.clone()).collect();
        assert_eq!(names, vec![DARK_THEME.to_string(), LIGHT_THEME.to_string()]);
        let refined = theme_settings::refine_theme_family(family);
        assert_eq!(refined.themes.len(), 2);
    }

    #[test]
    fn test_brewery_dark_uses_intellij_palette() {
        let family: serde_json::Value = serde_json::from_str(THEME_JSON).expect("valid JSON");
        let dark = &family["themes"][0]["style"];
        assert_eq!(dark["editor.background"], "#1e1f22ff");
        assert_eq!(dark["panel.background"], "#2b2d30ff");
        assert_eq!(dark["border.focused"], "#3574f0ff");
        assert_eq!(dark["syntax"]["keyword"]["color"], "#cf8e6dff");
        assert_eq!(dark["syntax"]["string"]["color"], "#6aab73ff");
        let light = &family["themes"][1]["style"];
        assert_eq!(light["editor.background"], "#ffffffff");
        assert_eq!(light["syntax"]["keyword"]["color"], "#0033b3ff");
    }

    #[test]
    fn test_border_is_visible_against_editor_terminal_and_panel() {
        let family: serde_json::Value = serde_json::from_str(THEME_JSON).expect("valid JSON");
        for theme in family["themes"].as_array().expect("themes is an array") {
            let style = &theme["style"];
            let name = theme["name"].as_str().unwrap_or("?");
            let border = style["border"].as_str().expect("border is a string");
            for surface in ["editor.background", "terminal.background", "panel.background"] {
                assert_ne!(
                    border, style[surface],
                    "{name}: border must differ from {surface} so docked panels stay separated"
                );
            }
        }
    }

    #[test]
    fn test_icon_theme_covers_every_zed_icon_key() {
        let family = theme::deserialize_icon_theme(ICON_THEME_JSON.as_bytes())
            .expect("icon_theme.json is valid");
        let zed_keys: Vec<String> = theme::default_icon_theme().file_icons.keys().cloned().collect();
        for icon_theme in &family.themes {
            for key in &zed_keys {
                assert!(
                    icon_theme.file_icons.contains_key(key),
                    "{} misses icon key {key}",
                    icon_theme.name
                );
            }
        }
    }

    #[test]
    fn test_every_icon_path_is_embedded() {
        let family = theme::deserialize_icon_theme(ICON_THEME_JSON.as_bytes())
            .expect("icon_theme.json is valid");
        for icon_theme in &family.themes {
            let mut paths: Vec<String> = icon_theme
                .file_icons
                .values()
                .map(|icon| icon.path.to_string())
                .collect();
            paths.extend(icon_theme.directory_icons.collapsed.iter().map(|p| p.to_string()));
            for path in paths {
                assert!(BreweryIcons::get(&path).is_some(), "{path} is not embedded");
            }
        }
    }

    #[test]
    fn test_icon_theme_names() {
        let family = theme::deserialize_icon_theme(ICON_THEME_JSON.as_bytes()).expect("valid");
        let names: Vec<_> = family.themes.iter().map(|t| t.name.clone()).collect();
        assert_eq!(names, vec![DARK_ICON_THEME.to_string(), LIGHT_ICON_THEME.to_string()]);
    }
}
