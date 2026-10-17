use ratatui::style::Color;
use serde::Deserialize;

/// Speaker roles recognized by the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    User,
    Cursor,
    Agy,
    Other,
    System,
    /// A `[[agents.custom]]` agent; colored by its `color` setting.
    Custom(u8),
}

/// Runtime theme colors for each speaker role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub user: Color,
    pub cursor: Color,
    pub agy: Color,
    pub other: Color,
    pub system: Color,
}

impl Default for Theme {
    /// Default theme colors.
    fn default() -> Self {
        Self {
            user: Color::Cyan,
            cursor: Color::Green,
            agy: Color::Magenta,
            other: Color::White,
            system: Color::Gray,
        }
    }
}

/// Serialized theme colors as they appear in config TOML files.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct ThemeToml {
    pub user: Option<String>,
    pub cursor: Option<String>,
    pub agy: Option<String>,
    pub other: Option<String>,
    pub system: Option<String>,
    /// Force colored output even when `NO_COLOR` is set.
    pub force_color: Option<bool>,
}

impl Theme {
    /// Apply serialized TOML colors onto this theme, keeping current colors for any
    /// missing or invalid color name.
    pub fn apply_toml(&mut self, t: &ThemeToml) {
        if let Some(s) = &t.user
            && let Some(c) = parse_color(s)
        {
            self.user = c;
        }
        if let Some(s) = &t.cursor
            && let Some(c) = parse_color(s)
        {
            self.cursor = c;
        }
        if let Some(s) = &t.agy
            && let Some(c) = parse_color(s)
        {
            self.agy = c;
        }
        if let Some(s) = &t.other
            && let Some(c) = parse_color(s)
        {
            self.other = c;
        }
        if let Some(s) = &t.system
            && let Some(c) = parse_color(s)
        {
            self.system = c;
        }
    }

    /// Build a theme from a serialized TOML mapping, keeping defaults for any
    /// missing or invalid color name.
    pub fn from_toml(t: &ThemeToml) -> Self {
        let mut theme = Self::default();
        theme.apply_toml(t);
        theme
    }

    /// Return the color for a speaker role.
    pub fn color_for(&self, s: Speaker) -> Color {
        match s {
            Speaker::User => self.user,
            Speaker::Cursor => self.cursor,
            Speaker::Agy => self.agy,
            Speaker::Other => self.other,
            Speaker::System => self.system,
            Speaker::Custom(index) => crate::custom_agent::spec_at(index)
                .and_then(|spec| spec.color.as_deref())
                .and_then(parse_color)
                .unwrap_or(self.other),
        }
    }
}

/// Parse a color from either a named color or a `#rrggbb` hex string.
pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }

    // Normalize case and separators: lowercase, and drop any '-', '_' or
    // spaces so "light-red", "light_red", "light red" and "lightred" all match.
    let mut normalized = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '-' | '_' | ' ') {
            continue;
        }
        normalized.push(ch.to_ascii_lowercase());
    }

    match normalized.as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" => Some(Color::Gray),
        "darkgray" => Some(Color::DarkGray),
        "lightred" => Some(Color::LightRed),
        "lightgreen" => Some(Color::LightGreen),
        "lightyellow" => Some(Color::LightYellow),
        "lightblue" => Some(Color::LightBlue),
        "lightmagenta" => Some(Color::LightMagenta),
        "lightcyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => None,
    }
}

fn parse_hex(hex: &str) -> Option<Color> {
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_color_named() {
        assert_eq!(parse_color("black"), Some(Color::Black));
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("white"), Some(Color::White));
        assert_eq!(parse_color("dark-gray"), Some(Color::DarkGray));
        assert_eq!(parse_color("DARK_GRAY"), Some(Color::DarkGray));
        assert_eq!(parse_color("light red"), Some(Color::LightRed));
        assert_eq!(parse_color("LightMagenta"), Some(Color::LightMagenta));
        assert_eq!(parse_color("light-cyan"), Some(Color::LightCyan));
        assert_eq!(parse_color("  LIGHT-BLUE  "), Some(Color::LightBlue));
        assert_eq!(parse_color("chartreuse"), None);
        assert_eq!(parse_color(""), None);
    }

    #[test]
    fn parse_color_hex() {
        assert_eq!(parse_color("#ff0000"), Some(Color::Rgb(255, 0, 0)));
        assert_eq!(parse_color("#a1b2c3"), Some(Color::Rgb(0xa1, 0xb2, 0xc3)));
        assert_eq!(parse_color("#A1B2C3"), Some(Color::Rgb(0xa1, 0xb2, 0xc3)));
        assert_eq!(parse_color("#a1b2c"), None);
        assert_eq!(parse_color("#a1b2cg"), None);
        assert_eq!(parse_color("a1b2c3"), None);
    }

    #[test]
    fn from_toml_defaults_and_overrides() {
        let theme = Theme::from_toml(&ThemeToml::default());
        assert_eq!(theme.user, Color::Cyan);
        assert_eq!(theme.cursor, Color::Green);
        assert_eq!(theme.agy, Color::Magenta);
        assert_eq!(theme.other, Color::White);
        assert_eq!(theme.system, Color::Gray);

        let parsed: ThemeToml = toml::from_str(
            r##"
            user = "yellow"
            cursor = "#112233"
            agy = "not-a-color"
            other = "light-magenta"
            system = "blue"
            unknown_field = "ignored"
        "##,
        )
        .unwrap();
        let theme = Theme::from_toml(&parsed);
        assert_eq!(theme.user, Color::Yellow);
        assert_eq!(theme.cursor, Color::Rgb(0x11, 0x22, 0x33));
        assert_eq!(theme.agy, Color::Magenta);
        assert_eq!(theme.other, Color::LightMagenta);
        assert_eq!(theme.system, Color::Blue);
    }

    #[test]
    fn color_for_maps_each_speaker() {
        let theme = Theme::default();
        assert_eq!(theme.color_for(Speaker::User), theme.user);
        assert_eq!(theme.color_for(Speaker::Cursor), theme.cursor);
        assert_eq!(theme.color_for(Speaker::Agy), theme.agy);
        assert_eq!(theme.color_for(Speaker::Other), theme.other);
        assert_eq!(theme.color_for(Speaker::System), theme.system);

        let custom = Theme {
            user: Color::Red,
            cursor: Color::Yellow,
            agy: Color::Blue,
            other: Color::Black,
            system: Color::Magenta,
        };
        assert_eq!(custom.color_for(Speaker::User), Color::Red);
        assert_eq!(custom.color_for(Speaker::Cursor), Color::Yellow);
        assert_eq!(custom.color_for(Speaker::Agy), Color::Blue);
        assert_eq!(custom.color_for(Speaker::Other), Color::Black);
        assert_eq!(custom.color_for(Speaker::System), Color::Magenta);
    }
}

/// Decide whether chat colors should be emitted.
///
/// Crossterm drops every color when `NO_COLOR` is set (non-empty), which made
/// the speaker theme invisible. `DMUXAI_FORCE_COLOR=1` or `[theme]
/// force_color = true` re-enables colors; `DMUXAI_FORCE_COLOR=0` disables them.
pub fn resolve_color_output(
    no_color: Option<&str>,
    force_env: Option<&str>,
    force_config: Option<bool>,
) -> bool {
    if let Some(v) = force_env {
        match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => return true,
            "0" | "false" | "no" | "off" => return false,
            _ => {}
        }
    }
    if force_config == Some(true) {
        return true;
    }
    !no_color.is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod color_output_tests {
    use super::resolve_color_output;

    #[test]
    fn no_color_disables_unless_forced() {
        assert!(resolve_color_output(None, None, None));
        assert!(!resolve_color_output(Some("1"), None, None));
        assert!(resolve_color_output(Some(""), None, None));
        assert!(resolve_color_output(Some("1"), Some("1"), None));
        assert!(resolve_color_output(Some("1"), None, Some(true)));
        assert!(!resolve_color_output(None, Some("0"), Some(true)));
    }
}
