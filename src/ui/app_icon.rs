//! App icon slots: every icon the shell renders, which definition backs it,
//! and how it plays.
//!
//! Production defaults are the kit's icon set. Custom definitions and their
//! playback bindings persist under `%APPDATA%\SpurGit` and are honored on
//! load. One `IconView` entity exists per slot so an icon's clock is
//! persistent and shared by all of its dots.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::{
    AnyElement, App, AppContext as _, Entity, Global, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::icon::IconDef;

use super::icon_view::{IconView, PlayMode};

/// Every replaceable icon in the application shell. The brand character is
/// not in this list on purpose: it is not a dot icon and cannot be replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconSlot {
    Refresh,
    Fetch,
    Settings,
    OpenRepo,
    PaletteRoots,
    PaletteExplorer,
    PaletteSettings,
}

impl IconSlot {
    pub const ALL: [IconSlot; 7] = [
        IconSlot::Refresh,
        IconSlot::Fetch,
        IconSlot::Settings,
        IconSlot::OpenRepo,
        IconSlot::PaletteRoots,
        IconSlot::PaletteExplorer,
        IconSlot::PaletteSettings,
    ];

    /// Stable key used by the bindings file and the definition filename.
    pub fn key(self) -> &'static str {
        match self {
            IconSlot::Refresh => "refresh",
            IconSlot::Fetch => "fetch",
            IconSlot::Settings => "settings",
            IconSlot::OpenRepo => "open_repo",
            IconSlot::PaletteRoots => "palette_roots",
            IconSlot::PaletteExplorer => "palette_explorer",
            IconSlot::PaletteSettings => "palette_settings",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|slot| slot.key() == key)
    }

    /// The kit icon asset a slot renders while no custom definition is bound.
    pub fn builtin(self) -> IconName {
        match self {
            IconSlot::Refresh => IconName::RotateCw,
            IconSlot::Fetch => IconName::ArrowDown,
            IconSlot::Settings => IconName::Settings,
            IconSlot::OpenRepo => IconName::Plus,
            IconSlot::PaletteRoots => IconName::FolderOpen,
            IconSlot::PaletteExplorer => IconName::ExternalLink,
            IconSlot::PaletteSettings => IconName::Settings,
        }
    }

    pub fn size(self) -> f32 {
        match self {
            IconSlot::PaletteRoots | IconSlot::PaletteExplorer | IconSlot::PaletteSettings => {
                15.0
            }
            _ => 14.0,
        }
    }
}

/// Per-slot binding: source and playback.
#[derive(Clone, Copy, Debug, Default)]
pub struct SlotBinding {
    pub custom: bool,
    pub mode: PlayMode,
}

pub struct AppIconStore {
    bindings: HashMap<IconSlot, SlotBinding>,
    defs: HashMap<IconSlot, IconDef>,
    entities: HashMap<IconSlot, Entity<IconView>>,
}

impl Global for AppIconStore {}

impl AppIconStore {
    fn binding(&self, slot: IconSlot) -> SlotBinding {
        self.bindings.get(&slot).copied().unwrap_or_default()
    }

    fn resolved_def(&self, slot: IconSlot) -> IconDef {
        // Only a bound custom definition can be a dot icon; every shipped
        // slot renders a kit asset until one is bound.
        self.defs.get(&slot).cloned().unwrap_or_default()
    }
}

// ---- lifecycle ----

pub fn init(cx: &mut App) {
    let (bindings, defs) = load();
    let mut store = AppIconStore {
        bindings,
        defs,
        entities: HashMap::new(),
    };
    let now = Instant::now();
    for slot in IconSlot::ALL {
        let binding = store.binding(slot);
        let mut view = IconView::new(store.resolved_def(slot), "rest");
        view.set_size(slot.size());
        view.set_mode(binding.mode, now);
        store.entities.insert(slot, cx.new(|_| view));
    }
    cx.set_global(store);
}

// ---- rendering ----

/// Render a slot's icon. Asset-backed slots without a replacement render the
/// kit icon in `color`; everything else renders its persistent `IconView`
/// with the bound play mode and hover/click triggers.
pub fn render(slot: IconSlot, color: Hsla, cx: &App) -> AnyElement {
    let store = cx.global::<AppIconStore>();
    let binding = store.binding(slot);
    if !binding.custom {
        let icon = slot.builtin();
        return Icon::new(icon)
            .size(px(slot.size()))
            .text_color(color)
            .into_any_element();
    }
    let Some(entity) = store.entities.get(&slot).cloned() else {
        return div().size(px(slot.size())).into_any_element();
    };
    let wrapper = div()
        .id(("app-icon", slot as u32))
        .flex_none()
        .size(px(slot.size()));
    match binding.mode {
        PlayMode::Hover => {
            let hovered_entity = entity.clone();
            wrapper
                .on_hover(move |hovered, _, cx| {
                    let now = Instant::now();
                    hovered_entity.update(cx, |view, cx| {
                        if *hovered {
                            view.begin_play(now);
                        } else {
                            view.end_play(now);
                        }
                        cx.notify();
                    });
                })
                .child(entity)
                .into_any_element()
        }
        PlayMode::Click => {
            let clicked_entity = entity.clone();
            wrapper
                .on_click(move |_, _, cx| {
                    let now = Instant::now();
                    clicked_entity.update(cx, |view, cx| {
                        view.restart_once(now);
                        cx.notify();
                    });
                })
                .child(entity)
                .into_any_element()
        }
        _ => wrapper.child(entity).into_any_element(),
    }
}

// ---- persistence ----

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct BindingsFile {
    #[serde(default)]
    slots: HashMap<String, SlotEntry>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct SlotEntry {
    #[serde(default)]
    custom: bool,
    #[serde(default)]
    mode: String,
}

fn bindings_path() -> PathBuf {
    crate::settings::data_dir().join("icons.json")
}

fn icons_dir() -> PathBuf {
    crate::settings::data_dir().join("icons")
}

fn def_path(slot: IconSlot) -> PathBuf {
    icons_dir().join(format!("{}.json", slot.key()))
}

fn load() -> (HashMap<IconSlot, SlotBinding>, HashMap<IconSlot, IconDef>) {
    let mut bindings = HashMap::new();
    let mut defs = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(bindings_path())
        && let Ok(file) = serde_json::from_str::<BindingsFile>(&text)
    {
        for (key, entry) in file.slots {
            let Some(slot) = IconSlot::from_key(&key) else {
                continue;
            };
            let mode = PlayMode::from_key(&entry.mode).unwrap_or_default();
            bindings.insert(slot, SlotBinding { custom: entry.custom, mode });
        }
    }
    for slot in IconSlot::ALL {
        if !bindings.get(&slot).map(|b| b.custom).unwrap_or(false) {
            continue;
        }
        match IconDef::from_file(&def_path(slot)) {
            Ok(def) => {
                defs.insert(slot, def);
            }
            Err(err) => {
                // A broken replacement must not take the app icon down.
                crate::logging::log!("ignored icon definition for {}: {err}", slot.key());
                if let Some(binding) = bindings.get_mut(&slot) {
                    binding.custom = false;
                }
            }
        }
    }
    (bindings, defs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_keys_are_unique_and_round_trip() {
        let mut keys = Vec::new();
        for slot in IconSlot::ALL {
            assert!(!keys.contains(&slot.key()), "duplicate {}", slot.key());
            keys.push(slot.key());
            assert_eq!(IconSlot::from_key(slot.key()), Some(slot));
        }
        assert_eq!(IconSlot::from_key("nope"), None);
    }

    #[test]
    fn play_modes_round_trip() {
        for mode in PlayMode::ALL {
            assert_eq!(PlayMode::from_key(mode.key()), Some(mode));
        }
        assert_eq!(PlayMode::from_key("sometimes"), None);
    }
}
