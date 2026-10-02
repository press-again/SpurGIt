//! Keyboard shortcuts: the compiled default bindings, `keymap.json`
//! overrides from `%APPDATA%\SpurGit`, the `?` cheat-sheet card, and the
//! Ctrl+1/2/3 repository-section actions. The palette's shortcut hints and
//! the cheat sheet both read the effective bindings, so a remap shows up
//! everywhere without a second source of truth.

use super::*;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme, Sizable as _};
use gpui_kit::{App, Global, IntoElement, Keystroke};

use serde_json::Value;

use crate::i18n::{self, t};

gpui_kit::actions!(
    spur,
    [ShowShortcuts, ShowHistory, ShowChanges, ShowStashes, UndoLast]
);

/// Cheat-sheet grouping, rendered in this order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ShortcutGroup {
    General,
    Repository,
    Commit,
    Palette,
}

impl ShortcutGroup {
    pub(super) const ORDER: [ShortcutGroup; 4] = [
        ShortcutGroup::General,
        ShortcutGroup::Repository,
        ShortcutGroup::Commit,
        ShortcutGroup::Palette,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            ShortcutGroup::General => t().shortcuts_group_general,
            ShortcutGroup::Repository => t().shortcuts_group_repository,
            ShortcutGroup::Commit => t().shortcuts_group_commit,
            ShortcutGroup::Palette => t().shortcuts_group_palette,
        }
    }
}

/// One remappable action: its `keymap.json` name, compiled defaults, the
/// display for those defaults, its cheat-sheet label and group, and the GPUI
/// context the binding is scoped to (`None` means global).
pub(super) struct ShortcutDef {
    pub action: &'static str,
    pub keys: &'static [&'static str],
    pub display: &'static str,
    pub label: fn(&i18n::Strings) -> &'static str,
    pub group: ShortcutGroup,
    pub context: Option<&'static str>,
}

/// Every binding SpurGit registers, in cheat-sheet order. Collisions between
/// these defaults are a programming error (covered by a test); collisions
/// with `keymap.json` overrides resolve first-in-table-order.
pub(super) const SHORTCUTS: &[ShortcutDef] = &[
    ShortcutDef {
        action: "toggle-palette",
        keys: &["ctrl-k"],
        display: "Ctrl K",
        label: |t| t.shortcut_palette,
        group: ShortcutGroup::General,
        context: None,
    },
    ShortcutDef {
        action: "open-settings",
        keys: &["ctrl-,"],
        display: "Ctrl ,",
        label: |t| t.shortcut_settings,
        group: ShortcutGroup::General,
        context: None,
    },
    ShortcutDef {
        action: "show-shortcuts",
        keys: &["shift-/", "ctrl-/"],
        display: "?",
        label: |t| t.shortcut_shortcuts,
        group: ShortcutGroup::General,
        context: None,
    },
    ShortcutDef {
        action: "close-dialog",
        keys: &["escape"],
        display: "Esc",
        label: |t| t.shortcut_close,
        group: ShortcutGroup::General,
        context: None,
    },
    ShortcutDef {
        action: "undo-last",
        keys: &["ctrl-alt-z"],
        display: "Ctrl Alt Z",
        label: |t| t.shortcut_undo_last,
        group: ShortcutGroup::General,
        context: None,
    },
    ShortcutDef {
        action: "operation-log",
        keys: &["ctrl-shift-l"],
        display: "Ctrl Shift L",
        label: |t| t.shortcut_oplog,
        group: ShortcutGroup::General,
        context: None,
    },
    ShortcutDef {
        action: "next-tab",
        keys: &["ctrl-tab"],
        display: "Ctrl Tab",
        label: |t| t.shortcut_next_tab,
        group: ShortcutGroup::Repository,
        context: None,
    },
    ShortcutDef {
        action: "prev-tab",
        keys: &["ctrl-shift-tab"],
        display: "Ctrl Shift Tab",
        label: |t| t.shortcut_prev_tab,
        group: ShortcutGroup::Repository,
        context: None,
    },
    ShortcutDef {
        action: "close-tab",
        keys: &["ctrl-w"],
        display: "Ctrl W",
        label: |t| t.shortcut_close_tab,
        group: ShortcutGroup::Repository,
        context: None,
    },
    ShortcutDef {
        action: "show-history",
        keys: &["ctrl-1"],
        display: "Ctrl 1",
        label: |t| t.shortcut_history,
        group: ShortcutGroup::Repository,
        context: None,
    },
    ShortcutDef {
        action: "show-changes",
        keys: &["ctrl-2"],
        display: "Ctrl 2",
        label: |t| t.shortcut_changes,
        group: ShortcutGroup::Repository,
        context: None,
    },
    ShortcutDef {
        action: "show-stashes",
        keys: &["ctrl-3"],
        display: "Ctrl 3",
        label: |t| t.shortcut_stashes,
        group: ShortcutGroup::Repository,
        context: None,
    },
    ShortcutDef {
        action: "submit-commit",
        keys: &["ctrl-enter"],
        display: "Ctrl Enter",
        label: |t| t.shortcut_commit,
        group: ShortcutGroup::Commit,
        context: None,
    },
    ShortcutDef {
        action: "palette-next",
        keys: &["tab"],
        display: "Tab",
        label: |t| t.shortcut_palette_next,
        group: ShortcutGroup::Palette,
        context: Some("palette"),
    },
    ShortcutDef {
        action: "palette-prev",
        keys: &["shift-tab"],
        display: "Shift Tab",
        label: |t| t.shortcut_palette_prev,
        group: ShortcutGroup::Palette,
        context: Some("palette"),
    },
];

pub(super) fn def(action: &str) -> Option<&'static ShortcutDef> {
    SHORTCUTS.iter().find(|def| def.action == action)
}

/// One action's effective keystrokes after `keymap.json` overrides.
#[derive(Clone, Debug)]
pub struct EffectiveBinding {
    pub action: String,
    pub keys: Vec<String>,
}

/// Display for an action's keys: the curated label when untouched, the
/// prettified override otherwise, "Not bound" when conflicts ate everything.
pub fn display_for(action: &str, keys: &[String]) -> String {
    if keys.is_empty() {
        return t().keymap_unbound.to_string();
    }
    if let Some(def) = def(action) {
        let defaults: Vec<String> = def.keys.iter().map(|key| key.to_string()).collect();
        if *keys == defaults {
            return def.display.to_string();
        }
    }
    keys.iter().map(|key| prettify(key)).collect::<Vec<_>>().join(", ")
}

/// "ctrl-shift-tab" → "Ctrl Shift Tab", "shift-/" → "?", "escape" → "Esc".
/// Key names are not localized (like the cheat-sheet chips themselves).
pub fn prettify(keystroke: &str) -> String {
    keystroke
        .split_whitespace()
        .map(|one| {
            if one == "escape" {
                return "Esc".to_string();
            }
            let parts: Vec<String> = one
                .split('-')
                .map(|part| match part {
                    "ctrl" => "Ctrl".to_string(),
                    "shift" => "Shift".to_string(),
                    "alt" => "Alt".to_string(),
                    part => {
                        let mut chars = part.chars();
                        match chars.next() {
                            Some(first) => {
                                first.to_uppercase().collect::<String>() + chars.as_str()
                            }
                            None => String::new(),
                        }
                    }
                })
                .collect();
            if parts.len() == 2 && parts[0] == "Shift" && parts[1] == "/" {
                return "?".to_string();
            }
            parts.join(" ")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct ShortcutStore {
    bindings: Vec<EffectiveBinding>,
    diagnostics: Vec<String>,
    customized: usize,
    /// Bindings registered before ours (gpui-component inputs, menus, ...).
    /// A live rebind clears the whole keymap, so these are re-added first.
    base: Vec<gpui_kit::KeyBinding>,
}

impl Global for ShortcutStore {}

/// `keymap.json` next to `settings.json`: `{ "bindings": [
/// { "action": "toggle-palette", "keys": "ctrl-p" } ] }`. `keys` also
/// accepts an array. Unknown actions, unparsable keystrokes, and collisions
/// become diagnostics; the safe default always survives them.
pub fn keymap_path() -> PathBuf {
    keymap_path_for(&crate::settings::data_dir())
}

fn keymap_path_for(dir: &Path) -> PathBuf {
    dir.join("keymap.json")
}

fn parse_keymap_file(text: &str) -> (Vec<(String, Vec<String>)>, Vec<String>) {
    let mut diagnostics = Vec::new();
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => {
            return (
                Vec::new(),
                vec![format!("keymap.json is not valid JSON: {err}")],
            );
        }
    };
    let Some(bindings) = value.get("bindings") else {
        return (Vec::new(), Vec::new());
    };
    let Value::Array(bindings) = bindings else {
        return (
            Vec::new(),
            vec![format!("\"bindings\" must be an array, found {bindings}")],
        );
    };
    let mut entries = Vec::new();
    for (ix, entry) in bindings.iter().enumerate() {
        let Some(obj) = entry.as_object() else {
            diagnostics.push(format!("\"bindings[{ix}]\" must be an object"));
            continue;
        };
        let Some(action) = obj.get("action").and_then(|action| action.as_str()) else {
            diagnostics.push(format!("\"bindings[{ix}]\" needs an \"action\" string"));
            continue;
        };
        let keys = match obj.get("keys") {
            Some(Value::String(keys)) => vec![keys.clone()],
            Some(Value::Array(keys)) => {
                let mut list = Vec::new();
                let mut bad = false;
                for keys in keys {
                    match keys.as_str() {
                        Some(keys) => list.push(keys.to_string()),
                        None => {
                            bad = true;
                            break;
                        }
                    }
                }
                if bad {
                    diagnostics.push(format!(
                        "\"bindings[{ix}]\" \"keys\" must hold strings"
                    ));
                    continue;
                }
                list
            }
            _ => {
                diagnostics.push(format!("\"bindings[{ix}]\" needs \"keys\""));
                continue;
            }
        };
        if keys.is_empty() {
            diagnostics.push(format!("\"bindings[{ix}]\" \"keys\" is empty"));
            continue;
        }
        let mut valid = true;
        for keys in &keys {
            if keys.split_whitespace().count() == 0 {
                valid = false;
                break;
            }
            for one in keys.split_whitespace() {
                if Keystroke::parse(one).is_err() {
                    valid = false;
                    break;
                }
            }
        }
        if !valid {
            diagnostics.push(format!("\"{}\" is not a valid keystroke", keys.join(" ")));
            continue;
        }
        entries.push((action.to_string(), keys));
    }
    (entries, diagnostics)
}

/// Merge overrides over the table. Unknown actions are reported and
/// skipped; on a keystroke collision the earlier table entry wins and the
/// loser is reported (a binding that loses every keystroke is "Not bound").
fn merge(
    entries: Vec<(String, Vec<String>)>,
) -> (Vec<EffectiveBinding>, Vec<String>, usize) {
    let mut diagnostics = Vec::new();
    let mut overrides: HashMap<String, Vec<String>> = HashMap::new();
    for (action, keys) in entries {
        if def(&action).is_none() {
            diagnostics.push(format!("unknown action \"{action}\""));
            continue;
        }
        overrides.insert(action, keys);
    }
    let mut customized = 0;
    let mut taken: HashMap<String, String> = HashMap::new();
    let mut bindings = Vec::new();
    for entry in SHORTCUTS {
        let defaults: Vec<String> = entry.keys.iter().map(|key| key.to_string()).collect();
        let wanted = overrides.get(entry.action).unwrap_or(&defaults).clone();
        if wanted != defaults {
            customized += 1;
        }
        let mut kept = Vec::new();
        for keys in wanted {
            if let Some(owner) = taken.get(&keys) {
                diagnostics.push(format!(
                    "\"{keys}\" is bound to both \"{owner}\" and \"{}\"; \"{owner}\" wins",
                    entry.action
                ));
                continue;
            }
            taken.insert(keys.clone(), entry.action.to_string());
            kept.push(keys);
        }
        if kept.is_empty() {
            diagnostics.push(format!("\"{}\" has no working binding", entry.action));
        }
        bindings.push(EffectiveBinding {
            action: entry.action.to_string(),
            keys: kept,
        });
    }
    (bindings, diagnostics, customized)
}

fn load() -> (Vec<EffectiveBinding>, Vec<String>, usize) {
    let text = std::fs::read_to_string(keymap_path()).ok();
    load_from(text.as_deref())
}

/// Mergeable core for tests: `None` is a missing file (clean defaults).
fn load_from(text: Option<&str>) -> (Vec<EffectiveBinding>, Vec<String>, usize) {
    let Some(text) = text else {
        return (
            SHORTCUTS
                .iter()
                .map(|entry| EffectiveBinding {
                    action: entry.action.to_string(),
                    keys: entry.keys.iter().map(|key| key.to_string()).collect(),
                })
                .collect(),
            Vec::new(),
            0,
        );
    };
    let (entries, mut diagnostics) = parse_keymap_file(text);
    let (bindings, mut merge_diagnostics, customized) = merge(entries);
    diagnostics.append(&mut merge_diagnostics);
    (bindings, diagnostics, customized)
}

/// Load `keymap.json` over the compiled defaults before the first window.
/// Diagnostics go to the startup log and the Settings Keyboard card.
pub fn init(cx: &mut App) {
    let (bindings, diagnostics, customized) = load();
    for diagnostic in &diagnostics {
        crate::logging::log!("keymap: {diagnostic}");
    }
    // Runs after `gpui_kit::init`: everything bound so far is the base.
    let base = cx.key_bindings().borrow().bindings().cloned().collect();
    cx.set_global(ShortcutStore {
        bindings,
        diagnostics,
        customized,
        base,
    });
}

pub fn bindings(cx: &App) -> Vec<EffectiveBinding> {
    cx.global::<ShortcutStore>().bindings.clone()
}

pub fn diagnostics(cx: &App) -> Vec<String> {
    cx.global::<ShortcutStore>().diagnostics.clone()
}

pub fn customized(cx: &App) -> usize {
    cx.global::<ShortcutStore>().customized
}

/// True when the effective keys differ from the compiled default (the
/// Settings row earns its Reset button).
pub fn is_customized(action: &str, keys: &[String]) -> bool {
    match def(action) {
        Some(entry) => {
            entry.keys.iter().map(|key| key.to_string()).collect::<Vec<_>>() != keys
        }
        None => false,
    }
}

/// GPUI bindings for the effective keymap, built once at startup from the
/// merged table. Every keystroke was validated on load, so the panicking
/// `KeyBinding::new` cannot fire here.
pub fn key_bindings(cx: &App) -> Vec<gpui_kit::KeyBinding> {
    let mut out = Vec::new();
    for binding in bindings(cx) {
        let Some(entry) = def(&binding.action) else {
            continue;
        };
        for keys in &binding.keys {
            let context = entry.context;
            out.push(match binding.action.as_str() {
                "toggle-palette" => gpui_kit::KeyBinding::new(keys, palette::TogglePalette, context),
                "open-settings" => {
                    gpui_kit::KeyBinding::new(keys, settings::OpenSettings, context)
                }
                "show-shortcuts" => gpui_kit::KeyBinding::new(keys, ShowShortcuts, context),
                "close-dialog" => gpui_kit::KeyBinding::new(keys, palette::ClosePalette, context),
                "undo-last" => gpui_kit::KeyBinding::new(keys, UndoLast, context),
                "operation-log" => gpui_kit::KeyBinding::new(keys, oplog::ToggleOpLog, context),
                "next-tab" => gpui_kit::KeyBinding::new(keys, tabs::NextTab, context),
                "prev-tab" => gpui_kit::KeyBinding::new(keys, tabs::PrevTab, context),
                "close-tab" => gpui_kit::KeyBinding::new(keys, tabs::CloseTab, context),
                "show-history" => gpui_kit::KeyBinding::new(keys, ShowHistory, context),
                "show-changes" => gpui_kit::KeyBinding::new(keys, ShowChanges, context),
                "show-stashes" => gpui_kit::KeyBinding::new(keys, ShowStashes, context),
                "submit-commit" => gpui_kit::KeyBinding::new(keys, palette::SubmitCommit, context),
                "palette-next" => gpui_kit::KeyBinding::new(keys, palette::PaletteNext, context),
                "palette-prev" => gpui_kit::KeyBinding::new(keys, palette::PalettePrev, context),
                _ => continue,
            });
        }
    }
    out
}

/// Display of one action's effective keys for palette hints.
pub fn keys_display(cx: &App, action: &str) -> Option<String> {
    cx.global::<ShortcutStore>()
        .bindings
        .iter()
        .find(|binding| binding.action == action)
        .map(|binding| display_for(&binding.action, &binding.keys))
}

/// Canonical keystroke string for a captured key press (`None` while only
/// modifiers are held, or for keys that can never be bindings). Round-trips
/// through `Keystroke::parse` by construction.
pub fn canonical_keystroke(
    control: bool,
    alt: bool,
    shift: bool,
    platform: bool,
    function: bool,
    key: &str,
) -> Option<String> {
    let bare = key.to_lowercase();
    if bare.is_empty() {
        return None;
    }
    if matches!(
        bare.as_str(),
        "shift"
            | "control"
            | "ctrl"
            | "alt"
            | "altgraph"
            | "meta"
            | "super"
            | "win"
            | "cmd"
            | "fn"
            | "function"
            | "capslock"
            | "numlock"
            | "scrolllock"
            | "escape"
    ) {
        return None;
    }
    let mut parts = Vec::new();
    if control {
        parts.push("ctrl");
    }
    if alt {
        parts.push("alt");
    }
    if shift {
        parts.push("shift");
    }
    if platform {
        parts.push("win");
    }
    if function {
        parts.push("fn");
    }
    parts.push(&bare);
    Some(parts.join("-"))
}

/// Persist one binding into `keymap.json` (read-modify-write, preserving
/// unrelated content). A malformed file is refused, never overwritten.
pub fn save_keymap_binding(dir: &Path, action: &str, keys: &str) -> Result<(), String> {
    let path = keymap_path_for(dir);
    let mut value = match std::fs::read_to_string(&path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            serde_json::json!({"bindings": []})
        }
        Err(err) => return Err(format!("could not read {}: {err}", path.display())),
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|err| format!("keymap.json is malformed ({err}); fix or delete it"))?,
    };
    let Some(obj) = value.as_object_mut() else {
        return Err("keymap.json is not an object; fix or delete it".to_string());
    };
    let bindings = obj
        .entry("bindings")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(list) = bindings.as_array_mut() else {
        return Err("\"bindings\" is not an array; fix or delete it".to_string());
    };
    list.retain(|entry| entry.get("action").and_then(|action| action.as_str()) != Some(action));
    list.push(serde_json::json!({"action": action, "keys": keys}));
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
    let tmp = dir.join("keymap.json.tmp");
    let json = serde_json::to_string_pretty(&value)
        .map_err(|err| format!("could not encode keymap: {err}"))?;
    std::fs::write(&tmp, json).map_err(|err| format!("could not write {}: {err}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|err| format!("could not replace {}: {err}", path.display()))
}

/// Drop one override so the compiled default applies again. A missing file
/// is already-default; a malformed one is refused like on save.
pub fn reset_keymap_binding(dir: &Path, action: &str) -> Result<(), String> {
    let path = keymap_path_for(dir);
    let text = match std::fs::read_to_string(&path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("could not read {}: {err}", path.display())),
        Ok(text) => text,
    };
    let mut value: Value = serde_json::from_str(&text)
        .map_err(|err| format!("keymap.json is malformed ({err}); fix or delete it"))?;
    let mut changed = false;
    if let Some(list) = value
        .get_mut("bindings")
        .and_then(|bindings| bindings.as_array_mut())
    {
        let before = list.len();
        list.retain(|entry| entry.get("action").and_then(|action| action.as_str()) != Some(action));
        changed = list.len() != before;
    }
    if !changed {
        return Ok(());
    }
    let tmp = dir.join("keymap.json.tmp");
    let json = serde_json::to_string_pretty(&value)
        .map_err(|err| format!("could not encode keymap: {err}"))?;
    std::fs::write(&tmp, json).map_err(|err| format!("could not write {}: {err}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|err| format!("could not replace {}: {err}", path.display()))
}

impl SpurShell {
    /// Reload the effective keymap and rebind live: clear every binding,
    /// then register the merged table. Called at startup (from main) and
    /// after every capture/reset in Settings.
    pub(super) fn apply_effective_bindings(&mut self, cx: &mut Context<Self>) {
        let (bindings, diagnostics, customized) = load();
        for diagnostic in &diagnostics {
            crate::logging::log!("keymap: {diagnostic}");
        }
        let base = cx.global::<ShortcutStore>().base.clone();
        cx.set_global(ShortcutStore {
            bindings,
            diagnostics,
            customized,
            base: base.clone(),
        });
        let bindings = key_bindings(cx);
        // Clearing drops the component bindings too (text-input editing,
        // menu navigation): put them back before ours, in the same order.
        cx.clear_key_bindings();
        cx.bind_keys(base);
        cx.bind_keys(bindings);
        cx.notify();
    }

    /// `?` / Ctrl+/ outside text fields (guarded like tab navigation so
    /// typing is never hijacked).
    pub(super) fn toggle_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if tabs::text_input_focused(self, window, cx) {
            return;
        }
        self.shortcuts_open = !self.shortcuts_open;
        if self.shortcuts_open {
            self.open_modal(cx);
        } else {
            self.close_modal(cx);
        }
        cx.notify();
    }

    pub(super) fn cancel_shortcuts(&mut self, cx: &mut Context<Self>) {
        if self.shortcuts_open {
            self.shortcuts_open = false;
            self.close_modal(cx);
        }
    }

    /// Arm key capture for one action from its Settings row. Focus moves to
    /// the Settings body so presses reach the page key handler (a plain
    /// click leaves focus wherever it was, and capture would never fire).
    pub(super) fn start_capture(
        &mut self,
        action: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shortcut_capture = Some(action);
        self.shortcut_status = None;
        window.focus(&self.shortcut_focus, cx);
        cx.notify();
    }

    /// Drop one override and rebind live.
    pub(super) fn reset_binding(&mut self, action: String, cx: &mut Context<Self>) {
        match reset_keymap_binding(&crate::settings::data_dir(), &action) {
            Ok(()) => {
                self.shortcut_status = Some((t().keymap_saved(), false));
                self.apply_effective_bindings(cx);
            }
            Err(err) => {
                self.shortcut_status = Some((err, true));
                cx.notify();
            }
        }
    }

    /// One key press while a Settings row is capturing. Escape cancels;
    /// bare modifiers keep waiting; plain keys are refused (they would
    /// hijack typing); collisions name their owner instead of stealing.
    pub(super) fn capture_shortcut_key(
        &mut self,
        modifiers: &gpui_kit::Modifiers,
        key: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(action) = self.shortcut_capture.clone() else {
            return;
        };
        if key.eq_ignore_ascii_case("escape") {
            self.shortcut_capture = None;
            self.shortcut_status = None;
            cx.notify();
            return;
        }
        let Some(canonical) = canonical_keystroke(
            modifiers.control,
            modifiers.alt,
            modifiers.shift,
            modifiers.platform,
            modifiers.function,
            key,
        ) else {
            return;
        };
        let bare = key.to_lowercase();
        let fkey =
            bare.len() > 1 && bare.as_bytes()[0] == b'f' && bare[1..].parse::<u32>().is_ok();
        if !(modifiers.control || modifiers.alt || fkey) {
            self.shortcut_status = Some((t().keymap_needs_modifier(), true));
            cx.notify();
            return;
        }
        if let Some(owner) = self.shortcut_owner(&canonical, &action, cx) {
            self.shortcut_status = Some((t().keymap_conflict(&owner, &canonical), true));
            cx.notify();
            return;
        }
        match save_keymap_binding(&crate::settings::data_dir(), &action, &canonical) {
            Ok(()) => {
                self.shortcut_capture = None;
                self.shortcut_status = Some((t().keymap_saved(), false));
                self.apply_effective_bindings(cx);
            }
            Err(err) => {
                self.shortcut_status = Some((err, true));
                cx.notify();
            }
        }
    }

    /// Label of the action holding `keys`, ignoring `except` (for the
    /// collision message).
    fn shortcut_owner(&self, keys: &str, except: &str, cx: &App) -> Option<String> {
        cx.global::<ShortcutStore>()
            .bindings
            .iter()
            .find(|binding| binding.action != except && binding.keys.iter().any(|key| key == keys))
            .map(|binding| match def(&binding.action) {
                Some(entry) => (entry.label)(t()).to_string(),
                None => binding.action.clone(),
            })
    }

    pub(super) fn show_section(&mut self, section: repo::RepoSection, cx: &mut Context<Self>) {
        if self.active_repo().is_none() {
            return;
        }
        let entering_changes = section == repo::RepoSection::LocalChanges
            && self.section != repo::RepoSection::LocalChanges;
        self.section = section;
        if entering_changes {
            // The open diff may have changed while another page was shown.
            self.refresh_open_diff(cx);
        }
        cx.notify();
    }

    pub(super) fn show_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if tabs::text_input_focused(self, window, cx) {
            return;
        }
        self.show_section(repo::RepoSection::History, cx);
    }

    pub(super) fn show_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if tabs::text_input_focused(self, window, cx) {
            return;
        }
        self.show_section(repo::RepoSection::LocalChanges, cx);
    }

    pub(super) fn show_stashes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if tabs::text_input_focused(self, window, cx) {
            return;
        }
        self.show_section(repo::RepoSection::Stashes, cx);
    }

    /// Cheat-sheet card: every effective binding grouped by scope, with the
    /// remap footer. Rendered through the shared modal fade like the other
    /// request cards.
    pub(super) fn render_shortcuts_dialog(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let store = cx.global::<ShortcutStore>();
        let mut groups: Vec<gpui_kit::AnyElement> = Vec::new();
        for group in ShortcutGroup::ORDER {
            let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
            for binding in &store.bindings {
                let Some(entry) = def(&binding.action) else {
                    continue;
                };
                if entry.group != group {
                    continue;
                }
                let label = (entry.label)(t());
                let display = display_for(&binding.action, &binding.keys);
                rows.push(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(div().flex_1().min_w_0().child(label.to_string()))
                        .child(kbd_chip(&display, cx))
                        .into_any_element(),
                );
            }
            if rows.is_empty() {
                continue;
            }
            groups.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(group.label().to_string()),
                    )
                    .children(rows)
                    .into_any_element(),
            );
        }
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0.0, 0.0, 0.0, 0.35))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.cancel_shortcuts(cx)),
            )
            .child(
                div()
                    .w(px(460.))
                    .rounded(px(PANEL_RADIUS))
                    .border_1()
                    .border_color(hairline(0.10))
                    .bg(cx.theme().popover)
                    .shadow_lg()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                gpui_kit::component::Icon::new(IconName::Keyboard)
                                    .size(px(16.))
                                    .text_color(text_muted(cx)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(TEXT_LG))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(text_primary(cx))
                                    .child(t().shortcuts_title),
                            )
                            .child(
                                Button::new("shortcuts-close")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_shortcuts(cx)
                                    })),
                            ),
                    )
                    .children(groups)
                    .child(
                        div()
                            .text_size(px(TEXT_XS))
                            .text_color(text_faint(cx))
                            .child(t().keymap_hint(&keymap_path().display().to_string())),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keystrokes_all_parse() {
        // main.rs builds KeyBinding::new (panicking) from this table, so
        // every default must parse here first.
        for entry in SHORTCUTS {
            for keys in entry.keys {
                for one in keys.split_whitespace() {
                    assert!(
                        Keystroke::parse(one).is_ok(),
                        "\"{one}\" (action \"{}\") must parse",
                        entry.action
                    );
                }
            }
        }
    }

    #[test]
    fn default_keystrokes_have_no_collisions() {
        let mut taken: HashMap<String, String> = HashMap::new();
        for entry in SHORTCUTS {
            for keys in entry.keys {
                if let Some(owner) = taken.get(*keys) {
                    panic!("\"{keys}\" is bound to both \"{owner}\" and \"{}\"", entry.action);
                }
                taken.insert(keys.to_string(), entry.action.to_string());
            }
        }
    }

    #[test]
    fn keymap_overrides_replace_and_report() {
        // A valid override replaces the default and counts as customized.
        let (entries, diagnostics) = parse_keymap_file(
            r#"{"bindings": [{"action": "toggle-palette", "keys": "ctrl-p"}]}"#,
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let (bindings, diagnostics, customized) = merge(entries);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(customized, 1);
        let toggle = bindings
            .iter()
            .find(|binding| binding.action == "toggle-palette")
            .expect("toggle must survive");
        assert_eq!(toggle.keys, vec!["ctrl-p"]);
        assert_eq!(display_for("toggle-palette", &toggle.keys), "Ctrl P");

        // Untouched actions keep their curated display.
        let close = bindings
            .iter()
            .find(|binding| binding.action == "close-dialog")
            .expect("close must survive");
        assert_eq!(display_for("close-dialog", &close.keys), "Esc");
    }

    #[test]
    fn keymap_problems_keep_safe_defaults() {
        // Malformed JSON, unknown actions, bad keystrokes, and collisions
        // all fall back to defaults with a diagnostic each.
        let (entries, diagnostics) = parse_keymap_file("{oops");
        assert_eq!(entries.len(), 0);
        assert_eq!(diagnostics.len(), 1);

        let (entries, diagnostics) = parse_keymap_file(
            r#"{"bindings": [
                {"action": "nope", "keys": "ctrl-q"},
                {"action": "toggle-palette", "keys": " "},
                {"action": "toggle-palette", "keys": ""}
            ]}"#,
        );
        // Blank keystrokes are reported at parse; the unknown action
        // survives parsing and is reported at merge below.
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
        let (bindings, diagnostics, _) = merge(entries);
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.contains("unknown action")),
            "{diagnostics:?}"
        );
        let toggle = bindings
            .iter()
            .find(|binding| binding.action == "toggle-palette")
            .expect("toggle must survive");
        assert_eq!(toggle.keys, vec!["ctrl-k"]);

        // open-settings onto ctrl-k collides with toggle-palette's default:
        // the earlier table entry wins and the loser is reported.
        let (entries, _) = parse_keymap_file(
            r#"{"bindings": [{"action": "open-settings", "keys": "ctrl-k"}]}"#,
        );
        let (bindings, diagnostics, _) = merge(entries);
        let toggle = bindings
            .iter()
            .find(|binding| binding.action == "toggle-palette")
            .expect("toggle must survive");
        assert_eq!(toggle.keys, vec!["ctrl-k"]);
        let settings = bindings
            .iter()
            .find(|binding| binding.action == "open-settings")
            .expect("settings must survive");
        assert!(settings.keys.is_empty(), "{settings:?}");
        assert_eq!(display_for("open-settings", &settings.keys), "Not bound");
        assert!(
            diagnostics.iter().any(|diagnostic| diagnostic.contains("wins")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn prettify_spells_common_chords() {
        assert_eq!(prettify("ctrl-k"), "Ctrl K");
        assert_eq!(prettify("ctrl-shift-tab"), "Ctrl Shift Tab");
        assert_eq!(prettify("shift-/"), "?");
        assert_eq!(prettify("escape"), "Esc");
        assert_eq!(prettify("ctrl-,"), "Ctrl ,");
    }

    #[test]
    fn canonical_keystroke_round_trips_through_parse() {
        // A captured chord rebuilds the canonical string, which parses back
        // to the same modifiers and key.
        let cases = [
            ((true, false, false, false, false, "k"), "ctrl-k"),
            ((true, false, true, false, false, "Tab"), "ctrl-shift-tab"),
            ((false, false, true, false, false, "/"), "shift-/"),
            ((false, true, false, false, false, ","), "alt-,"),
            ((false, false, false, true, false, "p"), "win-p"),
            ((false, false, false, false, false, "f5"), "f5"),
            ((true, false, false, false, false, "F12"), "ctrl-f12"),
        ];
        for ((control, alt, shift, platform, function, key), expect) in cases {
            let canonical =
                canonical_keystroke(control, alt, shift, platform, function, key)
                    .expect("must build");
            assert_eq!(canonical, expect);
            assert!(
                Keystroke::parse(&canonical).is_ok(),
                "\"{canonical}\" must parse"
            );
        }
        // Bare modifiers, empty keys, and Escape can never bind (Escape
        // cancels the capture instead).
        for key in ["", "shift", "Control", "alt", "meta", "Escape", "fn"] {
            assert!(
                canonical_keystroke(false, false, false, false, false, key).is_none()
                    && canonical_keystroke(true, true, true, true, true, key).is_none(),
                "\"{key}\" must not bind"
            );
        }
    }

    #[test]
    fn keymap_file_round_trips_overrides_and_resets() {
        let dir = std::env::temp_dir().join(format!("spur-keymap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let read_bindings = || {
            let text = std::fs::read_to_string(keymap_path_for(&dir)).expect("keymap file");
            let (entries, diagnostics) = parse_keymap_file(&text);
            assert!(diagnostics.is_empty(), "{diagnostics:?}");
            entries
        };
        // Saving creates the file and keeps one entry per action.
        save_keymap_binding(&dir, "toggle-palette", "ctrl-p").expect("save");
        save_keymap_binding(&dir, "toggle-palette", "ctrl-q").expect("re-save");
        save_keymap_binding(&dir, "open-settings", "ctrl-p").expect("save");
        let entries = read_bindings();
        assert_eq!(entries.len(), 2, "{entries:?}");
        assert!(entries.contains(&("toggle-palette".to_string(), vec!["ctrl-q".to_string()])));
        // Resetting drops the entry; resetting again is a no-op.
        reset_keymap_binding(&dir, "toggle-palette").expect("reset");
        reset_keymap_binding(&dir, "toggle-palette").expect("reset again");
        let entries = read_bindings();
        assert_eq!(entries.len(), 1, "{entries:?}");
        // A malformed file is refused, never overwritten.
        std::fs::write(keymap_path_for(&dir), "{oops").expect("break the file");
        assert!(save_keymap_binding(&dir, "toggle-palette", "ctrl-p").is_err());
        assert!(reset_keymap_binding(&dir, "toggle-palette").is_err());
        assert_eq!(
            std::fs::read_to_string(keymap_path_for(&dir)).expect("file"),
            "{oops"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
