//! Menu explainer popouts (history context menu): hover an option for
//! a beat and a small card slides out beside it — what the action does, plus
//! a looping lane diagram for the ref-moving actions.
//!
//! Mechanics: menu rows report hover enter/leave, the window keeps one
//! animation frame alive while an explainer is pending or visible (no timers),
//! and placement flips left/right against the viewport. The hovered row marks
//! itself seen on every prepaint; once it stops painting (its menu closed
//! without a leave) the popout retires within a couple of frames, so it stays
//! up exactly as long as the hover and never lingers as an orphan.

use super::*;

use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::base::ElementExt as _;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::{
    Animation, AnimationExt as _, Bounds, IntoElement, SharedString, StatefulInteractiveElement as _,
    WeakEntity,
};

use crate::i18n::t;

/// Hover this long before the popout appears.
pub(super) const EXPLAINER_DELAY: Duration = Duration::from_millis(600);
/// Shell frames without a prepaint of the hovered row before the popout
/// retires: the menu closed without a leave event.
pub(super) const UNSEEN_FRAME_LIMIT: u8 = 2;
/// Card width; the side flips when this plus the gap would leave the window.
pub(super) const EXPLAINER_W: f32 = 264.0;
/// Row edge to menu outer edge: item padding 8 + list padding 4 + border 1
/// (gpui-component popup menu).
const MENU_INSET: f32 = 13.0;
/// Visible separation between the menu and the card.
const EXPLAINER_GAP: f32 = 6.0;
/// Estimated card height for vertical clamping (content varies a little).
const EXPLAINER_H: f32 = 200.0;
/// One diagram loop.
const DIAGRAM_PERIOD: Duration = Duration::from_millis(1600);

/// Which history action the popout explains. One variant per menu item so a
/// leave event can only clear its own hover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExplainerKind {
    CheckoutDetached,
    CreateBranch,
    CreateTag,
    CherryPick,
    Revert,
    Reset,
    CopySha,
    CopyShortSha,
    CopySubject,
    CopyPatch,
}

impl ExplainerKind {
    /// Stable element id: each kind appears once per menu.
    fn row_id(self) -> &'static str {
        match self {
            ExplainerKind::CheckoutDetached => "menu-explainer-checkout",
            ExplainerKind::CreateBranch => "menu-explainer-branch",
            ExplainerKind::CreateTag => "menu-explainer-tag",
            ExplainerKind::CherryPick => "menu-explainer-pick",
            ExplainerKind::Revert => "menu-explainer-revert",
            ExplainerKind::Reset => "menu-explainer-reset",
            ExplainerKind::CopySha => "menu-explainer-sha",
            ExplainerKind::CopyShortSha => "menu-explainer-short",
            ExplainerKind::CopySubject => "menu-explainer-subject",
            ExplainerKind::CopyPatch => "menu-explainer-patch",
        }
    }
    /// Ref-moving actions get the looping lane diagram; copies are text-only.
    fn animated(self) -> bool {
        !matches!(
            self,
            ExplainerKind::CopySha
                | ExplainerKind::CopyShortSha
                | ExplainerKind::CopySubject
                | ExplainerKind::CopyPatch
        )
    }

    fn icon(self) -> IconName {
        match self {
            ExplainerKind::CheckoutDetached => IconName::GitCommitHorizontal,
            ExplainerKind::CreateBranch => IconName::GitBranchPlus,
            ExplainerKind::CreateTag => IconName::Tag,
            ExplainerKind::CherryPick => IconName::Plus,
            ExplainerKind::Revert => IconName::RotateCcw,
            ExplainerKind::Reset => IconName::Rewind,
            ExplainerKind::CopySha
            | ExplainerKind::CopyShortSha
            | ExplainerKind::CopySubject
            | ExplainerKind::CopyPatch => IconName::ClipboardCopy,
        }
    }

    fn body(self, ctx_a: &str) -> String {
        match self {
            ExplainerKind::CheckoutDetached => t().explainer_checkout_detached().to_string(),
            ExplainerKind::CreateBranch => t().explainer_create_branch().to_string(),
            ExplainerKind::CreateTag => t().explainer_create_tag().to_string(),
            ExplainerKind::CherryPick => t().explainer_cherry_pick(ctx_a),
            ExplainerKind::Revert => t().explainer_revert().to_string(),
            ExplainerKind::Reset => t().explainer_reset(ctx_a),
            ExplainerKind::CopySha => t().explainer_copy_sha().to_string(),
            ExplainerKind::CopyShortSha => t().explainer_copy_short().to_string(),
            ExplainerKind::CopySubject => t().explainer_copy_subject().to_string(),
            ExplainerKind::CopyPatch => t().explainer_copy_patch().to_string(),
        }
    }
}

/// One pending or visible popout. The row bounds come from the row's own
/// prepaint hook, so placement never depends on cursor tracking (an open
/// menu eats the moves before they reach the shell).
#[derive(Clone, Debug)]
pub(super) struct ExplainerState {
    pub kind: ExplainerKind,
    pub title: String,
    pub ctx_a: String,
    pub row: Option<Bounds<gpui_kit::Pixels>>,
    pub since: Instant,
    /// Shell frames since the hovered row last prepainted.
    pub unseen_frames: u8,
    pub visible: bool,
    pub generation: u64,
}

/// Card origin for a row: beside the menu (not just the row) on the side
/// with room, top-aligned with the row and clamped into the window. All
/// logical pixels; pure, unit-tested.
pub(super) fn place_popout(
    row_x: f32,
    row_y: f32,
    row_w: f32,
    viewport_w: f32,
    viewport_h: f32,
) -> (f32, f32) {
    let offset = MENU_INSET + EXPLAINER_GAP;
    let left = if row_x + row_w + offset + EXPLAINER_W <= viewport_w {
        row_x + row_w + offset
    } else {
        (row_x - offset - EXPLAINER_W).max(8.0)
    };
    let top = row_y.clamp(8.0, (viewport_h - EXPLAINER_H - 8.0).max(8.0));
    (left, top)
}

/// Position along a waypoint path at loop fraction `t` (wraps; pure,
/// unit-tested).
fn dot_along(t: f32, points: &[(f32, f32)]) -> (f32, f32) {
    if points.is_empty() {
        return (0.0, 0.0);
    }
    if points.len() == 1 {
        return points[0];
    }
    let t = t.rem_euclid(1.0);
    let segs = (points.len() - 1) as f32;
    let f = (t * segs).min(segs - f32::EPSILON);
    let ix = f as usize;
    let local = f - ix as f32;
    let (x0, y0) = points[ix];
    let (x1, y1) = points[ix + 1];
    (x0 + (x1 - x0) * local, y0 + (y1 - y0) * local)
}

/// Fade the travelling dot near the loop ends so the wrap is seamless.
fn fade_ends(t: f32) -> f32 {
    (t * 4.0).min(1.0).min((1.0 - t) * 4.0).max(0.15)
}

/// One menu row with a hover explainer: the same row treatment as
/// [`context_menu_item`](super::widgets::context_menu_item), plus hover
/// enter/leave reporting. History menu only.
pub(super) fn explained_menu_item(
    label: String,
    icon: IconName,
    kind: ExplainerKind,
    title: String,
    ctx_a: String,
    entity: WeakEntity<SpurShell>,
    handler: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> PopupMenuItem {
    PopupMenuItem::element(move |_window, cx| {
        div()
            .id(kind.row_id())
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(Icon::new(icon).size(px(14.)).text_color(text_muted(cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(label.clone()),
            )
            // Every prepaint: record the row bounds for placement and mark
            // the row seen, which keeps its popout alive (no notify needed,
            // the shell is already driving frames).
            .on_prepaint({
                let entity = entity.clone();
                move |bounds, _, cx| {
                    entity
                        .update(cx, |shell, _| shell.see_explainer_row(kind, bounds))
                        .ok();
                }
            })
            .on_hover({
                let entity = entity.clone();
                let title = title.clone();
                let ctx_a = ctx_a.clone();
                move |hovered, window, cx| {
                    if *hovered {
                        entity
                            .update(cx, |shell, cx| {
                                shell.note_explainer_hover(kind, title.clone(), ctx_a.clone(), cx)
                            })
                            .ok();
                        // NB: request_animation_frame is only legal during
                        // layout/paint — from an event, refresh() schedules
                        // the frame that starts the render drive instead.
                        window.refresh();
                    } else {
                        entity
                            .update(cx, |shell, cx| shell.leave_explainer(kind, cx))
                            .ok();
                    }
                }
            })
    })
    .on_click(handler)
}

impl SpurShell {
    pub(super) fn note_explainer_hover(
        &mut self,
        kind: ExplainerKind,
        title: String,
        ctx_a: String,
        cx: &mut Context<Self>,
    ) {
        self.explainer_gen += 1;
        let generation = self.explainer_gen;
        self.explainer = Some(ExplainerState {
            kind,
            title,
            ctx_a,
            row: None,
            since: Instant::now(),
            unseen_frames: 0,
            visible: false,
            generation,
        });
        cx.notify();
    }

    fn see_explainer_row(&mut self, kind: ExplainerKind, row: Bounds<gpui_kit::Pixels>) {
        if let Some(state) = self.explainer.as_mut().filter(|state| state.kind == kind) {
            state.row = Some(row);
            state.unseen_frames = 0;
        }
    }

    pub(super) fn leave_explainer(&mut self, kind: ExplainerKind, cx: &mut Context<Self>) {
        if self
            .explainer
            .as_ref()
            .is_some_and(|state| state.kind == kind)
        {
            self.explainer = None;
            cx.notify();
        }
    }

    pub(super) fn clear_explainer(&mut self, cx: &mut Context<Self>) {
        if self.explainer.take().is_some() {
            cx.notify();
        }
    }

    /// The card for a visible explainer, beside its row with a viewport
    /// flip. `None` when nothing is visible (or the row never prepainted).
    pub(super) fn render_explainer(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::AnyElement> {
        let state = self.explainer.as_ref().filter(|state| state.visible)?;
        let row = state.row?;
        // Row bounds and viewport are both logical `Pixels` already; no
        // scale-factor division (that pulled the card onto the menu on HiDPI).
        let viewport = window.viewport_size();
        let (left, top) = place_popout(
            f32::from(row.origin.x),
            f32::from(row.origin.y),
            f32::from(row.size.width),
            f32::from(viewport.width),
            f32::from(viewport.height),
        );
        let kind = state.kind;
        // Deferred at window level like the menu itself, one priority above
        // it (menus paint at POPUP_PRIORITY) so the card is never underneath.
        Some(
            gpui_kit::deferred(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(EXPLAINER_W))
                .rounded(px(PANEL_RADIUS))
                .border_1()
                .border_color(hairline(0.10))
                .bg(cx.theme().popover)
                .shadow_lg()
                .p(px(12.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            Icon::new(kind.icon())
                                .size(px(14.))
                                .flex_none()
                                .text_color(violet(cx)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(TEXT_SM))
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .text_color(text_primary(cx))
                                .child(state.title.clone()),
                        ),
                )
                .children(kind.animated().then(|| {
                    explainer_diagram(kind, state.generation, cx.reduce_motion(), cx)
                }))
                .child(
                    div()
                        .text_size(px(TEXT_XS))
                        .text_color(text_muted(cx))
                        .child(kind.body(&state.ctx_a)),
                )
                .with_animation(
                    SharedString::from(format!("menu-explainer-{}", state.generation)),
                    motion::animation(motion::MENU_IN),
                    |el, t| el.opacity(t),
                )
                .into_any_element(),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY + 1)
            .into_any_element(),
        )
    }
}

/// Lane sketch for one ref-moving action: two lanes, static dots, one
/// travelling dot on a looping path (or a parked dot under reduced motion).
fn explainer_diagram(
    kind: ExplainerKind,
    generation: u64,
    reduce_motion: bool,
    cx: &Context<SpurShell>,
) -> gpui_kit::AnyElement {
    /// Canvas geometry: lanes at these heights, full-bleed dots at 8px.
    const UPPER: f32 = 12.0;
    const LOWER: f32 = 32.0;
    const X0: f32 = 14.0;
    // (lanes, static dots, travel path, flag, dot color)
    let accent = violet(cx);
    let lanes = match kind {
        ExplainerKind::CherryPick => vec![UPPER, LOWER],
        _ => vec![UPPER],
    };
    let statics: Vec<(f32, f32, gpui_kit::Hsla)> = match kind {
        ExplainerKind::CherryPick => vec![(170.0, LOWER, text_faint(cx))],
        ExplainerKind::Revert => vec![(60.0, UPPER, cx.theme().warning)],
        ExplainerKind::Reset => vec![(60.0, UPPER, cx.theme().danger)],
        ExplainerKind::CheckoutDetached => vec![],
        ExplainerKind::CreateBranch => vec![(60.0, UPPER, accent)],
        ExplainerKind::CreateTag => vec![(60.0, UPPER, cx.theme().warning)],
        _ => vec![],
    };
    let path: Vec<(f32, f32)> = match kind {
        ExplainerKind::CherryPick => vec![(170.0, LOWER), (60.0, UPPER)],
        ExplainerKind::Revert => vec![(60.0, UPPER), (120.0, UPPER)],
        ExplainerKind::Reset => vec![(170.0, UPPER), (60.0, UPPER)],
        ExplainerKind::CheckoutDetached => vec![(60.0, UPPER), (60.0, 40.0)],
        ExplainerKind::CreateBranch | ExplainerKind::CreateTag => {
            vec![(60.0, UPPER), (150.0, UPPER)]
        }
        _ => vec![(60.0, UPPER)],
    };
    let dot_color = match kind {
        ExplainerKind::Revert | ExplainerKind::CreateTag => cx.theme().warning,
        ExplainerKind::Reset => cx.theme().danger,
        ExplainerKind::CheckoutDetached => text_muted(cx),
        _ => accent,
    };
    let flag: Option<(f32, gpui_kit::Hsla)> = match kind {
        ExplainerKind::CreateBranch => Some((150.0, accent)),
        ExplainerKind::CreateTag => Some((150.0, cx.theme().warning)),
        _ => None,
    };
    let mut canvas = div().relative().w_full().h(px(46.)).overflow_hidden();
    for y in lanes {
        canvas = canvas.child(
            div()
                .absolute()
                .left(px(X0))
                .right(px(X0))
                .top(px(y))
                .h(px(2.))
                .rounded(px(1.))
                .bg(ink(0.14)),
        );
    }
    for (x, y, color) in statics {
        canvas = canvas.child(
            div()
                .absolute()
                .left(px(x - 4.0))
                .top(px(y - 4.0))
                .w(px(8.))
                .h(px(8.))
                .rounded(px(4.))
                .bg(color),
        );
    }
    if let Some((x, color)) = flag {
        canvas = canvas
            .child(
                div()
                    .absolute()
                    .left(px(x - 1.0))
                    .top(px(2.0))
                    .w(px(2.))
                    .h(px(10.))
                    .bg(color),
            )
            .child(
                div()
                    .absolute()
                    .left(px(x + 1.0))
                    .top(px(0.0))
                    .w(px(9.))
                    .h(px(7.))
                    .rounded(px(2.))
                    .border_1()
                    .border_color(color),
            );
    }
    if reduce_motion {
        let (x, y) = path.last().copied().unwrap_or((X0, UPPER));
        canvas = canvas.child(
            div()
                .absolute()
                .left(px(x - 4.0))
                .top(px(y - 4.0))
                .w(px(8.))
                .h(px(8.))
                .rounded(px(4.))
                .bg(dot_color),
        );
    } else {
        canvas = canvas.child(
            div()
                .absolute()
                .w(px(8.))
                .h(px(8.))
                .rounded(px(4.))
                .bg(dot_color)
                .with_animation(
                    SharedString::from(format!("explainer-dot-{generation}")),
                    Animation::new(DIAGRAM_PERIOD).repeat(),
                    move |el, t| {
                        let (x, y) = dot_along(t, &path);
                        el.left(px(x - 4.0)).top(px(y - 4.0)).opacity(fade_ends(t))
                    },
                ),
        );
    }
    canvas.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popout_sits_beside_the_row_and_flips_at_the_edge() {
        // Room on the right: card clears the menu edge plus the gap,
        // top-aligned with the row.
        assert_eq!(
            place_popout(400.0, 200.0, 220.0, 1920.0, 1080.0),
            (400.0 + 220.0 + 13.0 + 6.0, 200.0)
        );
        // No room: card goes left, clamped into the window.
        let (left, top) = place_popout(1700.0, 200.0, 220.0, 1920.0, 1080.0);
        assert_eq!(left, 1700.0 - 13.0 - 6.0 - 264.0);
        assert_eq!(top, 200.0);
        // Top-aligned rows clamp to the top margin, bottom rows to the
        // estimated card height.
        assert_eq!(
            place_popout(400.0, 0.0, 220.0, 1920.0, 1080.0),
            (400.0 + 220.0 + 13.0 + 6.0, 8.0)
        );
        let (_, bottom) = place_popout(400.0, 1070.0, 220.0, 1920.0, 1080.0);
        assert_eq!(bottom, 1080.0 - 200.0 - 8.0);
    }

    #[test]
    fn dot_travels_waypoints_and_wraps() {
        let line = [(0.0, 0.0), (100.0, 50.0)];
        assert_eq!(dot_along(0.0, &line), (0.0, 0.0));
        assert_eq!(dot_along(0.5, &line), (50.0, 25.0));
        // t = 1 wraps to the start (seamless loop).
        assert_eq!(dot_along(1.0, &line), (0.0, 0.0));
        let bent = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)];
        assert_eq!(dot_along(0.75, &bent), (100.0, 50.0));
        assert_eq!(dot_along(0.0, &[]), (0.0, 0.0));
        assert_eq!(dot_along(0.3, &[(7.0, 9.0)]), (7.0, 9.0));
    }

    #[test]
    fn dot_fade_covers_the_loop() {
        assert!((fade_ends(0.5) - 1.0).abs() < f32::EPSILON);
        assert!(fade_ends(0.0) >= 0.15);
        assert!(fade_ends(1.0) >= 0.15);
        assert!(fade_ends(-0.5) >= 0.0);
    }
}
