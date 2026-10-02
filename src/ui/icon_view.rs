//! GPUI side of the animated dot icons: one fixed-size canvas paints the
//! sampled dots, and the view owns the persistent player that schedules its
//! own redraws while active.
//!
//! Painting is paint-local: a dot changes size or opacity inside its slot and
//! never affects layout, so an animated icon can never move adjacent text.

use std::time::Instant;

use gpui_kit::component::ActiveTheme;
use gpui_kit::{
    App, Bounds, BorderStyle, Context, Hsla, IntoElement, Render, Styled as _, Window, canvas,
    point, px, quad, size, transparent_black,
};

use crate::icon::player::Player;
use crate::icon::sample::{SampledDot, sample_state};
use crate::icon::{ColorRole, DotShape, IconDef};

use super::widgets::{text_faint, text_muted, text_primary, violet};

/// When a bound app icon plays its animation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayMode {
    #[default]
    Off,
    Always,
    Hover,
    Click,
    Load,
}

impl PlayMode {
    pub const ALL: [PlayMode; 5] = [
        PlayMode::Off,
        PlayMode::Always,
        PlayMode::Hover,
        PlayMode::Click,
        PlayMode::Load,
    ];

    pub fn key(self) -> &'static str {
        match self {
            PlayMode::Off => "off",
            PlayMode::Always => "always",
            PlayMode::Hover => "hover",
            PlayMode::Click => "click",
            PlayMode::Load => "load",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.key() == key)
    }
}

pub struct IconView {
    def: IconDef,
    player: Player,
    size: f32,
    /// Show the named state with effects suppressed.
    static_preview: bool,
}

impl IconView {
    /// A static view on `state` (no animation scheduling).
    pub fn new(def: IconDef, state: &str) -> Self {
        Self {
            player: Player::new(state),
            def,
            size: 16.0,
            static_preview: false,
        }
    }

    pub fn set_size(&mut self, size: f32) {
        self.size = size.max(4.0);
    }

    /// Apply a playback mode: off/hover/click rest at the icon's *static*
    /// appearance (the named state with no effects), always loops, and
    /// click/load play once from the start.
    pub fn set_mode(&mut self, mode: PlayMode, now: Instant) {
        self.player
            .set_once(matches!(mode, PlayMode::Click | PlayMode::Load));
        match mode {
            PlayMode::Off | PlayMode::Hover | PlayMode::Click => {
                self.static_preview = true;
                self.player.pause(now);
                self.player.seek(0.0, now);
            }
            PlayMode::Always => {
                self.static_preview = false;
                self.player.play(now);
            }
            PlayMode::Load => {
                self.static_preview = false;
                self.player.restart(now);
                self.player.play(now);
            }
        }
    }

    /// Hover entered / click released: leave the static appearance and play.
    pub fn begin_play(&mut self, now: Instant) {
        self.static_preview = false;
        self.player.play(now);
    }

    /// Hover left: back to the static appearance.
    pub fn end_play(&mut self, now: Instant) {
        self.static_preview = true;
        self.player.pause(now);
    }

    pub fn restart_once(&mut self, now: Instant) {
        self.static_preview = false;
        self.player.restart(now);
        self.player.set_once(true);
        self.player.play(now);
    }

    /// Sample now without painting. Honors the static flag, so a static view
    /// always reports the actual static state.
    #[cfg(test)]
    pub fn sampled(&self, now: Instant, reduced: bool) -> Vec<SampledDot> {
        if self.static_preview || reduced {
            sample_state(&self.def, self.player.state())
        } else {
            self.player.sample(&self.def, now)
        }
    }
}

impl Render for IconView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let reduced = cx.reduce_motion();
        // A one-shot playback falls back to the static appearance once it
        // finishes, so "static" always means the genuine static state.
        let once_done = self.player.is_once() && self.player.finished(&self.def, now);
        let dots = if self.static_preview || reduced || once_done {
            sample_state(&self.def, self.player.state())
        } else {
            self.player.sample(&self.def, now)
        };
        let playing = !self.static_preview
            && !once_done
            && self.player.is_playing()
            && !reduced
            && !self.player.finished(&self.def, now);
        let element = canvas(
            move |_bounds, _window, _cx| dots,
            move |bounds, dots, window, cx| {
                paint_dots(bounds, &dots, window, cx);
            },
        )
        .size(px(self.size));
        if playing {
            // The clock is the player's; this only asks for the next frame.
            window.request_animation_frame();
        }
        element
    }
}

fn paint_dots(
    bounds: Bounds<gpui_kit::Pixels>,
    dots: &[SampledDot],
    window: &mut Window,
    cx: &mut App,
) {
    let x0: f32 = bounds.origin.x.into();
    let y0: f32 = bounds.origin.y.into();
    let w: f32 = bounds.size.width.into();
    let h: f32 = bounds.size.height.into();
    // Square canvas centred in the slot; normalized coordinates cover it.
    let side = w.min(h);
    let ox = x0 + (w - side) / 2.0;
    let oy = y0 + (h - side) / 2.0;

    for dot in dots {
        if dot.opacity <= 0.001 {
            continue;
        }
        let half = (dot.r * side).max(0.35);
        let corner = match dot.shape {
            DotShape::Circle => half,
            // 25% of the side keeps a "rounded square" clearly square.
            DotShape::RoundedSquare => half * 0.5,
        };
        let cx_px = ox + dot.x * side;
        let cy_px = oy + dot.y * side;
        let color = role_color(dot.role, cx).opacity(dot.opacity);
        window.paint_quad(quad(
            Bounds {
                origin: point(px(cx_px - half), px(cy_px - half)),
                size: size(px(half * 2.0), px(half * 2.0)),
            },
            px(corner),
            color,
            px(0.),
            transparent_black(),
            BorderStyle::default(),
        ));
    }
}

/// Resolve a theme-relative role to the live theme's color.
fn role_color(role: ColorRole, cx: &App) -> Hsla {
    match role {
        ColorRole::Text => text_primary(cx),
        ColorRole::Muted => text_muted(cx),
        ColorRole::Faint => text_faint(cx),
        ColorRole::Accent => violet(cx),
        ColorRole::Success => cx.theme().success,
        ColorRole::Warning => cx.theme().warning,
        ColorRole::Danger => cx.theme().danger,
    }
}

#[cfg(test)]
mod mode_tests {
    use super::*;
    use crate::icon::{Animation, Easing, Effect};
    use std::time::Duration;

    fn dimmed_icon() -> IconDef {
        let mut def = IconDef::from_json(include_str!("../../icons/dot-tree.json"))
            .expect("shipped icon is valid");
        def.animations = vec![Animation {
            name: "pulse".into(),
            duration_ms: 1000,
            looping: true,
            effects: vec![Effect::Pulse {
                intensity: 1.0,
                phase_offset: 0.0,
                stagger: 0.0,
                easing: Easing::Linear,
            }],
        }];
        def
    }

    #[test]
    fn static_mode_shows_the_actual_static_state() {
        let def = dimmed_icon();
        let t0 = Instant::now();
        let mut view = IconView::new(def.clone(), "rest");
        view.set_mode(PlayMode::Off, t0);
        let sampled = view.sampled(t0 + Duration::from_millis(123), false);
        assert_eq!(
            sampled,
            sample_state(&def, "rest"),
            "static must not freeze an animated frame"
        );
    }

    #[test]
    fn always_mode_samples_the_effect() {
        let def = dimmed_icon();
        let t0 = Instant::now();
        let mut view = IconView::new(def.clone(), "rest");
        view.set_mode(PlayMode::Always, t0);
        let sampled = view.sampled(t0, false);
        let expected = sample_state(&def, "rest");
        assert!(
            sampled
                .iter()
                .zip(expected.iter())
                .all(|(animated, stat)| animated.opacity < stat.opacity),
            "always mode must sample the effect"
        );
    }

    #[test]
    fn hover_and_click_rest_at_the_static_state() {
        let def = dimmed_icon();
        let t0 = Instant::now();
        for mode in [PlayMode::Hover, PlayMode::Click] {
            let mut view = IconView::new(def.clone(), "rest");
            view.set_mode(mode, t0);
            assert_eq!(
                view.sampled(t0, false),
                sample_state(&def, "rest"),
                "{mode:?} idle must be the static state"
            );
        }
    }
}
