use std::collections::BTreeMap;

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub user: Color,
    pub cursor: Color,
    pub agy: Color,
    pub other: Color,
    pub system: Color,
    /// Conversation/prompt area background. `None` keeps the terminal default.
    pub background: Option<Color>,
    /// Default text color. `None` keeps the terminal default.
    pub foreground: Option<Color>,
    /// Block border color. `None` keeps the terminal default.
    pub border: Option<Color>,
    /// Highlight/accent color. `None` keeps the terminal default.
    pub accent: Option<Color>,
    /// Per-agent text colors from `[theme.agents]`; keys are lowercase and trimmed.
    pub agents: BTreeMap<String, Color>,
    /// Reduce motion: static spinner indicator.
    pub reduced_motion: bool,
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
            background: None,
            foreground: None,
            border: None,
            accent: None,
            agents: BTreeMap::new(),
            reduced_motion: false,
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
    pub background: Option<String>,
    pub foreground: Option<String>,
    pub border: Option<String>,
    pub accent: Option<String>,
    /// `[theme.agents]` table mapping agent ids to colors.
    pub agents: Option<BTreeMap<String, String>>,
    /// Force colored output even when `NO_COLOR` is set.
    pub force_color: Option<bool>,
    /// Reduce motion: static spinner indicator.
    pub reduced_motion: Option<bool>,
}

impl Theme {
    /// Apply serialized TOML colors onto this theme, keeping current colors for any
    /// missing or invalid color name.
    pub fn apply_toml(&mut self, t: &ThemeToml) {
        let mut warnings = Vec::new();
        self.apply_toml_with_warnings(t, &mut warnings);
    }

    /// Like [`Theme::apply_toml`], but records a warning for every invalid color
    /// instead of silently keeping the default.
    pub fn apply_toml_with_warnings(&mut self, t: &ThemeToml, warnings: &mut Vec<String>) {
        if let Some(rm) = t.reduced_motion {
            self.reduced_motion = rm;
        }
        apply_color(&mut self.user, "[theme] user", &t.user, warnings);
        apply_color(&mut self.cursor, "[theme] cursor", &t.cursor, warnings);
        apply_color(&mut self.agy, "[theme] agy", &t.agy, warnings);
        apply_color(&mut self.other, "[theme] other", &t.other, warnings);
        apply_color(&mut self.system, "[theme] system", &t.system, warnings);
        self.background = apply_optional_color(
            self.background,
            "[theme] background",
            &t.background,
            warnings,
        );
        self.foreground = apply_optional_color(
            self.foreground,
            "[theme] foreground",
            &t.foreground,
            warnings,
        );
        self.border = apply_optional_color(self.border, "[theme] border", &t.border, warnings);
        self.accent = apply_optional_color(self.accent, "[theme] accent", &t.accent, warnings);
        if let Some(agents) = &t.agents {
            for (id, value) in agents {
                let key = id.trim().to_ascii_lowercase();
                if key.is_empty() {
                    warnings.push(format!(
                        "[theme.agents] \"{id}\" = \"{value}\" has an empty agent id; ignoring"
                    ));
                    continue;
                }
                match parse_color(value) {
                    Some(color) => {
                        self.agents.insert(key, color);
                    }
                    None => warnings.push(format!(
                        "[theme.agents] {id} = \"{value}\" is not a valid color; using the fallback"
                    )),
                }
            }
        }
    }

    /// Build a theme from a serialized TOML mapping, keeping defaults for any
    /// missing or invalid color name.
    pub fn from_toml(t: &ThemeToml) -> Self {
        let mut theme = Self::default();
        theme.apply_toml(t);
        theme
    }

    /// Resolve the text color for an agent id.
    ///
    /// The first match wins: a `[theme.agents]` entry, then `fallback` (a custom
    /// agent's own `color`), then the built-in `cursor`/`agy` fields, then
    /// [`Theme::other`]. Ids match case-insensitively and ignore surrounding
    /// whitespace.
    pub fn agent_color(&self, agent_id: &str, fallback: Option<Color>) -> Color {
        let key = agent_id.trim().to_ascii_lowercase();
        if let Some(color) = self.agents.get(&key) {
            return *color;
        }
        if let Some(color) = fallback {
            return color;
        }
        match key.as_str() {
            "cursor" => self.cursor,
            "agy" => self.agy,
            _ => self.other,
        }
    }

    /// Return the `[theme.agents]` ids that are not in `known` (case-insensitive).
    pub fn unknown_agent_ids(&self, known: &[&str]) -> Vec<String> {
        self.agents
            .keys()
            .filter(|id| !known.iter().any(|k| k.eq_ignore_ascii_case(id)))
            .cloned()
            .collect()
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

/// Apply a required speaker color, keeping `current` and warning when invalid.
fn apply_color(current: &mut Color, key: &str, value: &Option<String>, warnings: &mut Vec<String>) {
    if let Some(v) = value {
        match parse_color(v) {
            Some(c) => *current = c,
            None => warnings.push(format!(
                "{key} = \"{v}\" is not a valid color; using the default"
            )),
        }
    }
}

/// Apply an optional surface color, keeping `current` and warning when invalid.
fn apply_optional_color(
    current: Option<Color>,
    key: &str,
    value: &Option<String>,
    warnings: &mut Vec<String>,
) -> Option<Color> {
    match value {
        Some(v) => match parse_color(v) {
            Some(c) => Some(c),
            None => {
                warnings.push(format!(
                    "{key} = \"{v}\" is not a valid color; using the default"
                ));
                current
            }
        },
        None => current,
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

/// Fixed sRGB bytes for a color, or `None` when it has no fixed value.
///
/// Named colors map to their ANSI palette values and [`Color::Rgb`] is passed
/// through; [`Color::Reset`] and indexed colors have no fixed value.
fn rgb_of(color: Color) -> Option<(u8, u8, u8)> {
    Some(match color {
        Color::Black => (0x00, 0x00, 0x00),
        Color::Red => (0x80, 0x00, 0x00),
        Color::Green => (0x00, 0x80, 0x00),
        Color::Yellow => (0x80, 0x80, 0x00),
        Color::Blue => (0x00, 0x00, 0x80),
        Color::Magenta => (0x80, 0x00, 0x80),
        Color::Cyan => (0x00, 0x80, 0x80),
        Color::Gray => (0xc0, 0xc0, 0xc0),
        Color::DarkGray => (0x80, 0x80, 0x80),
        Color::LightRed => (0xff, 0x00, 0x00),
        Color::LightGreen => (0x00, 0xff, 0x00),
        Color::LightYellow => (0xff, 0xff, 0x00),
        Color::LightBlue => (0x00, 0x00, 0xff),
        Color::LightMagenta => (0xff, 0x00, 0xff),
        Color::LightCyan => (0x00, 0xff, 0xff),
        Color::White => (0xff, 0xff, 0xff),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Reset | Color::Indexed(_) => return None,
    })
}

/// Render a color as a lowercase `#rrggbb` string.
///
/// Named colors map to fixed hex values and [`Color::Rgb`] is passed through;
/// [`Color::Reset`] and indexed colors have no fixed value and return `None`.
pub fn color_hex(color: Color) -> Option<String> {
    let (r, g, b) = rgb_of(color)?;
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

/// Linearize one sRGB channel for the WCAG relative-luminance formula.
fn srgb_linear(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance in `[0.0, 1.0]`.
fn relative_luminance((r, g, b): (u8, u8, u8)) -> f64 {
    0.2126 * srgb_linear(r) + 0.7152 * srgb_linear(g) + 0.0722 * srgb_linear(b)
}

/// Pick black or white text for the highest contrast against `bg`.
///
/// Uses the WCAG relative-luminance and contrast-ratio formulas with sRGB
/// linearization. Named colors are resolved via [`rgb_of`]; [`Color::Reset`]
/// and indexed colors have no fixed value and default to [`Color::White`].
pub fn text_on(bg: Color) -> Color {
    let Some((r, g, b)) = rgb_of(bg) else {
        return Color::White;
    };
    let luminance = relative_luminance((r, g, b));
    let on_black = (luminance + 0.05) / 0.05;
    let on_white = 1.05 / (luminance + 0.05);
    if on_black >= on_white {
        Color::Black
    } else {
        Color::White
    }
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
            ..Theme::default()
        };
        assert_eq!(custom.color_for(Speaker::User), Color::Red);
        assert_eq!(custom.color_for(Speaker::Cursor), Color::Yellow);
        assert_eq!(custom.color_for(Speaker::Agy), Color::Blue);
        assert_eq!(custom.color_for(Speaker::Other), Color::Black);
        assert_eq!(custom.color_for(Speaker::System), Color::Magenta);
    }

    #[test]
    fn theme_parses_surface_colors_with_warnings() {
        // TC-600, FR-600
        let parsed: ThemeToml = toml::from_str(
            r##"
            background = "#101010"
            foreground = "dark-gray"
            border = "#abcdef"
            accent = "bogus"
        "##,
        )
        .unwrap();
        let mut theme = Theme::default();
        let mut warnings = Vec::new();
        theme.apply_toml_with_warnings(&parsed, &mut warnings);

        assert_eq!(theme.background, Some(Color::Rgb(0x10, 0x10, 0x10)));
        assert_eq!(theme.foreground, Some(Color::DarkGray));
        assert_eq!(theme.border, Some(Color::Rgb(0xab, 0xcd, 0xef)));
        assert_eq!(theme.accent, None);
        assert!(
            warnings
                .iter()
                .any(|w| w == "[theme] accent = \"bogus\" is not a valid color; using the default"),
            "warnings: {warnings:?}"
        );
    }

    #[test]
    fn theme_agent_colors_resolve_with_fallbacks() {
        // TC-602, FR-601
        let parsed: ThemeToml = toml::from_str(
            r##"
            [agents]
            opencode-glm = "#ff8800"
            Codex = "magenta"
            unknown-agent = "cyan"
            broken = "not-a-color"
        "##,
        )
        .unwrap();
        let mut theme = Theme::default();
        let mut warnings = Vec::new();
        theme.apply_toml_with_warnings(&parsed, &mut warnings);

        assert_eq!(
            theme.agent_color("opencode-glm", None),
            Color::Rgb(0xff, 0x88, 0x00)
        );
        // Ids match case-insensitively and ignore whitespace.
        assert_eq!(
            theme.agent_color("  OPENCODE-GLM ", None),
            Color::Rgb(0xff, 0x88, 0x00)
        );
        assert_eq!(theme.agent_color("codex", None), Color::Magenta);
        // The custom-agent fallback beats the built-in fields.
        assert_eq!(theme.agent_color("cursor", Some(Color::Blue)), Color::Blue);
        // Built-in cursor/agy fields, then `other`.
        assert_eq!(theme.agent_color("cursor", None), theme.cursor);
        assert_eq!(theme.agent_color("agy", None), theme.agy);
        assert_eq!(theme.agent_color("opencode", None), theme.other);
        // The invalid entry is dropped with a warning naming the table.
        assert!(
            warnings.iter().any(|w| w
                == "[theme.agents] broken = \"not-a-color\" is not a valid color; using the fallback"),
            "warnings: {warnings:?}"
        );
        assert_eq!(
            theme.unknown_agent_ids(&["cursor", "agy", "codex"]),
            vec!["opencode-glm".to_owned(), "unknown-agent".to_owned()]
        );
    }

    #[test]
    fn color_hex_maps_named_and_rgb() {
        assert_eq!(color_hex(Color::Black).as_deref(), Some("#000000"));
        assert_eq!(color_hex(Color::Red).as_deref(), Some("#800000"));
        assert_eq!(color_hex(Color::White).as_deref(), Some("#ffffff"));
        assert_eq!(color_hex(Color::LightMagenta).as_deref(), Some("#ff00ff"));
        assert_eq!(
            color_hex(Color::Rgb(0x11, 0x22, 0x33)).as_deref(),
            Some("#112233")
        );
        assert_eq!(color_hex(Color::Reset), None);
        assert_eq!(color_hex(Color::Indexed(4)), None);
    }

    #[test]
    fn theme_agent_empty_id_warns() {
        // FR-601: blank/whitespace agent ids are reported, not silently dropped.
        let mut agents = BTreeMap::new();
        agents.insert("   ".to_owned(), "red".to_owned());
        let t = ThemeToml {
            agents: Some(agents),
            ..ThemeToml::default()
        };
        let mut theme = Theme::default();
        let mut warnings = Vec::new();
        theme.apply_toml_with_warnings(&t, &mut warnings);

        assert!(theme.agents.is_empty());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("has an empty agent id; ignoring")),
            "warnings: {warnings:?}"
        );
    }

    #[test]
    fn text_on_picks_highest_contrast() {
        // NFR-600: black on light backgrounds, white on dark backgrounds.
        assert_eq!(text_on(Color::Black), Color::White);
        assert_eq!(text_on(Color::White), Color::Black);
        assert_eq!(text_on(Color::Yellow), Color::Black);
        assert_eq!(text_on(Color::Blue), Color::White);
        assert_eq!(text_on(Color::Rgb(0x00, 0x00, 0x80)), Color::White);
        assert_eq!(text_on(Color::Rgb(0xf0, 0xf0, 0xf0)), Color::Black);
        // No fixed value: default to white.
        assert_eq!(text_on(Color::Reset), Color::White);
        assert_eq!(text_on(Color::Indexed(4)), Color::White);
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
            "1" | "true" | "yes" | "on" => return colors_enabled(no_color, force_config, true),
            "0" | "false" | "no" | "off" => return false,
            _ => {}
        }
    }
    colors_enabled(no_color, force_config, false)
}

/// Pure `NO_COLOR` policy shared by the TUI runtime and the theme tests.
///
/// Colors are enabled when `env_force` is set or `force` is `Some(true)`;
/// otherwise a non-empty `no_color` disables them.
pub fn colors_enabled(no_color: Option<&str>, force: Option<bool>, env_force: bool) -> bool {
    if env_force || force == Some(true) {
        return true;
    }
    !no_color.is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod color_output_tests {
    use super::{colors_enabled, resolve_color_output};

    #[test]
    fn no_color_disables_unless_forced() {
        assert!(resolve_color_output(None, None, None));
        assert!(!resolve_color_output(Some("1"), None, None));
        assert!(resolve_color_output(Some(""), None, None));
        assert!(resolve_color_output(Some("1"), Some("1"), None));
        assert!(resolve_color_output(Some("1"), None, Some(true)));
        assert!(!resolve_color_output(None, Some("0"), Some(true)));
    }

    #[test]
    fn no_color_disables_new_theme_keys() {
        // TC-604, FR-602
        assert!(!colors_enabled(Some("1"), None, false));
        assert!(colors_enabled(Some("1"), Some(true), false));
        assert!(colors_enabled(Some("1"), None, true));
        assert!(colors_enabled(Some(""), None, false));
        assert!(colors_enabled(None, None, false));
    }
}
