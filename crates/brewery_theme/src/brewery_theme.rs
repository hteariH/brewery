//! Brewery's look: the IntelliJ-style theme family and icon theme.

pub const DARK_THEME: &str = "Brewery Dark";
pub const LIGHT_THEME: &str = "Brewery Light";

#[cfg(test)]
mod tests {
    use super::*;

    const THEME_JSON: &str = include_str!("../../../assets/themes/brewery/brewery.json");

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
}
