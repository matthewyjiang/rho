//! Named color schemes for the interactive TUI.
//!
//! Built-in schemes ship in-process. Custom schemes load from
//! `$RHO_HOME/themes/*.json` (or `~/.rho/themes/`) in Windows Terminal
//! color-scheme JSON form so users can drop files from common theme catalogs.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

/// Config / picker id for the host-terminal-matched theme.
pub(super) const TERMINAL_THEME_ID: &str = "terminal";

/// Shared RGB triple for schemes and palette math.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Rgb {
    pub(super) const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    pub(super) fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim().strip_prefix('#').unwrap_or(hex.trim());
        match hex.len() {
            6 => {
                let value = u32::from_str_radix(hex, 16).ok()?;
                Some(Self::new(
                    ((value >> 16) & 0xff) as u8,
                    ((value >> 8) & 0xff) as u8,
                    (value & 0xff) as u8,
                ))
            }
            3 => {
                let value = u32::from_str_radix(hex, 16).ok()?;
                let red = ((value >> 8) & 0xf) as u8;
                let green = ((value >> 4) & 0xf) as u8;
                let blue = (value & 0xf) as u8;
                Some(Self::new(red * 17, green * 17, blue * 17))
            }
            _ => None,
        }
    }

    pub(super) fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.red, self.green, self.blue)
    }

    pub(super) fn luminance(self) -> f32 {
        (0.2126 * f32::from(self.red)
            + 0.7152 * f32::from(self.green)
            + 0.0722 * f32::from(self.blue))
            / 255.0
    }

    pub(super) fn blend_toward(self, overlay: Self, alpha: f32) -> Self {
        Self::new(
            blend_channel(self.red, overlay.red, alpha),
            blend_channel(self.green, overlay.green, alpha),
            blend_channel(self.blue, overlay.blue, alpha),
        )
    }
}

fn blend_channel(base: u8, overlay: u8, alpha: f32) -> u8 {
    (base as f32 + (overlay as f32 - base as f32) * alpha).round() as u8
}

/// Fixed 16-color scheme plus surface colors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ColorScheme {
    pub id: String,
    pub name: String,
    pub background: Rgb,
    pub foreground: Rgb,
    /// ANSI colors 0-15 (normal then bright).
    pub ansi: [Rgb; 16],
    pub source: ThemeSourceKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ThemeSourceKind {
    Builtin,
    Custom,
}

/// One row in the theme catalog. Fixed entries already carry their scheme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ThemeEntry {
    Terminal,
    Fixed(ColorScheme),
}

impl ThemeEntry {
    pub(super) fn id(&self) -> &str {
        match self {
            Self::Terminal => TERMINAL_THEME_ID,
            Self::Fixed(scheme) => scheme.id.as_str(),
        }
    }

    pub(super) fn name(&self) -> &str {
        match self {
            Self::Terminal => "Terminal",
            Self::Fixed(scheme) => scheme.name.as_str(),
        }
    }

    pub(super) fn source_label(&self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Fixed(scheme) => match scheme.source {
                ThemeSourceKind::Builtin => "built-in",
                ThemeSourceKind::Custom => "custom",
            },
        }
    }

    pub(super) fn is_custom(&self) -> bool {
        matches!(
            self,
            Self::Fixed(ColorScheme {
                source: ThemeSourceKind::Custom,
                ..
            })
        )
    }

    pub(super) fn detail(&self) -> String {
        match self {
            Self::Terminal => terminal_theme_detail().to_string(),
            Self::Fixed(scheme) => scheme_detail(scheme),
        }
    }
}

/// Normalize a configured theme id. Empty becomes terminal.
pub(super) fn normalize_theme_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        TERMINAL_THEME_ID.into()
    } else {
        trimmed.to_string()
    }
}

pub(super) fn is_terminal_theme_id(id: &str) -> bool {
    normalize_theme_id(id) == TERMINAL_THEME_ID
}

/// Display name for a theme id without scanning the full catalog when possible.
pub(super) fn theme_display_name(id: &str) -> String {
    let id = normalize_theme_id(id);
    if is_terminal_theme_id(&id) {
        return "Terminal".into();
    }
    resolve_fixed_scheme(&id)
        .map(|scheme| scheme.name)
        .unwrap_or(id)
}

/// Directory for user-supplied theme JSON files.
pub(super) fn themes_dir() -> anyhow::Result<PathBuf> {
    Ok(crate::paths::rho_dir()?.join("themes"))
}

/// Resolve a theme id to a fixed scheme, if any.
///
/// `terminal` returns `None` (caller keeps host sampling). Unknown ids return
/// `None` so the TUI can fall back to the terminal theme.
pub(super) fn resolve_fixed_scheme(id: &str) -> Option<ColorScheme> {
    let id = normalize_theme_id(id);
    if is_terminal_theme_id(&id) {
        return None;
    }
    if let Some(scheme) = builtin_scheme(&id) {
        return Some(scheme);
    }
    load_custom_scheme(&id).ok().flatten()
}

/// Catalog for the theme picker: terminal, built-ins, and custom files.
///
/// Order is alphabetical by display name so built-ins are not privileged.
/// Fixed entries are fully loaded so callers do not re-resolve.
pub(super) fn list_themes() -> Vec<ThemeEntry> {
    let mut items = vec![ThemeEntry::Terminal];

    for scheme in builtin_schemes() {
        items.push(ThemeEntry::Fixed(scheme));
    }

    if let Ok(dir) = themes_dir() {
        items.extend(scan_custom_theme_dir(&dir));
    }

    items.sort_by(|left, right| {
        left.name()
            .to_ascii_lowercase()
            .cmp(&right.name().to_ascii_lowercase())
            .then_with(|| left.id().cmp(right.id()))
    });
    items
}

pub(super) fn scheme_detail(scheme: &ColorScheme) -> String {
    let variant = if scheme_is_dark(scheme) {
        "dark"
    } else {
        "light"
    };
    format!(
        "{variant} · bg {} · fg {} · accent {}\nred {} green {} yellow {} blue {}\n{}",
        scheme.background.to_hex(),
        scheme.foreground.to_hex(),
        scheme.ansi[6].to_hex(),
        scheme.ansi[1].to_hex(),
        scheme.ansi[2].to_hex(),
        scheme.ansi[3].to_hex(),
        scheme.ansi[4].to_hex(),
        match scheme.source {
            ThemeSourceKind::Builtin => "built-in scheme".to_string(),
            ThemeSourceKind::Custom => "custom scheme from ~/.rho/themes".to_string(),
        }
    )
}

pub(super) fn terminal_theme_detail() -> &'static str {
    "Match the host terminal palette.\nRho samples background and ANSI colors at startup.\nThis is the default."
}

fn scheme_is_dark(scheme: &ColorScheme) -> bool {
    scheme.background.luminance() <= 0.55
}

fn builtin_schemes() -> Vec<ColorScheme> {
    vec![
        one_half_dark(),
        one_half_light(),
        monochrome_dark(),
        monochrome_light(),
    ]
}

fn builtin_scheme(id: &str) -> Option<ColorScheme> {
    builtin_schemes().into_iter().find(|scheme| scheme.id == id)
}

/// [One Half Dark](https://github.com/sonph/onehalf) by Son A. Pham.
fn one_half_dark() -> ColorScheme {
    scheme_from_parts(
        "one-half-dark",
        "One Half Dark",
        ThemeSourceKind::Builtin,
        "#282c34",
        "#dcdfe4",
        [
            "#282c34", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#dcdfe4",
            "#5c6370", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff",
        ],
    )
}

/// [One Half Light](https://github.com/sonph/onehalf) by Son A. Pham.
fn one_half_light() -> ColorScheme {
    scheme_from_parts(
        "one-half-light",
        "One Half Light",
        ThemeSourceKind::Builtin,
        "#fafafa",
        "#383a42",
        [
            "#383a42", "#e45649", "#50a14f", "#c18401", "#0184bc", "#a626a4", "#0997b3", "#fafafa",
            "#4f525e", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd", "#56b6c2", "#ffffff",
        ],
    )
}

fn monochrome_dark() -> ColorScheme {
    scheme_from_parts(
        "monochrome-dark",
        "Monochrome Dark",
        ThemeSourceKind::Builtin,
        "#121212",
        "#e6e6e6",
        [
            "#121212", "#b0b0b0", "#c0c0c0", "#d0d0d0", "#a8a8a8", "#b8b8b8", "#c8c8c8", "#e6e6e6",
            "#6a6a6a", "#c4c4c4", "#d4d4d4", "#e0e0e0", "#bcbcbc", "#cccccc", "#dadada", "#ffffff",
        ],
    )
}

fn monochrome_light() -> ColorScheme {
    scheme_from_parts(
        "monochrome-light",
        "Monochrome Light",
        ThemeSourceKind::Builtin,
        "#f5f5f5",
        "#1a1a1a",
        [
            "#1a1a1a", "#4a4a4a", "#3a3a3a", "#5a5a5a", "#2a2a2a", "#404040", "#505050", "#f5f5f5",
            "#8a8a8a", "#5a5a5a", "#4a4a4a", "#6a6a6a", "#3a3a3a", "#505050", "#606060", "#ffffff",
        ],
    )
}

fn scheme_from_parts(
    id: &str,
    name: &str,
    source: ThemeSourceKind,
    background: &str,
    foreground: &str,
    ansi: [&str; 16],
) -> ColorScheme {
    ColorScheme {
        id: id.into(),
        name: name.into(),
        background: Rgb::from_hex(background).expect("builtin background"),
        foreground: Rgb::from_hex(foreground).expect("builtin foreground"),
        ansi: ansi.map(|hex| Rgb::from_hex(hex).expect("builtin ansi")),
        source,
    }
}

fn scan_custom_theme_dir(dir: &Path) -> Vec<ThemeEntry> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if stem.is_empty() || is_terminal_theme_id(stem) || builtin_scheme(stem).is_some() {
            // Built-in ids stay reserved so a colliding file does not hide them.
            continue;
        }
        let Ok(Some(scheme)) = load_custom_scheme_from_path(stem, &path) else {
            continue;
        };
        items.push(ThemeEntry::Fixed(scheme));
    }
    items
}

fn load_custom_scheme(id: &str) -> anyhow::Result<Option<ColorScheme>> {
    let path = themes_dir()?.join(format!("{id}.json"));
    if !path.is_file() {
        return Ok(None);
    }
    load_custom_scheme_from_path(id, &path)
}

fn load_custom_scheme_from_path(id: &str, path: &Path) -> anyhow::Result<Option<ColorScheme>> {
    let raw = fs::read_to_string(path)
        .map_err(|error| anyhow::anyhow!("read theme {}: {error}", path.display()))?;
    let file: WindowsTerminalScheme = serde_json::from_str(&raw)
        .map_err(|error| anyhow::anyhow!("parse theme {}: {error}", path.display()))?;
    Ok(Some(file.into_scheme(id, ThemeSourceKind::Custom)?))
}

/// Windows Terminal `schemes` entry shape (also used by iterm2-color-schemes ports).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsTerminalScheme {
    name: Option<String>,
    background: String,
    foreground: String,
    #[serde(default)]
    cursor_color: Option<String>,
    #[serde(default)]
    selection_background: Option<String>,
    black: String,
    red: String,
    green: String,
    yellow: String,
    blue: String,
    purple: String,
    cyan: String,
    white: String,
    bright_black: String,
    bright_red: String,
    bright_green: String,
    bright_yellow: String,
    bright_blue: String,
    bright_purple: String,
    bright_cyan: String,
    bright_white: String,
}

impl WindowsTerminalScheme {
    fn into_scheme(self, id: &str, source: ThemeSourceKind) -> anyhow::Result<ColorScheme> {
        let parse = |label: &str, value: &str| {
            Rgb::from_hex(value)
                .ok_or_else(|| anyhow::anyhow!("invalid {label} color '{value}' in theme '{id}'"))
        };
        // cursorColor / selectionBackground are accepted for WT compatibility but
        // not stored; the TUI derives chrome from background, foreground, and ANSI.
        let _cursor = self
            .cursor_color
            .as_deref()
            .map(|value| parse("cursorColor", value))
            .transpose()?;
        let _selection = self
            .selection_background
            .as_deref()
            .map(|value| parse("selectionBackground", value))
            .transpose()?;
        Ok(ColorScheme {
            id: id.into(),
            name: self
                .name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| id.to_string()),
            background: parse("background", &self.background)?,
            foreground: parse("foreground", &self.foreground)?,
            ansi: [
                parse("black", &self.black)?,
                parse("red", &self.red)?,
                parse("green", &self.green)?,
                parse("yellow", &self.yellow)?,
                parse("blue", &self.blue)?,
                parse("purple", &self.purple)?,
                parse("cyan", &self.cyan)?,
                parse("white", &self.white)?,
                parse("brightBlack", &self.bright_black)?,
                parse("brightRed", &self.bright_red)?,
                parse("brightGreen", &self.bright_green)?,
                parse("brightYellow", &self.bright_yellow)?,
                parse("brightBlue", &self.bright_blue)?,
                parse("brightPurple", &self.bright_purple)?,
                parse("brightCyan", &self.bright_cyan)?,
                parse("brightWhite", &self.bright_white)?,
            ],
            source,
        })
    }
}

#[cfg(test)]
#[path = "theme_scheme_tests.rs"]
mod tests;
