//! Animated dot-matrix icons: geometry, named states, animation presets, and
//! the versioned JSON format (v1).
//!
//! This module is GPUI-free on purpose: rendering lives in
//! `src/ui/icon_view.rs`.
//!
//! Attribution: the animation vocabulary — staggered cell phases, the cosine
//! pulse wave, and explicit per-cell order — is adapted from Zeron's loader
//! motion (`zeronsh/zeron` `crates/proto/src/motion.rs`,
//! `crates/ui/src/loaders.rs`, `crates/ui/src/motion.rs`, MIT). The
//! application architecture around it is Spur's own; nothing from Zeron's
//! runtime is imported.

pub mod player;
pub mod sample;

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Marker that identifies a Spur icon document.
pub const FORMAT: &str = "spur.icon";
/// Current format version. Readers accept equal or older versions.
pub const VERSION: u32 = 1;

/// Grid extents the validator allows.
pub const MIN_GRID: u16 = 3;
pub const MAX_GRID: u16 = 24;
/// Spacing compresses the pattern about the icon centre. 1.0 fills the box
/// (edge cells stay inside), so the range is bounded at 1.0 — values above it
/// would clamp dots onto the border.
pub const MIN_SPACING: f32 = 0.5;
pub const MAX_SPACING: f32 = 1.0;

/// Icon surface a dot is painted on. Colors are theme-relative roles, never
/// absolute values, so one definition reads correctly in every theme.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorRole {
    #[default]
    Text,
    Muted,
    Faint,
    Accent,
    Success,
    Warning,
    Danger,
}

/// Editing grid metadata. Dot positions are normalized (0..1), so the grid
/// only guides placement while authoring and never moves dots.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub spacing: f32,
}

impl Default for Grid {
    fn default() -> Self {
        Self {
            cols: 7,
            rows: 7,
            spacing: 1.0,
        }
    }
}

/// Shape of a dot. Geometry, not state: a state override cannot change it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DotShape {
    #[default]
    Circle,
    RoundedSquare,
}

fn one() -> f32 {
    1.0
}

/// One dot of the icon, in normalized coordinates.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Dot {
    /// Stable identity; states and chase orders reference this, never an index.
    pub id: String,
    /// Centre X across the icon box, 0..1.
    pub x: f32,
    /// Centre Y across the icon box, 0..1.
    pub y: f32,
    /// Radius as a fraction of the box's shorter side.
    pub r: f32,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default)]
    pub role: ColorRole,
    #[serde(default)]
    pub shape: DotShape,
}

impl Dot {
    #[cfg(test)]
    pub fn new(id: impl Into<String>, x: f32, y: f32, r: f32) -> Self {
        Self {
            id: id.into(),
            x,
            y,
            r,
            opacity: 1.0,
            role: ColorRole::default(),
            shape: DotShape::default(),
        }
    }

    #[cfg(test)]
    pub fn with_role(mut self, role: ColorRole) -> Self {
        self.role = role;
        self
    }

    /// Base values as a state value.
    pub fn state_value(&self) -> StateValue {
        StateValue {
            r: self.r,
            opacity: self.opacity,
            role: self.role,
        }
    }
}

/// A state's override for one dot. Missing fields inherit the dot's base value.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DotOverride {
    pub id: String,
    #[serde(default)]
    pub r: Option<f32>,
    #[serde(default)]
    pub opacity: Option<f32>,
    #[serde(default)]
    pub role: Option<ColorRole>,
}

/// A named state: zero or more overrides over the base dots.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StateDef {
    pub name: String,
    #[serde(default)]
    pub values: Vec<DotOverride>,
}

impl StateDef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            values: Vec::new(),
        }
    }
}

/// One resolved value per dot (the transition's currency).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StateValue {
    pub r: f32,
    pub opacity: f32,
    pub role: ColorRole,
}

/// Easing curves for sampling. All are pure and defined at both ends, so a
/// looping effect is continuous across its cycle boundary.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    #[default]
    Linear,
    Sine,
    EaseInOut,
    EaseOut,
}

impl Easing {
    /// `t` is clamped to 0..1; every curve maps 0 -> 0 and 1 -> 1.
    pub fn eval(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::Sine => 0.5 - 0.5 * (std::f32::consts::PI * t).cos(),
            Easing::EaseInOut => t * t * (3.0 - 2.0 * t),
            Easing::EaseOut => 1.0 - (1.0 - t).powi(3),
        }
    }
}

/// One animation effect. Parameters are plain data, never code.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    /// Opacity and size breathe with a cosine wave.
    Pulse {
        #[serde(default = "one")]
        intensity: f32,
        #[serde(default)]
        phase_offset: f32,
        #[serde(default)]
        stagger: f32,
        #[serde(default)]
        easing: Easing,
    },
    /// Brightness travels along `direction_deg`.
    Sweep {
        #[serde(default)]
        direction_deg: f32,
        #[serde(default = "default_width")]
        width: f32,
        #[serde(default = "one")]
        intensity: f32,
        #[serde(default)]
        easing: Easing,
    },
    /// Brightness follows an explicit dot order.
    Chase {
        #[serde(default)]
        order: Vec<String>,
        #[serde(default = "one")]
        intensity: f32,
        #[serde(default)]
        easing: Easing,
    },
    /// A named Rust effect. This build ships none, so it samples as identity
    /// and the file still loads.
    Custom {
        name: String,
        #[serde(default)]
        params: std::collections::BTreeMap<String, f32>,
    },
}

fn default_width() -> f32 {
    0.25
}

#[cfg(test)]
impl Effect {
    pub fn default_pulse() -> Self {
        Effect::Pulse {
            intensity: 0.7,
            phase_offset: 0.0,
            stagger: 0.08,
            easing: Easing::Sine,
        }
    }

    pub fn default_sweep() -> Self {
        Effect::Sweep {
            direction_deg: 0.0,
            width: 0.3,
            intensity: 0.85,
            easing: Easing::Sine,
        }
    }

    pub fn default_chase(order: Vec<String>) -> Self {
        Effect::Chase {
            order,
            intensity: 0.9,
            easing: Easing::Sine,
        }
    }
}

/// One named animation preset.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Animation {
    pub name: String,
    pub duration_ms: u32,
    #[serde(default = "yes")]
    pub looping: bool,
    #[serde(default)]
    pub effects: Vec<Effect>,
}

fn yes() -> bool {
    true
}

impl Animation {
    pub fn new(name: impl Into<String>, duration_ms: u32) -> Self {
        Self {
            name: name.into(),
            duration_ms,
            looping: true,
            effects: Vec::new(),
        }
    }
}

/// Named-state transition settings.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct TransitionDef {
    pub duration_ms: u32,
    #[serde(default)]
    pub easing: Easing,
}

impl Default for TransitionDef {
    fn default() -> Self {
        Self {
            duration_ms: 260,
            easing: Easing::EaseInOut,
        }
    }
}

/// A complete icon document.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IconDef {
    pub format: String,
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub grid: Grid,
    pub dots: Vec<Dot>,
    #[serde(default)]
    pub states: Vec<StateDef>,
    #[serde(default)]
    pub animations: Vec<Animation>,
    #[serde(default)]
    pub transition: TransitionDef,
}

impl Default for IconDef {
    fn default() -> Self {
        Self {
            format: FORMAT.to_string(),
            version: VERSION,
            name: "Untitled".to_string(),
            grid: Grid::default(),
            dots: Vec::new(),
            states: vec![StateDef::new("rest")],
            animations: vec![Animation::new("pulse", 2400)],
            transition: TransitionDef::default(),
        }
    }
}

impl IconDef {
    /// Parse and validate a document. The caller keeps its current document
    /// unless this returns `Ok`, so malformed files can never destroy work.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let def: IconDef =
            serde_json::from_str(text).map_err(|e| format!("invalid icon JSON: {e}"))?;
        def.validate()?;
        Ok(def)
    }
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        Self::from_json(&text)
    }

    pub fn state(&self, name: &str) -> Option<&StateDef> {
        self.states.iter().find(|s| s.name == name)
    }

    pub fn dot(&self, id: &str) -> Option<&Dot> {
        self.dots.iter().find(|d| d.id == id)
    }

    /// Resolved values for one named state: every dot's base value, then the
    /// state's overrides. Dots not mentioned by a state stay visible.
    pub fn resolved_state(&self, state: &str) -> Vec<(String, StateValue)> {
        let overrides = self.state(state);
        self.dots
            .iter()
            .map(|d| {
                let mut value = d.state_value();
                if let Some(ov) = overrides.and_then(|s| s.values.iter().find(|o| o.id == d.id)) {
                    if let Some(r) = ov.r {
                        value.r = r;
                    }
                    if let Some(opacity) = ov.opacity {
                        value.opacity = opacity;
                    }
                    if let Some(role) = ov.role {
                        value.role = role;
                    }
                }
                (d.id.clone(), value)
            })
            .collect()
    }

    /// Structural validation. Every rule has a human-readable message so a
    /// bad file reports exactly what is wrong.
    pub fn validate(&self) -> Result<(), String> {
        if self.format != FORMAT {
            return Err(format!(
                "not a Spur icon file (format is \"{}\", expected \"{FORMAT}\")",
                self.format
            ));
        }
        if self.version > VERSION {
            return Err(format!(
                "icon format version {} is newer than this build supports ({VERSION})",
                self.version
            ));
        }
        if self.name.trim().is_empty() {
            return Err("icon name is empty".to_string());
        }
        if !(MIN_GRID..=MAX_GRID).contains(&self.grid.cols)
            || !(MIN_GRID..=MAX_GRID).contains(&self.grid.rows)
        {
            return Err(format!(
                "grid dimensions must be {}..{MAX_GRID} (got {}x{})",
                MIN_GRID, self.grid.cols, self.grid.rows
            ));
        }
        if !self.grid.spacing.is_finite()
            || !(MIN_SPACING..=MAX_SPACING).contains(&self.grid.spacing)
        {
            return Err(format!(
                "grid spacing must be {MIN_SPACING}..{MAX_SPACING} (got {})",
                self.grid.spacing
            ));
        }
        if self.dots.is_empty() {
            return Err("icon has no dots".to_string());
        }
        if self.dots.len() > 4096 {
            return Err("icon has too many dots (max 4096)".to_string());
        }
        let mut ids = HashSet::new();
        for dot in &self.dots {
            if dot.id.trim().is_empty() {
                return Err("a dot has an empty id".to_string());
            }
            if !ids.insert(dot.id.as_str()) {
                return Err(format!("duplicate dot id \"{}\"", dot.id));
            }
            if !valid_unit(dot.x) || !valid_unit(dot.y) {
                return Err(format!("dot \"{}\" is outside the 0..1 box", dot.id));
            }
            if !dot.r.is_finite() || dot.r <= 0.0 || dot.r > 0.5 {
                return Err(format!("dot \"{}\" radius must be 0..0.5", dot.id));
            }
            if !valid_unit(dot.opacity) {
                return Err(format!("dot \"{}\" opacity must be 0..1", dot.id));
            }
        }
        let mut state_names = HashSet::new();
        for state in &self.states {
            if state.name.trim().is_empty() {
                return Err("a state has an empty name".to_string());
            }
            if !state_names.insert(state.name.as_str()) {
                return Err(format!("duplicate state name \"{}\"", state.name));
            }
            let mut seen = HashSet::new();
            for ov in &state.values {
                if !ids.contains(ov.id.as_str()) {
                    return Err(format!(
                        "state \"{}\" references unknown dot \"{}\"",
                        state.name, ov.id
                    ));
                }
                if !seen.insert(ov.id.as_str()) {
                    return Err(format!(
                        "state \"{}\" overrides dot \"{}\" twice",
                        state.name, ov.id
                    ));
                }
                if let Some(r) = ov.r
                    && (!r.is_finite() || r <= 0.0 || r > 0.5)
                {
                    return Err(format!(
                        "state \"{}\" radius for \"{}\" must be 0..0.5",
                        state.name, ov.id
                    ));
                }
                if let Some(opacity) = ov.opacity
                    && !valid_unit(opacity)
                {
                    return Err(format!(
                        "state \"{}\" opacity for \"{}\" must be 0..1",
                        state.name, ov.id
                    ));
                }
            }
        }
        let mut anim_names = HashSet::new();
        for anim in &self.animations {
            if anim.name.trim().is_empty() {
                return Err("an animation has an empty name".to_string());
            }
            if !anim_names.insert(anim.name.as_str()) {
                return Err(format!("duplicate animation name \"{}\"", anim.name));
            }
            if anim.duration_ms == 0 || anim.duration_ms > 60_000 {
                return Err(format!(
                    "animation \"{}\" duration must be 1..60000 ms",
                    anim.name
                ));
            }
            for effect in &anim.effects {
                if let Effect::Chase { order, .. } = effect {
                    let mut seen = HashSet::new();
                    for id in order {
                        if !ids.contains(id.as_str()) {
                            return Err(format!(
                                "animation \"{}\" chase order references unknown dot \"{id}\"",
                                anim.name
                            ));
                        }
                        if !seen.insert(id.as_str()) {
                            return Err(format!(
                                "animation \"{}\" chase order repeats dot \"{id}\"",
                                anim.name
                            ));
                        }
                    }
                }
                if let Effect::Custom { name, .. } = effect
                    && name.trim().is_empty()
                {
                    return Err(format!(
                        "animation \"{}\" has a custom effect with no name",
                        anim.name
                    ));
                }
            }
        }
        if self.transition.duration_ms > 10_000 {
            return Err("transition duration must be at most 10000 ms".to_string());
        }
        Ok(())
    }
}

fn valid_unit(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> IconDef {
        IconDef {
            name: "Tiny".into(),
            dots: vec![
                Dot::new("a", 0.25, 0.25, 0.1),
                Dot::new("b", 0.75, 0.5, 0.1).with_role(ColorRole::Accent),
            ],
            states: vec![StateDef::new("rest"), StateDef::new("dim")],
            animations: vec![Animation::new("pulse", 1000)],
            ..IconDef::default()
        }
    }

    #[test]
    fn easing_endpoints_hold_for_every_curve() {
        for easing in [Easing::Linear, Easing::Sine, Easing::EaseInOut, Easing::EaseOut] {
            assert_eq!(easing.eval(0.0), 0.0, "{easing:?}");
            assert_eq!(easing.eval(1.0), 1.0, "{easing:?}");
            assert_eq!(easing.eval(-1.0), 0.0, "{easing:?}");
            assert_eq!(easing.eval(2.0), 1.0, "{easing:?}");
        }
    }

    #[test]
    fn validation_accepts_a_tiny_icon_and_resolves_state_overrides() {
        let mut def = tiny();
        def.states[1].values.push(DotOverride {
            id: "a".into(),
            r: Some(0.05),
            opacity: Some(0.0),
            role: None,
        });
        def.validate().unwrap();
        let dim = def.resolved_state("dim");
        assert_eq!(dim[0].0, "a");
        assert_eq!(dim[0].1.opacity, 0.0);
        assert_eq!(dim[0].1.r, 0.05);
        assert_eq!(dim[1].1.opacity, 1.0, "untouched dots keep base values");
    }

    #[test]
    fn malformed_definitions_are_rejected_with_reasons() {
        let err = IconDef::from_json("{ not json").unwrap_err();
        assert!(err.contains("invalid icon JSON"), "{err}");

        let mut def = tiny();
        def.dots.push(Dot::new("a", 0.5, 0.5, 0.1));
        assert!(def.validate().unwrap_err().contains("duplicate dot id"));

        let mut def = tiny();
        def.format = "someone.else".into();
        assert!(def.validate().unwrap_err().contains("not a Spur icon"));

        let mut def = tiny();
        def.states[0].values.push(DotOverride {
            id: "ghost".into(),
            r: None,
            opacity: Some(0.5),
            role: None,
        });
        assert!(def.validate().unwrap_err().contains("unknown dot"));

        let mut def = tiny();
        def.animations[0].effects.push(Effect::Chase {
            order: vec!["a".into(), "a".into()],
            intensity: 1.0,
            easing: Easing::Linear,
        });
        assert!(def.validate().unwrap_err().contains("repeats dot"));

        let mut def = tiny();
        def.dots[0].x = 1.5;
        assert!(def.validate().unwrap_err().contains("outside the 0..1"));
    }

    #[test]
    fn dot_shape_defaults_to_circle() {
        // Files written before the shape field existed load as circles.
        let json = r#"{
            "format": "spur.icon",
            "version": 1,
            "name": "Shapes",
            "dots": [
                { "id": "a", "x": 0.5, "y": 0.5, "r": 0.1 },
                { "id": "b", "x": 0.2, "y": 0.2, "r": 0.1, "shape": "rounded_square" }
            ]
        }"#;
        let parsed = IconDef::from_json(json).unwrap();
        assert_eq!(parsed.dots[0].shape, DotShape::Circle);
        assert_eq!(parsed.dots[1].shape, DotShape::RoundedSquare);
    }
}
