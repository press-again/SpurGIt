//! The branchling: the brand character. One component
//! draws it everywhere it appears — the title bar, the empty state and About.
//!
//! Above 31 physical px the vector body and eyes ride as two stacked SVG
//! masks: the face opening is an evenodd cutout, so the ground shows through.
//! At 24–31 px the micro drawing (a larger opening) keeps the vector eyes
//! legible. At 24 px and below the vector eyes blur into the opening, so the
//! hand-hinted pixel maps are painted instead, one device pixel per cell. The
//! map is chosen at paint time because the scale factor changes when the
//! window moves between monitors.
//!
use super::*;

use gpui_kit::{BorderStyle, Hsla, canvas, quad, svg, transparent_black};

/// Vector body (outline + face opening), 320×320 canvas.
const BODY: &[u8] = include_bytes!("../../assets/brand/body.svg");
/// The 24–31 px variant: a slightly larger opening so the eyes survive.
const MICRO_BODY: &[u8] = include_bytes!("../../assets/brand/micro-body.svg");
/// The micro eyes, drawn to the micro opening.
const MICRO_EYES: &[u8] = include_bytes!("../../assets/brand/micro-eyes.svg");

/// The hand-hinted maps, one row per line (`#` 1.0, `+` 0.6, `-` 0.3), with
/// the squint and glance variants the kit derives from them. 16 px Look
/// falls back to the resting map: there is no room to shift an eye without
/// merging it with the rim.
const PIXEL_MAPS: [(usize, &str, &str, &str); 3] = [
    (
        16,
        include_str!("../../assets/brand/pixel-16.txt"),
        include_str!("../../assets/brand/pixel-16-focus.txt"),
        include_str!("../../assets/brand/pixel-16-look.txt"),
    ),
    (
        20,
        include_str!("../../assets/brand/pixel-20.txt"),
        include_str!("../../assets/brand/pixel-20-focus.txt"),
        include_str!("../../assets/brand/pixel-20-look.txt"),
    ),
    (
        24,
        include_str!("../../assets/brand/pixel-24.txt"),
        include_str!("../../assets/brand/pixel-24-focus.txt"),
        include_str!("../../assets/brand/pixel-24-look.txt"),
    ),
];

/// The outlined wordmark ("spur") and its canvas. The kit trims the viewBox to
/// the word, so the title bar derives the width from the height instead of
/// stretching it; the test keeps these two numbers and the file in step.
const WORDMARK: &[u8] = include_bytes!("../../assets/brand/wordmark.svg");
const WORDMARK_W: f32 = 384.625;
const WORDMARK_H: f32 = 140.0;

/// At or below this many physical pixels the pixel maps are painted.
const SMALL_MAX_PHYSICAL: f32 = 24.0;
/// At or below this many physical pixels the micro drawing is used (the kit's
/// sizing table: 24–31 px, vector).
const MICRO_MAX_PHYSICAL: f32 = 31.0;

/// Which drawing a physical size takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Band {
    /// Hand-hinted pixel map, resting face only.
    Pixel,
    /// Micro vector drawing, resting face only.
    Micro,
    /// Full vector drawing with every face.
    Vector,
}

fn band_for(physical: f32) -> Band {
    if physical <= SMALL_MAX_PHYSICAL {
        Band::Pixel
    } else if physical <= MICRO_MAX_PHYSICAL {
        Band::Micro
    } else {
        Band::Vector
    }
}

/// The character's eyes. Only the eyes ever change;
/// the outline, size and tint stay fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Face {
    Rest,
    Look,
    Focus,
    Happy,
    Wink,
}

impl Face {
    /// The eyes layer for this face; a missing file is a compile error.
    fn eyes(self) -> &'static [u8] {
        match self {
            Face::Rest => include_bytes!("../../assets/brand/eyes-rest.svg"),
            Face::Look => include_bytes!("../../assets/brand/eyes-look.svg"),
            Face::Focus => include_bytes!("../../assets/brand/eyes-focus.svg"),
            Face::Happy => include_bytes!("../../assets/brand/eyes-happy.svg"),
            Face::Wink => include_bytes!("../../assets/brand/eyes-wink.svg"),
        }
    }

    /// Fade key: changing the face restarts the eyes animation.
    fn key(self) -> usize {
        match self {
            Face::Rest => 0,
            Face::Look => 1,
            Face::Focus => 2,
            Face::Happy => 3,
            Face::Wink => 4,
        }
    }
}

/// The branchling at `size` logical px, tinted, showing `face`.
pub(super) fn branchling(
    size: f32,
    face: Face,
    tint: Hsla,
    window: &Window,
) -> gpui_kit::AnyElement {
    match band_for(size * window.scale_factor()) {
        Band::Pixel => pixels(size, face, tint),
        // The micro band carries the resting face.
        Band::Micro => vector(size, MICRO_BODY, MICRO_EYES, Face::Rest.key(), tint),
        Band::Vector => vector(size, BODY, face.eyes(), face.key(), tint),
    }
}

/// The face for one frame: a network operation the
/// user started wins, then a timed hold, then rest. Failures and cancels set
/// no hold, so the character stays at rest and the alert carries the news.
pub(super) fn face_for(
    network_running: bool,
    hold: Option<(Face, std::time::Instant)>,
    now: std::time::Instant,
) -> Face {
    if network_running {
        return Face::Focus;
    }
    match hold {
        Some((face, until)) if now < until => face,
        _ => Face::Rest,
    }
}

/// The outlined wordmark at `height` logical px, tinted for the title bar.
pub(super) fn wordmark(height: f32, tint: Hsla) -> gpui_kit::AnyElement {
    svg()
        .data(WORDMARK)
        .h(px(height))
        .w(px(height * WORDMARK_W / WORDMARK_H))
        .text_color(tint)
        .into_any_element()
}

/// Over 31 physical px: the vector body plus the eyes, both tinted masks. The
/// body never changes; the eyes fade in when the face changes (150 ms, and
/// gpui renders the end state directly under reduced motion). The outgoing
/// eyes are not kept — a two-layer crossfade needs the previous face, which
/// is caller state; with one change at a time the incoming fade is what
/// the eye sees.
fn vector(
    size: f32,
    body: &'static [u8],
    eyes: &'static [u8],
    eye_key: usize,
    tint: Hsla,
) -> gpui_kit::AnyElement {
    div()
        .relative()
        .flex_none()
        .size(px(size))
        .child(svg().data(body).absolute().inset_0().text_color(tint))
        .child(
            svg()
                .data(eyes)
                .absolute()
                .inset_0()
                .text_color(tint)
                .with_animation(
                    ("brand-eyes", eye_key),
                    motion::animation(Duration::from_millis(150)),
                    |eyes, t| eyes.opacity(t),
                ),
        )
        .into_any_element()
}

/// 24 physical px or less: the hand-hinted map, one device pixel per cell.
/// Focus and Look have their own maps; Happy's arcs and the Wink do not
/// survive this size, so they rest.
fn pixels(size: f32, face: Face, tint: Hsla) -> gpui_kit::AnyElement {
    canvas(
        move |_bounds, _window, _cx| (),
        move |bounds, _, window, _cx| paint_pixels(bounds, tint, face, window),
    )
    .flex_none()
    .size(px(size))
    .into_any_element()
}

/// Map for a physical size: 16 up to 18 px, 20 up to 22, else 24.
fn map_for(physical: f32) -> usize {
    if physical <= 18.0 {
        16
    } else if physical <= 22.0 {
        20
    } else {
        24
    }
}

/// The rows of one map, `\n`-separated.
fn map_rows(n: usize, face: Face) -> &'static str {
    let (_, rest, focus, look) = PIXEL_MAPS
        .iter()
        .find(|(size, ..)| *size == n)
        .expect("every selectable size has a map");
    match face {
        Face::Focus => focus,
        Face::Look => look,
        Face::Rest | Face::Happy | Face::Wink => rest,
    }
}

/// The alpha of one map cell; `None` is a transparent cell.
fn level(ch: char) -> Option<f32> {
    match ch {
        '#' => Some(1.0),
        '+' => Some(0.6),
        '-' => Some(0.3),
        _ => None,
    }
}

fn paint_pixels(bounds: Bounds<gpui_kit::Pixels>, tint: Hsla, face: Face, window: &mut Window) {
    let scale = window.scale_factor();
    let n = map_for(f32::from(bounds.size.width) * scale);
    // One cell is one device pixel; snapping the origin first keeps the quads
    // on the device grid instead of blurring across it.
    let cell = 1.0 / scale;
    let art = n as f32 * cell;
    let x0 = f32::from(bounds.origin.x) + (f32::from(bounds.size.width) - art) / 2.0;
    let y0 = f32::from(bounds.origin.y) + (f32::from(bounds.size.height) - art) / 2.0;
    let x0 = (x0 * scale).round() / scale;
    let y0 = (y0 * scale).round() / scale;
    for (row, line) in map_rows(n, face).lines().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            let Some(alpha) = level(ch) else {
                continue;
            };
            window.paint_quad(quad(
                Bounds {
                    origin: point(px(x0 + col as f32 * cell), px(y0 + row as f32 * cell)),
                    size: size(px(cell), px(cell)),
                },
                px(0.),
                tint.opacity(alpha),
                px(0.),
                transparent_black(),
                BorderStyle::default(),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_face_ships_its_eyes_on_the_shared_canvas() {
        for face in [Face::Rest, Face::Look, Face::Focus, Face::Happy, Face::Wink] {
            let text = std::str::from_utf8(face.eyes()).expect("eyes are utf8");
            assert!(text.starts_with("<svg"), "{face:?}: not an SVG");
            assert!(
                text.contains("viewBox=\"0 0 320 320\""),
                "{face:?}: must share the body's canvas to stack"
            );
        }
        for body in [BODY, MICRO_BODY] {
            let text = std::str::from_utf8(body).expect("body is utf8");
            assert!(text.contains("viewBox=\"0 0 320 320\""));
            assert!(
                text.contains("evenodd"),
                "the face opening has to be a cutout"
            );
        }
    }

    #[test]
    fn pixel_maps_are_square_and_use_the_known_levels() {
        for (n, rest, focus, look) in PIXEL_MAPS {
            for (face, map) in [("rest", rest), ("focus", focus), ("look", look)] {
                let rows: Vec<&str> = map.lines().collect();
                assert_eq!(rows.len(), n, "{n}px {face} map: {} rows", rows.len());
                for (y, row) in rows.iter().enumerate() {
                    assert_eq!(
                        row.chars().count(),
                        n,
                        "{n}px {face} row {y} is {} wide",
                        row.len()
                    );
                    assert!(
                        row.chars().all(|c| level(c).is_some() || c == '.'),
                        "{n}px {face} row {y} leaves the # + - . alphabet: {row}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_expression_maps_differ_from_the_resting_face() {
        for (n, rest, focus, look) in PIXEL_MAPS {
            assert_ne!(focus, rest, "{n}px Focus must squint");
            if n == 16 {
                // The glance has no room at 16 px (the eye would merge with
                // the rim), so Look rests there.
                assert_eq!(look, rest, "16px Look rests");
            } else {
                assert_ne!(look, rest, "{n}px Look must glance");
            }
        }
    }

    #[test]
    fn focus_beats_a_hold_and_holds_expire() {
        let now = Instant::now();
        let later = now + Duration::from_millis(2500);
        assert_eq!(face_for(false, None, now), Face::Rest);
        assert_eq!(face_for(false, Some((Face::Happy, later)), now), Face::Happy);
        assert_eq!(
            face_for(false, Some((Face::Happy, later)), later),
            Face::Rest,
            "an expired hold rests"
        );
        assert_eq!(
            face_for(true, Some((Face::Happy, later)), now),
            Face::Focus,
            "a running operation wins"
        );
    }

    #[test]
    fn the_wordmark_keeps_the_canvas_the_title_bar_sizes_for() {
        let text = std::str::from_utf8(WORDMARK).expect("wordmark is utf8");
        assert!(
            text.contains(&format!("viewBox=\"0 0 {WORDMARK_W} {WORDMARK_H}\"")),
            "the wordmark canvas moved; update WORDMARK_W / WORDMARK_H"
        );
        assert!(
            text.contains("currentColor"),
            "the app tints the wordmark, so it must be one colour"
        );
    }

    #[test]
    fn the_size_bands_meet_at_the_kit_boundaries() {
        assert_eq!(band_for(24.0), Band::Pixel);
        assert_eq!(band_for(25.0), Band::Micro);
        assert_eq!(band_for(31.0), Band::Micro);
        assert_eq!(band_for(32.0), Band::Vector);
    }

    #[test]
    fn the_pixel_map_follows_the_scale_factor() {
        for (scale, expected) in [(1.0, 16_usize), (1.25, 20), (1.5, 24)] {
            assert_eq!(map_for(16.0 * scale), expected, "at {scale}x");
        }
        // The rare sizes clamp to the nearest map.
        assert_eq!(map_for(10.0), 16);
        assert_eq!(map_for(40.0), 24);
    }
}
