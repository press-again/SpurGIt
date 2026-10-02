//! The player: one persistent wall clock per animated icon instance, shared
//! by all of its dots.
//!
//! The playhead is always derived from (`base`, `anchor`) — never accumulated
//! frame to frame — so scrubbing, pausing, and playback sample exactly the
//! same values.

use std::time::{Duration, Instant};

use super::IconDef;
use super::sample::{SampledDot, apply_effects_with, sample_state};

pub struct Player {
    anchor: Instant,
    base: Duration,
    playing: bool,
    /// One-shot playback: loops are suppressed and the final frame is held.
    once: bool,
    state: String,
}

impl Player {
    /// A paused player on the initial state.
    pub fn new(state: impl Into<String>) -> Self {
        Self {
            anchor: Instant::now(),
            base: Duration::ZERO,
            playing: false,
            once: false,
            state: state.into(),
        }
    }

    pub fn state(&self) -> &str {
        &self.state
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// One-shot playback for "on click"/"on load" modes.
    pub fn set_once(&mut self, once: bool) {
        self.once = once;
    }

    pub fn is_once(&self) -> bool {
        self.once
    }

    /// Current playhead. Paused players hold their position.
    pub fn playhead(&self, now: Instant) -> Duration {
        if self.playing {
            self.base + now.saturating_duration_since(self.anchor)
        } else {
            self.base
        }
    }

    pub fn millis(&self, now: Instant) -> f32 {
        self.playhead(now).as_secs_f32() * 1000.0
    }

    pub fn play(&mut self, now: Instant) {
        if !self.playing {
            self.anchor = now;
            self.playing = true;
        }
    }

    pub fn pause(&mut self, now: Instant) {
        if self.playing {
            self.base = self.playhead(now);
            self.playing = false;
        }
    }

    /// Restart the timeline from zero.
    pub fn restart(&mut self, now: Instant) {
        self.base = Duration::ZERO;
        self.anchor = now;
    }

    /// Scrub to an absolute position in milliseconds.
    pub fn seek(&mut self, ms: f32, now: Instant) {
        self.base = Duration::from_secs_f32(ms.max(0.0) / 1000.0);
        self.anchor = now;
    }

    /// True when playback has reached its end; the view uses this to stop
    /// scheduling frames. A one-shot player finishes even when the preset
    /// loops.
    pub fn finished(&self, def: &IconDef, now: Instant) -> bool {
        match def.animations.first() {
            Some(anim) if self.once => self.millis(now) >= anim.duration_ms as f32,
            Some(anim) if !anim.looping => self.millis(now) >= anim.duration_ms as f32,
            _ => false,
        }
    }

    /// Sample the icon at `now`.
    pub fn sample(&self, def: &IconDef, now: Instant) -> Vec<SampledDot> {
        match def.animations.first() {
            Some(anim) => {
                let looping = anim.looping && !self.once;
                let time = if self.once {
                    self.millis(now).min(anim.duration_ms as f32)
                } else {
                    self.millis(now)
                };
                apply_effects_with(def, anim, time, &def.resolved_state(&self.state), looping)
            }
            None => sample_state(def, &self.state),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icon::{Animation, Dot, Effect, IconDef};

    fn icon() -> IconDef {
        let mut def = IconDef {
            name: "Player".into(),
            dots: vec![Dot::new("a", 0.25, 0.5, 0.1), Dot::new("b", 0.75, 0.5, 0.1)],
            animations: vec![Animation::new("pulse", 1000)],
            ..IconDef::default()
        };
        def.animations[0].effects.push(Effect::default_pulse());
        def
    }

    #[test]
    fn scrubbing_and_playback_sample_the_same_state() {
        let def = icon();
        let t0 = Instant::now();
        let mut player = Player::new("rest");
        player.play(t0);
        let played = player.sample(&def, t0 + Duration::from_millis(333));

        player.pause(t0 + Duration::from_millis(333));
        let paused = player.sample(&def, t0 + Duration::from_millis(900));
        assert_eq!(played, paused, "paused frame matches the played frame");

        // Scrolling the playhead back reproduces the earlier frame exactly.
        player.seek(0.0, t0 + Duration::from_millis(900));
        let scrubbed = player.sample(&def, t0 + Duration::from_millis(950));
        let start = {
            let mut fresh = Player::new("rest");
            fresh.play(t0);
            fresh.sample(&def, t0)
        };
        assert_eq!(scrubbed, start, "scrub to 0 equals a fresh start");
    }

    #[test]
    fn one_shot_animation_reports_finished() {
        let mut def = icon();
        def.animations[0].looping = false;
        let t0 = Instant::now();
        let mut player = Player::new("rest");
        player.play(t0);
        assert!(!player.finished(&def, t0 + Duration::from_millis(999)));
        assert!(player.finished(&def, t0 + Duration::from_millis(1000)));
    }

    #[test]
    fn once_mode_holds_the_final_frame_even_for_looping_presets() {
        let def = icon(); // looping pulse
        let t0 = Instant::now();
        let mut player = Player::new("rest");
        player.set_once(true);
        player.play(t0);
        assert!(!player.finished(&def, t0 + Duration::from_millis(999)));
        assert!(player.finished(&def, t0 + Duration::from_millis(1000)));
        let at_end = player.sample(&def, t0 + Duration::from_millis(1000));
        let beyond = player.sample(&def, t0 + Duration::from_millis(5000));
        assert_eq!(at_end, beyond, "the final frame is held");
    }
}
