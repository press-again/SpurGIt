//! Theme store: built-in presets plus user themes.
//!
//! The built-in themes live in `themes/spur.json` and are compiled in (no
//! runtime path dependency); the first one ("Spur Dark") is the default.
//! User themes are JSON files under `%APPDATA%\SpurGit\themes\` (`~/Library/Application Support/SpurGit/themes`
//! on macOS, `~/.config/spurgit/themes` elsewhere); the selected theme name persists in
//! the app settings store ([`crate::settings`]) next to them.
//!
//! Editing works on a full [`ThemeConfig`] draft: every exposed [`Token`] maps
//! to one or more config colors (accent fans out to the accent family,
//! background carries the table with it). Spur stays a dark client — surfaces
//! brighter than [`MAX_SURFACE_LUMA`] are rejected so the white washes and
//! hairlines stay readable.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeSet};
pub use gpui_kit::component::ThemeConfig;
use gpui_kit::{App, Global, SharedString};

/// Name of the read-only default built-in theme.
pub const PRESET_NAME: &str = "Spur Dark";

const BUILTIN_JSON: &str = include_str!("../themes/spur.json");
const THEME_AUTHOR: &str = "Spur";
/// Dark surfaces above this relative luminance (and light surfaces below
/// [`MIN_LIGHT_SURFACE_LUMA`]) would break the mode-aware washes; the editor
/// rejects them so a theme's mode stays usable.
const MAX_SURFACE_LUMA: f32 = 0.15;
const MIN_LIGHT_SURFACE_LUMA: f32 = 0.65;

/// A color the settings editor exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Accent,
    Background,
    Sidebar,
    Popover,
    Raised,
    Text,
    MutedText,
    FaintText,
    Danger,
    Warning,
    Success,
}

pub const TOKENS: [Token; 11] = [
    Token::Accent,
    Token::Background,
    Token::Sidebar,
    Token::Popover,
    Token::Raised,
    Token::Text,
    Token::MutedText,
    Token::FaintText,
    Token::Danger,
    Token::Warning,
    Token::Success,
];

impl Token {
    /// Surfaces are luminance-guarded: the dark chrome depends on them.
    pub fn is_surface(self) -> bool {
        matches!(
            self,
            Token::Background | Token::Sidebar | Token::Popover | Token::Raised
        )
    }
}

pub struct ThemeStore {
    /// Compiled-in themes; the first one is the default.
    builtins: Vec<ThemeConfig>,
    customs: Vec<ThemeConfig>,
    selected: SharedString,
}

impl Global for ThemeStore {}

pub fn init(cx: &mut App) {
    let builtins = parse_builtins();
    let customs = load_custom();
    let known = |name: &str| {
        builtins.iter().any(|t| t.name == name) || customs.iter().any(|t| t.name == name)
    };
    let selected = read_settings()
        .filter(|name| known(name))
        .unwrap_or_else(|| PRESET_NAME.to_string());
    cx.set_global(ThemeStore {
        builtins,
        customs,
        selected: selected.into(),
    });
    apply_selected(cx);
}

fn parse_builtins() -> Vec<ThemeConfig> {
    let themes = serde_json::from_str::<ThemeSet>(BUILTIN_JSON)
        .expect("built-in themes are valid JSON")
        .themes;
    assert!(!themes.is_empty(), "built-in themes file is empty");
    themes
}

pub(crate) fn themes_dir() -> PathBuf {
    crate::settings::data_dir().join("themes")
}

/// All selectable theme names: built-ins in file order (the default first),
/// then saved themes.
pub fn names(cx: &App) -> Vec<String> {
    let store = cx.global::<ThemeStore>();
    store
        .builtins
        .iter()
        .map(|t| t.name.to_string())
        .chain(store.customs.iter().map(|t| t.name.to_string()))
        .collect()
}

pub fn selected(cx: &App) -> String {
    cx.global::<ThemeStore>().selected.to_string()
}

/// True for a compiled-in theme: built-ins cannot be deleted or overwritten.
pub fn is_builtin(cx: &App, name: &str) -> bool {
    cx.global::<ThemeStore>()
        .builtins
        .iter()
        .any(|t| t.name == name)
}

pub fn config(cx: &App, name: &str) -> Option<ThemeConfig> {
    let store = cx.global::<ThemeStore>();
    config_for(store, name)
}

/// The built-in values the settings editor resets against: the selected
/// theme's built-in when it is one, otherwise the default built-in.
pub fn baseline(cx: &App) -> ThemeConfig {
    let store = cx.global::<ThemeStore>();
    store
        .builtins
        .iter()
        .find(|t| t.name == store.selected)
        .or_else(|| store.builtins.first())
        .cloned()
        .expect("at least one built-in theme")
}

/// A name that no existing theme uses: `base`, `base 2`, `base 3`, ...
pub fn unique_name(cx: &App, base: &str) -> String {
    let names = names(cx);
    let taken = |candidate: &str| names.iter().any(|n| n == candidate);
    if !taken(base) {
        return base.to_string();
    }
    for ix in 2.. {
        let candidate = format!("{base} {ix}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    base.to_string()
}

fn config_for(store: &ThemeStore, name: &str) -> Option<ThemeConfig> {
    store
        .builtins
        .iter()
        .chain(store.customs.iter())
        .find(|t| t.name == name)
        .cloned()
}

fn selected_config(cx: &App) -> ThemeConfig {
    let store = cx.global::<ThemeStore>();
    config_for(store, &store.selected).unwrap_or_else(|| store.builtins[0].clone())
}

/// The current `#rrggbb` value of an editable token.
pub fn token_hex(config: &ThemeConfig, token: Token) -> String {
    let colors = &config.colors;
    let value = match token {
        Token::Accent => colors.ring.clone(),
        Token::Background => colors.background.clone(),
        Token::Sidebar => colors.sidebar.clone(),
        Token::Popover => colors.popover.clone(),
        Token::Raised => colors.muted.clone(),
        Token::Text => colors.foreground.clone(),
        Token::MutedText => colors.muted_foreground.clone(),
        Token::FaintText => colors.table_head_foreground.clone(),
        Token::Danger => colors.danger.clone(),
        Token::Warning => colors.warning.clone(),
        Token::Success => colors.success.clone(),
    };
    value
        .map(|v| normalize_hex(&v))
        .unwrap_or_else(|| "#000000".to_string())
}

/// Apply one token to a config draft, expanding it to the related roles.
/// Pure (unit-tested); no I/O or app state.
pub fn set_token(config: &mut ThemeConfig, token: Token, hex: &str) -> Result<(), String> {
    let rgb = parse_hex(hex).map_err(|_| "Color must be #rrggbb".to_string())?;
    if token.is_surface() {
        let luma = relative_luma(rgb);
        if config.mode.is_dark() && luma > MAX_SURFACE_LUMA {
            return Err("Surfaces must stay dark".to_string());
        }
        if !config.mode.is_dark() && luma < MIN_LIGHT_SURFACE_LUMA {
            return Err("Surfaces must stay light".to_string());
        }
    }
    let value = to_hex(rgb);
    let colors = &mut config.colors;

    match token {
        Token::Accent => {
            let hover = to_hex(lighten(rgb, 0.10));
            let active = to_hex(darken(rgb, 0.08));
            let text = to_hex(lighten(rgb, 0.16));
            let text_hover = to_hex(lighten(rgb, 0.22));
            colors.ring = Some(value.clone().into());
            colors.caret = Some(value.clone().into());
            colors.drag_border = Some(value.clone().into());
            colors.primary = Some(value.clone().into());
            colors.primary_hover = Some(hover.clone().into());
            colors.primary_active = Some(active.clone().into());
            colors.button_primary = Some(value.clone().into());
            colors.button_primary_hover = Some(hover.into());
            colors.button_primary_active = Some(active.into());
            colors.sidebar_primary = Some(value.clone().into());
            colors.link = Some(text.into());
            colors.link_hover = Some(text_hover.into());
            colors.link_active = Some(value.clone().into());
            colors.accent = Some(format!("{value}1a").into());
            colors.table_active_border = Some(format!("{value}59").into());
            colors.list_active_border = Some(format!("{value}59").into());
        }
        Token::Background => {
            colors.background = Some(value.clone().into());
            colors.table = Some(value.clone().into());
            colors.table_head = Some(format!("{value}00").into());
        }
        Token::Sidebar => colors.sidebar = Some(value.into()),
        Token::Popover => colors.popover = Some(value.into()),
        Token::Raised => colors.muted = Some(value.into()),
        Token::Text => {
            colors.foreground = Some(value.clone().into());
            colors.popover_foreground = Some(value.into());
        }
        Token::MutedText => {
            colors.muted_foreground = Some(value.clone().into());
            colors.secondary_foreground = Some(value.clone().into());
            colors.sidebar_foreground = Some(value.into());
        }
        Token::FaintText => colors.table_head_foreground = Some(value.into()),
        Token::Danger => {
            colors.danger = Some(value.clone().into());
            colors.button_danger = Some(to_hex(darken(rgb, 0.62)).into());
            colors.button_danger_hover = Some(to_hex(darken(rgb, 0.52)).into());
            colors.button_danger_active = Some(to_hex(darken(rgb, 0.40)).into());
            colors.button_danger_foreground = Some(value.into());
        }
        Token::Warning => colors.warning = Some(value.into()),
        Token::Success => colors.success = Some(value.into()),
    }
    Ok(())
}

/// Preview an unsaved draft without touching the selected theme or the disk.
pub fn preview_config(cx: &mut App, config: &ThemeConfig) {
    apply_config(cx, config);
}

/// Apply a saved theme by name and remember it for the next launch.
pub fn apply(cx: &mut App, name: &str) -> bool {
    let Some(config) = config(cx, name) else {
        return false;
    };
    apply_config(cx, &config);
    cx.global_mut::<ThemeStore>().selected = name.into();
    store_settings(name);
    true
}

fn apply_selected(cx: &mut App) {
    let config = selected_config(cx);
    apply_config(cx, &config);
}

/// Apply one config and keep the mode-aware washes in sync with its mode.
fn apply_config(cx: &mut App, config: &ThemeConfig) {
    crate::ui::widgets::set_light_wash(!config.mode.is_dark());
    Theme::global_mut(cx).apply_config(&Rc::new(config.clone()));
    cx.refresh_windows();
}

/// Write a theme into the user library without selecting it. Returns the
/// (validated) name.
pub fn install(cx: &mut App, mut config: ThemeConfig) -> Result<String, String> {
    let name = config.name.trim().to_string();
    if name.is_empty() {
        return Err("Enter a theme name".to_string());
    }
    if is_builtin(cx, &name) {
        return Err(format!("\"{name}\" is a built-in theme"));
    }
    config.name = name.as_str().into();
    config.is_default = false;
    write_theme(&config)?;
    register(cx.global_mut::<ThemeStore>(), config);
    Ok(name)
}

/// Write a theme to the user directory, select it, and apply it.
pub fn save(cx: &mut App, config: ThemeConfig) -> Result<(), String> {
    let name = install(cx, config)?;
    let store = cx.global_mut::<ThemeStore>();
    store.selected = name.as_str().into();
    store_settings(&name);
    apply_selected(cx);
    cx.refresh_windows();
    Ok(())
}

/// Import one theme JSON file into the library: a copy is written under the
/// themes directory with a unique name; the theme is not selected.
pub fn import(cx: &mut App, path: &std::path::Path) -> Result<String, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("Could not read file: {e}"))?;
    let mut set: ThemeSet =
        serde_json::from_str(&text).map_err(|e| format!("Not a theme file: {e}"))?;
    if set.themes.is_empty() {
        return Err("No theme in file".to_string());
    }
    let mut config = set.themes.remove(0);
    let base = config.name.trim().to_string();
    let base = if base.is_empty() {
        "Imported theme".to_string()
    } else {
        base
    };
    let name = unique_name(cx, &base);
    config.name = name.as_str().into();
    config.is_default = false;
    install(cx, config)?;
    Ok(name)
}

fn write_theme(config: &ThemeConfig) -> Result<(), String> {
    let name = config.name.trim();
    let path = themes_dir().join(format!("{}.json", slug(name)));
    std::fs::create_dir_all(themes_dir())
        .map_err(|e| format!("Could not create theme folder: {e}"))?;
    let set = ThemeSet {
        name: name.into(),
        author: Some(THEME_AUTHOR.into()),
        url: None,
        themes: vec![config.clone()],
    };
    let json =
        serde_json::to_string_pretty(&set).map_err(|e| format!("Could not encode theme: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("Could not write theme: {e}"))
}

fn register(store: &mut ThemeStore, config: ThemeConfig) {
    let name = config.name.to_string();
    // Match by name, and also by slug so names differing only in case or
    // punctuation cannot point two list entries at one file.
    if let Some(ix) = store
        .customs
        .iter()
        .position(|t| t.name == name.as_str() || slug(t.name.as_str()) == slug(&name))
    {
        store.customs.remove(ix);
    }
    store.customs.push(config);
}

/// Delete a saved theme. Built-in themes are never deletable.
pub fn delete(cx: &mut App, name: &str) -> Result<(), String> {
    if is_builtin(cx, name) {
        return Err("Built-in themes cannot be deleted".to_string());
    }
    let path = themes_dir().join(format!("{}.json", slug(name)));
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("Could not delete theme: {err}")),
    }
    let store = cx.global_mut::<ThemeStore>();
    store.customs.retain(|t| t.name != name);
    if store.selected == name {
        store.selected = PRESET_NAME.into();
        store_settings(PRESET_NAME);
    }
    apply_selected(cx);
    cx.refresh_windows();
    Ok(())
}

/// Filename-safe form of a theme name.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "theme".to_string()
    } else {
        trimmed
    }
}

fn load_custom() -> Vec<ThemeConfig> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(themes_dir()) else {
        return out;
    };
    let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    files.sort();
    for path in files {
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<ThemeSet>(&text) {
            Ok(set) => out.extend(set.themes),
            Err(err) => crate::logging::log!("ignored theme file {}: {err}", path.display()),
        }
    }
    out
}

fn read_settings() -> Option<String> {
    crate::settings::theme_name()
}

fn store_settings(name: &str) {
    if let Err(err) = crate::settings::set_theme_name(name) {
        crate::logging::log!("could not save settings: {err}");
    }
}

// ---- color math ----

fn parse_hex(hex: &str) -> Result<(u8, u8, u8), ()> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(());
    }
    let value = u32::from_str_radix(h, 16).map_err(|_| ())?;
    Ok((
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    ))
}

fn to_hex(rgb: (u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2)
}

fn lighten(rgb: (u8, u8, u8), factor: f32) -> (u8, u8, u8) {
    let mix = |c: u8| (c as f32 + (255.0 - c as f32) * factor).round() as u8;
    (mix(rgb.0), mix(rgb.1), mix(rgb.2))
}

fn darken(rgb: (u8, u8, u8), factor: f32) -> (u8, u8, u8) {
    let mix = |c: u8| (c as f32 * (1.0 - factor)).round() as u8;
    (mix(rgb.0), mix(rgb.1), mix(rgb.2))
}

fn relative_luma(rgb: (u8, u8, u8)) -> f32 {
    (0.2126 * rgb.0 as f32 + 0.7152 * rgb.1 as f32 + 0.0722 * rgb.2 as f32) / 255.0
}

fn normalize_hex(value: &str) -> String {
    let hex = value.trim().trim_start_matches('#');
    if hex.len() >= 6 {
        format!("#{}", hex[..6].to_ascii_lowercase())
    } else {
        value.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::ThemeMode;

    fn base() -> ThemeConfig {
        parse_builtins().remove(0)
    }

    #[test]
    fn built_in_themes_are_unique_and_respect_the_surface_guard() {
        let builtins = parse_builtins();
        assert!(builtins.len() >= 2, "a few built-ins ship");
        assert_eq!(builtins[0].name, PRESET_NAME, "the default is first");
        let mut names = Vec::new();
        for theme in &builtins {
            let name = theme.name.to_string();
            assert!(!names.contains(&name), "duplicate built-in {name}");
            names.push(name.clone());
            for token in TOKENS {
                if !token.is_surface() {
                    continue;
                }
                let hex = token_hex(theme, token);
                let rgb = parse_hex(&hex).unwrap_or_else(|_| panic!("{name}: bad {token:?}"));
                let luma = relative_luma(rgb);
                if theme.mode.is_dark() {
                    assert!(
                        luma <= MAX_SURFACE_LUMA,
                        "{name}: {token:?} {hex} is too bright for the dark washes"
                    );
                } else {
                    assert!(
                        luma >= MIN_LIGHT_SURFACE_LUMA,
                        "{name}: {token:?} {hex} is too dark for the light washers"
                    );
                }
            }
        }
    }

    #[test]
    fn light_theme_surfaces_must_stay_light() {
        let mut theme = base();
        theme.mode = ThemeMode::Light;
        assert!(set_token(&mut theme, Token::Background, "#f7f7f9").is_ok());
        assert!(set_token(&mut theme, Token::Background, "#0a0a0a").is_err());
        // Text may be dark in a light theme.
        assert!(set_token(&mut theme, Token::Text, "#1b1b1f").is_ok());
    }

    #[test]
    fn hex_roundtrip_and_rejects_bad_input() {
        assert_eq!(parse_hex("#8b7cf6").unwrap(), (0x8b, 0x7c, 0xf6));
        assert_eq!(parse_hex("8b7cf6").unwrap(), (0x8b, 0x7c, 0xf6));
        assert_eq!(to_hex((0x8b, 0x7c, 0xf6)), "#8b7cf6");
        assert!(parse_hex("#xyz").is_err());
        assert!(parse_hex("#12345").is_err());
        assert!(parse_hex("").is_err());
    }

    #[test]
    fn accent_token_fans_out_to_the_accent_family() {
        let mut theme = base();
        set_token(&mut theme, Token::Accent, "#60a5fa").unwrap();
        let c = &theme.colors;
        assert_eq!(c.ring.as_deref(), Some("#60a5fa"));
        assert_eq!(c.primary.as_deref(), Some("#60a5fa"));
        assert_eq!(c.button_primary.as_deref(), Some("#60a5fa"));
        assert_eq!(c.sidebar_primary.as_deref(), Some("#60a5fa"));
        assert_eq!(c.caret.as_deref(), Some("#60a5fa"));
        assert_eq!(c.link.as_deref(), Some("#79b3fb"));
        assert_eq!(c.link_hover.as_deref(), Some("#83b9fb"));
        assert_eq!(c.accent.as_deref(), Some("#60a5fa1a"));
        assert_eq!(c.table_active_border.as_deref(), Some("#60a5fa59"));
        assert_eq!(c.list_active_border.as_deref(), Some("#60a5fa59"));
    }

    #[test]
    fn background_token_carries_the_table_and_surface_guard() {
        let mut theme = base();
        set_token(&mut theme, Token::Background, "#0a0e17").unwrap();
        assert_eq!(theme.colors.background.as_deref(), Some("#0a0e17"));
        assert_eq!(theme.colors.table.as_deref(), Some("#0a0e17"));
        assert_eq!(theme.colors.table_head.as_deref(), Some("#0a0e1700"));
        // Text tokens are independent of the surface.
        set_token(&mut theme, Token::Text, "#e8e8ea").unwrap();
        assert_eq!(theme.colors.foreground.as_deref(), Some("#e8e8ea"));
        assert_eq!(theme.colors.popover_foreground.as_deref(), Some("#e8e8ea"));
        // Bright surfaces and bad hex are rejected without changing anything.
        assert!(set_token(&mut theme, Token::Background, "#f5f5f5").is_err());
        assert!(set_token(&mut theme, Token::Popover, "nope").is_err());
        assert_eq!(theme.colors.background.as_deref(), Some("#0a0e17"));
        // Non-surface tokens may be bright.
        assert!(set_token(&mut theme, Token::Danger, "#ff0000").is_ok());
        assert_eq!(theme.colors.danger.as_deref(), Some("#ff0000"));
    }

    #[test]
    fn saved_config_roundtrips_through_json() {
        let mut theme = base();
        set_token(&mut theme, Token::Accent, "#34d399").unwrap();
        theme.name = "Roundtrip".into();
        let set = ThemeSet {
            name: "Roundtrip".into(),
            author: None,
            url: None,
            themes: vec![theme.clone()],
        };
        let json = serde_json::to_string_pretty(&set).unwrap();
        let parsed: ThemeSet = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.themes.len(), 1);
        assert_eq!(parsed.themes[0].name, theme.name);
        assert_eq!(parsed.themes[0].colors.accent, theme.colors.accent);
        assert_eq!(parsed.themes[0].colors.ring, theme.colors.ring);
    }

    #[test]
    fn slug_is_filename_safe() {
        assert_eq!(slug("My Theme"), "my-theme");
        assert_eq!(slug("  Dusk /// 2  "), "dusk-2");
        assert_eq!(slug("///"), "theme");
    }

    #[test]
    fn every_token_reads_back_what_it_sets() {
        let mut theme = base();
        for token in TOKENS {
            let hex = match token {
                Token::Danger | Token::Warning | Token::Success => "#f87171",
                Token::Text | Token::MutedText | Token::FaintText => "#d4d4d8",
                _ => "#101319",
            };
            set_token(&mut theme, token, hex).unwrap();
            assert_eq!(
                token_hex(&theme, token),
                hex,
                "{token:?} did not read back"
            );
        }
    }
}
