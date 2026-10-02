//! Pure animation sampling: state resolution and the built-in effects.
//! Everything here is a function of (definition, state, time); nothing
//! accumulates frame to frame, and nothing touches GPUI.
//!
//! Effect combination is deterministic: state values first, then each effect
//! multiplies the current size/opacity. A looping effect is evaluated from a
//! wrapped phase, so the cycle boundary is continuous.

use super::{Animation, ColorRole, Dot, DotShape, Effect, IconDef, StateValue};

/// One sampled dot for a frame, in normalized coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct SampledDot {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub opacity: f32,
    pub role: ColorRole,
    pub shape: DotShape,
}

/// A per-dot adjustment returned by an effect. Values multiply the current
/// size and opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fx {
    pub r: f32,
    pub opacity: f32,
}

impl Fx {
    pub const IDENTITY: Fx = Fx { r: 1.0, opacity: 1.0 };
}

/// Zeron's cosine pulse: 0 at phase 0, 1 at phase 0.5, back to 0 at phase 1.
/// Continuous across the cycle boundary by construction.
pub fn pulse_wave(phase: f32) -> f32 {
    0.5 - 0.5 * (phase * std::f32::consts::TAU).cos()
}

/// Resolve a state to sampled dots without animation (the reduced-motion and
/// static-preview path).
pub fn sample_state(def: &IconDef, state: &str) -> Vec<SampledDot> {
    def.resolved_state(state)
        .into_iter()
        .map(|(id, value)| {
            let dot = def.dot(&id);
            SampledDot {
                id,
                x: dot.map(|d| d.x).unwrap_or(0.5),
                y: dot.map(|d| d.y).unwrap_or(0.5),
                r: value.r,
                opacity: value.opacity.clamp(0.0, 1.0),
                role: value.role,
                shape: dot.map(|d| d.shape).unwrap_or_default(),
            }
        })
        .collect()
}

/// Sample an animation at `time_ms`, applying its effects on top of the
/// supplied state values. `looping` is explicit so the player can hold a
/// one-shot playback at its final frame even when the preset loops.
pub(crate) fn apply_effects_with(
    def: &IconDef,
    animation: &Animation,
    time_ms: f32,
    values: &[(String, StateValue)],
    looping: bool,
) -> Vec<SampledDot> {
    let raw = if animation.duration_ms == 0 {
        0.0
    } else {
        time_ms / animation.duration_ms as f32
    };
    let phase = if looping {
        raw.rem_euclid(1.0)
    } else {
        raw.clamp(0.0, 1.0)
    };

    def.dots
        .iter()
        .enumerate()
        .map(|(index, dot)| {
            let mut value = values
                .iter()
                .find(|(id, _)| id == &dot.id)
                .map(|(_, v)| *v)
                .unwrap_or_else(|| dot.state_value());
            for effect in &animation.effects {
                let fx = eval_effect(effect, dot, index, phase);
                value.r *= fx.r;
                value.opacity *= fx.opacity;
            }
            SampledDot {
                id: dot.id.clone(),
                x: dot.x,
                y: dot.y,
                r: value.r.max(0.0),
                opacity: value.opacity.clamp(0.0, 1.0),
                role: value.role,
                shape: dot.shape,
            }
        })
        .collect()
}

fn eval_effect(effect: &Effect, dot: &Dot, index: usize, phase: f32) -> Fx {
    match effect {
        Effect::Pulse {
            intensity,
            phase_offset,
            stagger,
            easing,
        } => {
            let local = (phase + phase_offset + index as f32 * stagger).rem_euclid(1.0);
            let wave = pulse_wave(easing.eval(local));
            let intensity = intensity.clamp(0.0, 1.0);
            Fx {
                r: 1.0 - intensity * 0.3 * (1.0 - wave),
                opacity: 1.0 - intensity * (1.0 - wave),
            }
        }
        Effect::Sweep {
            direction_deg,
            width,
            intensity,
            easing,
        } => {
            let angle = direction_deg.to_radians();
            let (sin, cos) = angle.sin_cos();
            // Projection of the dot centre onto the sweep direction,
            // normalized to 0..1 across the icon box.
            let extent = cos.abs() + sin.abs();
            let s = if extent <= f32::EPSILON {
                0.5
            } else {
                ((dot.x * cos + dot.y * sin) / extent).clamp(0.0, 1.0)
            };
            let local = (phase - s).rem_euclid(1.0);
            let width = width.clamp(0.05, 1.0);
            let brightness = if local < width {
                pulse_wave(easing.eval(local / width))
            } else {
                0.0
            };
            let intensity = intensity.clamp(0.0, 1.0);
            Fx {
                r: 1.0,
                opacity: 1.0 - intensity * (1.0 - brightness),
            }
        }
        Effect::Chase {
            order,
            intensity,
            easing,
        } => {
            let intensity = intensity.clamp(0.0, 1.0);
            let len = order.len().max(1) as f32;
            match order.iter().position(|id| id == &dot.id) {
                Some(ix) => {
                    let local = (phase - ix as f32 / len).rem_euclid(1.0);
                    let wave = pulse_wave(easing.eval(local));
                    Fx {
                        r: 1.0,
                        opacity: 1.0 - intensity * (1.0 - wave),
                    }
                }
                // Dots outside the explicit order rest dimmed.
                None => Fx {
                    r: 1.0,
                    opacity: 1.0 - intensity,
                },
            }
        }
        Effect::Custom { .. } => Fx::IDENTITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icon::Easing;

    fn apply_effects(
        def: &IconDef,
        animation: &Animation,
        time_ms: f32,
        values: &[(String, StateValue)],
    ) -> Vec<SampledDot> {
        apply_effects_with(def, animation, time_ms, values, animation.looping)
    }

    fn icon() -> IconDef {
        let mut def = IconDef {
            name: "Test".into(),
            dots: vec![
                Dot::new("a", 0.1, 0.5, 0.1),
                Dot::new("b", 0.5, 0.5, 0.1),
                Dot::new("c", 0.9, 0.5, 0.1),
            ],
            ..IconDef::default()
        };
        def.animations = vec![Animation::new("loop", 1000)];
        def
    }

    #[test]
    fn sampling_is_deterministic() {
        let mut def = icon();
        def.animations[0].effects.push(Effect::default_pulse());
        let anim = def.animations[0].clone();
        let values = def.resolved_state("rest");
        let first = apply_effects(&def, &anim, 333.0, &values);
        let second = apply_effects(&def, &anim, 333.0, &values);
        assert_eq!(first, second);
    }

    #[test]
    fn looping_effects_are_continuous_at_the_cycle_boundary() {
        let mut def = icon();
        def.animations[0].effects.push(Effect::default_sweep());
        let anim = def.animations[0].clone();
        let values = def.resolved_state("rest");
        let start = apply_effects(&def, &anim, 0.0, &values);
        let end = apply_effects(&def, &anim, 1000.0, &values);
        let wrap = apply_effects(&def, &anim, 2000.0, &values);
        for ((a, b), c) in end.iter().zip(start.iter()).zip(wrap.iter()) {
            assert!(
                (a.opacity - b.opacity).abs() < 1e-5,
                "sweep: cycle end {} != start {} for {}",
                a.opacity,
                b.opacity,
                a.id
            );
            assert!((b.opacity - c.opacity).abs() < 1e-5, "sweep: next cycle differs");
        }
        let mut pulse = def.clone();
        pulse.animations[0].effects = vec![Effect::default_pulse()];
        let values = pulse.resolved_state("rest");
        let start = apply_effects(&pulse, &pulse.animations[0], 0.0, &values);
        let end = apply_effects(&pulse, &pulse.animations[0], 1000.0, &values);
        for (a, b) in end.iter().zip(start.iter()) {
            assert!((a.opacity - b.opacity).abs() < 1e-5, "pulse boundary {}", a.id);
            assert!((a.r - b.r).abs() < 1e-5, "pulse size boundary {}", a.id);
        }
    }

    #[test]
    fn sweep_brightness_travels_along_the_direction() {
        let def = icon();
        let anim = Animation {
            name: "sweep".into(),
            duration_ms: 1000,
            looping: true,
            effects: vec![Effect::default_sweep()], // 0 deg = left to right
        };
        let values = def.resolved_state("rest");
        let brightest = |time_ms: f32| {
            let frame = apply_effects(&def, &anim, time_ms, &values);
            frame
                .iter()
                .max_by(|a, b| a.opacity.total_cmp(&b.opacity))
                .unwrap()
                .id
                .clone()
        };
        // The wave front enters at the left edge and passes each dot in turn.
        assert_eq!(brightest(250.0), "a");
        assert_eq!(brightest(650.0), "b");
        assert_eq!(brightest(1050.0), "c");
    }

    #[test]
    fn chase_follows_the_explicit_order_not_the_array() {
        let def = icon();
        let anim = Animation {
            name: "chase".into(),
            duration_ms: 1000,
            looping: true,
            effects: vec![Effect::default_chase(vec!["c".into(), "a".into(), "b".into()])],
        };
        let values = def.resolved_state("rest");
        // Dot "c" leads (order index 0), so it peaks a quarter period before
        // "a" (index 1): sample at the phase where "c" is at its crest.
        let frame = apply_effects(&def, &anim, 500.0, &values);
        let get = |id: &str| frame.iter().find(|d| d.id == id).unwrap().opacity;
        assert!(get("c") > get("a"), "c {} a {}", get("c"), get("a"));
    }

    #[test]
    fn custom_effects_sample_as_identity() {
        let def = icon();
        let anim = Animation {
            name: "custom".into(),
            duration_ms: 1000,
            looping: true,
            effects: vec![Effect::Custom {
                name: "not_shipped".into(),
                params: Default::default(),
            }],
        };
        let values = def.resolved_state("rest");
        let frame = apply_effects(&def, &anim, 0.0, &values);
        assert!(frame.iter().all(|d| (d.opacity - 1.0).abs() < 1e-6));
    }

    #[test]
    fn one_shot_effects_hold_their_final_value() {
        let mut def = icon();
        def.animations[0].looping = false;
        def.animations[0].effects = vec![Effect::Pulse {
            intensity: 1.0,
            phase_offset: 0.0,
            stagger: 0.0,
            easing: Easing::Linear,
        }];
        let values = def.resolved_state("rest");
        let last = apply_effects(&def, &def.animations[0], 1000.0, &values);
        let beyond = apply_effects(&def, &def.animations[0], 5000.0, &values);
        assert_eq!(last, beyond);
    }
}
