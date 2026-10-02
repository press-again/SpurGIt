//! Small motion vocabulary for SpurGit.
//!
//! Adapted from Zeron's motion system (`zeronsh/zeron` crates/ui/src/motion.rs
//! @ `1fdcfe19`, MIT): the Tailwind hover-blend tween (150 ms,
//! cubic-bezier(0.4, 0, 0.2, 1)) and the catalog timings used here
//! (menu 140/100 ms, dialog 180 ms, ease `0.25, 0.1, 0.25, 1`). Curves are
//! evaluated with the GPUI version bundled here, so the numbers — not the
//! names — define the shape.
//!
//! Two mechanisms:
//! - [`hover_blend`] fades colors between rest and hover from the current
//!   displayed value, so a pointer that flips mid-fade never jumps.
//! - [`Transition`] tweens a scalar (the dialog fade) and re-anchors on
//!   interruption, so reopen/close spam cannot replay an entrance or flash.
//!
//! `with_animation` already obeys [`gpui_kit::App::reduce_motion`]; the
//! manual tweens take a `reduced` flag and snap when it is set.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui_kit::{App, Hsla, Rgba, Window};

/// Hover wash blend: 150 ms over cubic-bezier(0.4, 0, 0.2, 1).
const HOVER_FADE: Duration = Duration::from_millis(150);
/// Menu/panel entrance (Zeron MENU_IN).
pub(super) const MENU_IN: Duration = Duration::from_millis(140);
/// Menu/panel exit — quicker than the entrance (Zeron MENU_OUT).
pub(super) const MENU_OUT: Duration = Duration::from_millis(100);

/// CSS `cubic-bezier(0.4, 0, 0.2, 1)` — Tailwind's `transition-colors` curve.
fn ease_tailwind() -> impl Fn(f32) -> f32 {
    gpui_kit::base::animation::cubic_bezier(0.4, 0.0, 0.2, 1.0)
}

/// CSS `ease` — quick fades and menu/dialog pops.
fn ease_menu() -> impl Fn(f32) -> f32 {
    gpui_kit::base::animation::cubic_bezier(0.25, 0.1, 0.25, 1.0)
}

/// A oneshot catalogue animation for `with_animation` (delay-free specs).
pub(super) fn animation(duration: Duration) -> gpui_kit::Animation {
    gpui_kit::Animation::new(duration).with_easing(ease_menu())
}

// ---------------------------------------------------------------------------
// Hover color fades
// ---------------------------------------------------------------------------
//
// `.hover()` styles snap the frame the pointer enters; Zeron's app fades every
// interactive wash over `transition-colors`. This is the manual-drive tween:
// per-key progress advanced from wall time, blended into the color at render
// time. The render tail requests frames while any fade is mid-flight.

#[derive(Debug, Clone, Copy)]
struct FadeEntry {
    origin: f32,
    target: f32,
    started: Instant,
    /// Frame counter at the last read; unread entries are pruned (an element
    /// that unmounts mid-hover never sends its leave event).
    seen: u64,
}

impl FadeEntry {
    fn value(&self, now: Instant, duration: Duration) -> f32 {
        let elapsed = now.saturating_duration_since(self.started);
        if duration.is_zero() || elapsed >= duration {
            return self.target;
        }
        let raw = elapsed.as_secs_f32() / duration.as_secs_f32();
        lerp(self.origin, self.target, ease_tailwind()(raw))
    }

    fn settled(&self, now: Instant, duration: Duration) -> bool {
        self.origin == self.target || now.saturating_duration_since(self.started) >= duration
    }
}

/// Per-key hover progress store (pure core; unit-testable with explicit time).
#[derive(Default)]
pub(super) struct HoverFades {
    entries: HashMap<String, FadeEntry>,
    frame: u64,
}

impl HoverFades {
    fn duration() -> Duration {
        HOVER_FADE
    }

    /// Pointer entered/left the element behind `key`. Re-anchors at the current
    /// value so direction flips mid-flight stay continuous; reduced motion
    /// snaps straight to the endpoint.
    pub(super) fn set_at(&mut self, key: &str, hovered: bool, reduced: bool, now: Instant) {
        let target = if hovered { 1.0 } else { 0.0 };
        let duration = Self::duration();
        let current = self
            .entries
            .get(key)
            .map(|e| e.value(now, duration))
            .unwrap_or(0.0);
        if target == 0.0 && !self.entries.contains_key(key) {
            return; // never-hovered element reporting a leave — nothing to do
        }
        let origin = if reduced { target } else { current };
        let seen = self.frame;
        self.entries.insert(
            key.to_string(),
            FadeEntry {
                origin,
                target,
                started: now,
                seen,
            },
        );
    }

    /// Hover progress (0..1) for `key` at `now`; stamps liveness.
    pub(super) fn value_at(&mut self, key: &str, now: Instant) -> f32 {
        let frame = self.frame;
        match self.entries.get_mut(key) {
            Some(entry) => {
                entry.seen = frame;
                entry.value(now, Self::duration())
            }
            None => 0.0,
        }
    }

    /// Once-per-frame bookkeeping: prune settled-at-rest and unread entries,
    /// and report whether frames must keep coming.
    pub(super) fn tick_at(&mut self, now: Instant) -> bool {
        self.frame += 1;
        let frame = self.frame;
        let duration = Self::duration();
        let mut active = false;
        self.entries.retain(|_, entry| {
            if entry.seen + 1 < frame {
                return false;
            }
            let settled = entry.settled(now, duration);
            if !settled {
                active = true;
            }
            !(settled && entry.target == 0.0)
        });
        active
    }
}

thread_local! {
    static HOVER_FADES: RefCell<HoverFades> = RefCell::new(HoverFades::default());
}

/// Record a hover flip for `key` (reduced motion snaps).
pub(super) fn set_hover(key: &str, hovered: bool, reduced: bool) {
    HOVER_FADES.with(|fades| {
        fades
            .borrow_mut()
            .set_at(key, hovered, reduced, Instant::now())
    });
}

/// Hover progress (0..1) for `key` this frame.
fn hover_t(key: &str) -> f32 {
    HOVER_FADES.with(|fades| fades.borrow_mut().value_at(key, Instant::now()))
}

/// An `.on_hover` listener driving the fade for `key`; pair with
/// [`hover_blend`] reads of the same key.
pub(super) fn hover_listener(
    key: impl Into<String>,
) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
    let key = key.into();
    move |hovered, window, cx| {
        set_hover(&key, *hovered, cx.reduce_motion());
        window.refresh();
    }
}

/// Frame-drive hook: call ONCE per window frame (the shell render tail); true
/// while any hover fade is mid-flight.
pub(super) fn hover_fades_active() -> bool {
    HOVER_FADES.with(|fades| fades.borrow_mut().tick_at(Instant::now()))
}

/// Hover progress (0..1) for `key` during render, for elements that only
/// exist while hovered (the sidebar's group actions). Unknown keys are 0.
pub(super) fn hover_progress(key: &str) -> f32 {
    hover_t(key)
}

/// Blend two colors like the browser transitions them: component interpolation
/// in sRGB with premultiplied alpha, so a transparent wash brightens without
/// passing through grey.
pub(super) fn mix(from: Hsla, to: Hsla, t: f32) -> Hsla {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return from;
    }
    if t >= 1.0 {
        return to;
    }
    let (f, g) = (Rgba::from(from), Rgba::from(to));
    let a = lerp(f.a, g.a, t);
    if a <= f32::EPSILON {
        return Hsla::from(Rgba { a: 0.0, ..g });
    }
    Hsla::from(Rgba {
        r: lerp(f.r * f.a, g.r * g.a, t) / a,
        g: lerp(f.g * f.a, g.g * g.a, t) / a,
        b: lerp(f.b * f.a, g.b * g.a, t) / a,
        a,
    })
}

/// The standard hover blend: rest → hover color at `key`'s current progress.
pub(super) fn hover_blend(key: &str, rest: Hsla, hover: Hsla) -> Hsla {
    mix(rest, hover, hover_t(key))
}

// ---------------------------------------------------------------------------
// Scalar transitions (dialog fade)
// ---------------------------------------------------------------------------

/// Scalar transition curves. `SoftBack` is the gentle ease-out-back used only
/// for the palette ↔ roots width morph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Curve {
    Menu,
    SoftBack,
}

impl Curve {
    fn eval(self, t: f32) -> f32 {
        match self {
            Curve::Menu => ease_menu()(t),
            Curve::SoftBack => {
                let c1 = 1.1f32;
                let c3 = c1 + 1.0;
                let u = t - 1.0;
                1.0 + c3 * u * u * u + c1 * u * u
            }
        }
    }
}

/// A scalar that eases toward its latest target from wherever it currently is.
/// Retargeting mid-flight re-anchors at the displayed value, so rapid
/// open/close/reopen or palette↔roots sequences degrade smoothly instead of
/// replaying an entrance.
#[derive(Debug, Clone, Copy)]
pub(super) struct Transition {
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
    curve: Curve,
}

impl Transition {
    pub(super) fn settled(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            started: Instant::now(),
            duration: Duration::ZERO,
            curve: Curve::Menu,
        }
    }

    /// Aim at `to` with the menu easing (dialog fades).
    pub(super) fn retarget(&mut self, to: f32, duration: Duration, reduced: bool, now: Instant) {
        self.retarget_with(Curve::Menu, to, duration, reduced, now);
    }

    /// Aim at `to` over `duration` from the current displayed value with an
    /// explicit curve. Reduced motion snaps.
    pub(super) fn retarget_with(
        &mut self,
        curve: Curve,
        to: f32,
        duration: Duration,
        reduced: bool,
        now: Instant,
    ) {
        let current = self.value_at(now);
        self.from = if reduced { to } else { current };
        self.to = to;
        self.started = now;
        self.duration = if reduced { Duration::ZERO } else { duration };
        self.curve = curve;
    }

    pub(super) fn target(&self) -> f32 {
        self.to
    }

    pub(super) fn value_at(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return self.to;
        }
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed >= self.duration {
            return self.to;
        }
        let raw = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        lerp(self.from, self.to, self.curve.eval(raw))
    }

    pub(super) fn is_animating(&self, now: Instant) -> bool {
        !self.duration.is_zero() && now.saturating_duration_since(self.started) < self.duration
    }
}

pub(super) fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(t0: Instant, m: u64) -> Instant {
        t0 + Duration::from_millis(m)
    }

    #[test]
    fn hover_fade_ramps_and_reverses_continuously() {
        let mut fades = HoverFades::default();
        let t0 = Instant::now();

        fades.set_at("pill", true, false, t0);
        assert_eq!(fades.value_at("pill", t0), 0.0);
        let mid = fades.value_at("pill", ms(t0, 75));
        assert!(mid > 0.0 && mid < 1.0, "mid-flight enter: {mid}");
        assert_eq!(fades.value_at("pill", ms(t0, 150)), 1.0);

        // Leave mid-flight re-anchors at the current value — no jump.
        fades.set_at("pill", true, false, t0);
        let at_flip = fades.value_at("pill", ms(t0, 75));
        fades.set_at("pill", false, false, ms(t0, 75));
        let after_flip = fades.value_at("pill", ms(t0, 75));
        assert!(
            (after_flip - at_flip).abs() < 1e-4,
            "continuity: {at_flip} vs {after_flip}"
        );
        assert!(fades.value_at("pill", ms(t0, 140)) < after_flip);
        assert_eq!(fades.value_at("pill", ms(t0, 225)), 0.0);
    }

    #[test]
    fn hover_fade_reduced_motion_snaps() {
        let mut fades = HoverFades::default();
        let t0 = Instant::now();
        fades.set_at("row", true, true, t0);
        assert_eq!(fades.value_at("row", t0), 1.0);
        fades.set_at("row", false, true, t0);
        assert_eq!(fades.value_at("row", t0), 0.0);
    }

    #[test]
    fn hover_tick_reports_flight_and_prunes() {
        let mut fades = HoverFades::default();
        let t0 = Instant::now();

        fades.set_at("a", true, false, t0);
        assert!(fades.tick_at(ms(t0, 50)));
        fades.value_at("a", ms(t0, 50));
        assert!(fades.tick_at(ms(t0, 100)));
        fades.value_at("a", ms(t0, 100));
        assert!(!fades.tick_at(ms(t0, 200)));
        fades.value_at("a", ms(t0, 200));

        fades.set_at("a", false, false, ms(t0, 200));
        assert!(fades.tick_at(ms(t0, 260)));
        fades.value_at("a", ms(t0, 260));
        assert!(!fades.tick_at(ms(t0, 500)), "settled at rest");
        assert!(fades.entries.is_empty(), "rest entries are pruned");
    }

    #[test]
    fn transition_retargets_from_displayed_value() {
        let t0 = Instant::now();
        let mut t = Transition::settled(0.0);
        t.retarget(1.0, MENU_IN, false, t0);
        let mid = t.value_at(ms(t0, 70));
        assert!(mid > 0.0 && mid < 1.0, "mid open: {mid}");

        // Close mid-open: continue from the displayed value, never jump up.
        t.retarget(0.0, MENU_OUT, false, ms(t0, 70));
        let at_flip = t.value_at(ms(t0, 70));
        assert!((at_flip - mid).abs() < 1e-4, "continuity: {mid} vs {at_flip}");
        assert!(t.value_at(ms(t0, 120)) < at_flip);

        // Reopen mid-close: rises from the faded value, no entrance replay.
        let before_reopen = t.value_at(ms(t0, 120));
        t.retarget(1.0, MENU_IN, false, ms(t0, 120));
        let reopened = t.value_at(ms(t0, 130));
        assert!(reopened >= before_reopen, "reopen continues upward");
        assert_eq!(t.value_at(ms(t0, 400)), 1.0);
    }

    #[test]
    fn transition_reduced_motion_snaps() {
        let t0 = Instant::now();
        let mut t = Transition::settled(0.0);
        t.retarget(1.0, MENU_IN, true, t0);
        assert_eq!(t.value_at(t0), 1.0);
        assert!(!t.is_animating(t0));
    }

    #[test]
    fn soft_back_morph_overshoots_gently_then_settles() {
        let t0 = Instant::now();
        let mut t = Transition::settled(520.0);
        t.retarget_with(
            Curve::SoftBack,
            560.0,
            Duration::from_millis(170),
            false,
            t0,
        );
        let mut peak = 0.0f32;
        for i in 0..=170u64 {
            peak = peak.max(t.value_at(t0 + Duration::from_millis(i)));
        }
        assert!(peak > 560.0 && peak < 566.0, "gentle overshoot: {peak}");
        assert_eq!(t.value_at(t0 + Duration::from_millis(300)), 560.0);
    }

    #[test]
    fn mix_endpoints_and_transparent_blend() {
        let rest = gpui_kit::hsla(0.0, 0.0, 0.30, 1.0);
        let hover = gpui_kit::hsla(0.0, 0.0, 0.36, 1.0);
        assert_eq!(mix(rest, hover, 0.0), rest);
        assert_eq!(mix(rest, hover, 1.0), hover);
        assert_eq!(mix(rest, hover, -1.0), rest);
        assert_eq!(mix(rest, hover, 2.0), hover);

        let half = mix(
            gpui_kit::hsla(0.0, 0.0, 1.0, 0.0),
            gpui_kit::hsla(0.0, 0.0, 1.0, 0.1),
            0.5,
        );
        assert!((half.a - 0.05).abs() < 1e-4, "alpha midpoint {}", half.a);
    }
}
