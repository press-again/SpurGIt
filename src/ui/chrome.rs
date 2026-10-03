//! Chrome: app bar (identity + tabs + actions), empty state, floating
//! operations card with an overlay log.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Sizable as _};
use gpui_kit::{
    rgb, Hsla, InteractiveElement as _, IntoElement, MouseButton, StatefulInteractiveElement as _,
    Window, WindowControlArea,
};

use crate::i18n::t;

use super::app_icon::{self, IconSlot};

/// Native Windows caption button width (the system metric).
const CAPTION_W: f32 = 46.0;

impl SpurShell {
    // ---- app bar: identity, repository tabs, actions, caption controls ----
    //
    // One 38px row holds the title bar and tab strip. The native
    // OS caption is hidden (`appears_transparent`); minimize/maximize/close
    // are drawn here with platform control-area hit tests, so Windows still
    // owns drag, snap layouts, double-click zoom and the resize frame.

    pub(super) fn render_chrome(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // A rescan or a status round is running somewhere; the slot next to
        // the refresh button carries a small spinner so activity is visible
        // even when the table itself does not change.
        let busy = self.discovering || self.overview.refresh_loop.in_flight();

        // Fetch, Push and Pull act on the active repository — the scope
        // their dialogs already use — so each tooltip names that repository.
        // At most one button is filled: Pull while behind, Push while ahead
        // and not behind, none when in sync or diverged (the pull tooltip
        // then explains why a fast-forward is not possible). Counts render
        // only when non-zero, so the buttons stay snug.
        let active = self.active_repo();
        let repo = active.map(|row| row.name.clone());
        let ahead = active.map_or(0, |row| row.ahead);
        let behind = active.map_or(0, |row| row.behind);
        let diverged = ahead > 0 && behind > 0;
        let primary = primary_action(ahead, behind);
        // No repository open: the buttons cannot act, so they say so.
        let no_repo_tip = || t().no_repository_open.to_string();
        let fetch_tip = repo
            .as_deref()
            .map_or_else(no_repo_tip, |name| t().fetch_scope_tip(name));
        let push_tip = repo
            .as_deref()
            .map_or_else(no_repo_tip, |name| t().push_scope_tip(name));
        let pull_tip = match repo.as_deref() {
            Some(name) if diverged => t().pull_diverged_tip(name),
            Some(name) => t().pull_scope_tip(name),
            None => no_repo_tip(),
        };
        let no_repo = repo.is_none();
        let fetch_btn = Button::new("fetch")
            .icon(Icon::new(IconName::CloudDownload).size(px(13.)))
            .label(t().fetch_label)
            .small()
            .outline()
            .cursor_pointer()
            .disabled(no_repo)
            .tooltip(fetch_tip)
            .on_click(cx.listener(|this, _, _, cx| this.request_fetch(cx)));
        let push_btn = {
            let btn = Button::new("push")
                .icon(Icon::new(IconName::CloudUpload).size(px(13.)))
                .label(t().push_label)
                .small()
                .children((ahead > 0).then(|| count_label(ahead, "⇡")))
                .cursor_pointer()
                .disabled(no_repo)
                .tooltip(push_tip)
                .on_click(cx.listener(|this, _, _, cx| this.request_push(cx)));
            if primary == Primary::Push {
                btn.primary()
            } else {
                btn.outline()
            }
        };
        let pull_btn = {
            let btn = Button::new("pull")
                .icon(Icon::new(IconName::ArrowDownToLine).size(px(13.)))
                .label(t().pull_label)
                .small()
                .children((behind > 0).then(|| count_label(behind, "⇣")))
                .cursor_pointer()
                .disabled(no_repo)
                .tooltip(pull_tip)
                .on_click(cx.listener(|this, _, _, cx| this.request_pull(cx)));
            if primary == Primary::Pull {
                btn.primary()
            } else {
                btn.outline()
            }
        };

        // The macOS traffic lights are drawn over the bar's left edge.
        let left_pad = if cfg!(target_os = "macos") { 80. } else { 12. };
        // Only Windows answers the drag control area itself; elsewhere the
        // identity needs explicit window-move handlers.
        #[allow(unused_mut)] // only the non-Windows handlers below mutate it
        let mut identity = div()
            .flex_none()
            .flex()
            .items_center()
            .h_full()
            .gap(px(6.))
            .pr(px(8.))
            .window_control_area(WindowControlArea::Drag)
            .child(brand::branchling(16., self.brand_face(), violet(cx), window))
            .child(brand::wordmark(12., text_primary(cx)));
        #[cfg(not(target_os = "windows"))]
        {
            identity = identity.on_mouse_down(MouseButton::Left, |event, window, _| {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            });
        }

        div()
            .flex()
            .items_center()
            .h(px(TITLEBAR_H))
            .pl(px(left_pad))
            .gap(px(4.))
            .bg(cx.theme().background)
            .border_b_1()
            .border_color(hairline(0.05))
            // The identity is part of the drag surface: the branchling, 16
            // logical px (the component picks the 16/20/24 pixel map for the
            // monitor's scale factor), then the outlined wordmark.
            .child(identity)
            .child(icon_button_element(
                "settings",
                app_icon::render(IconSlot::Settings, text_muted(cx), cx),
                t().tooltip_settings,
                cx.listener(|this, _, window, cx| this.toggle_settings(window, cx)),
            ))
            .child(self.render_tabs(cx))
            .child(
                // Fixed slot: the spinner never shifts the controls.
                // Clicking opens the operation log.
                div()
                    .id("activity-log")
                    .cursor_pointer()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(14.))
                    .h_full()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_oplog(cx)))
                    .children(
                        busy.then(|| Spinner::new().with_size(px(12.)).color(text_muted(cx))),
                    ),
            )
            .child(icon_button_element(
                "refresh",
                app_icon::render(IconSlot::Refresh, text_muted(cx), cx),
                t().tooltip_refresh,
                cx.listener(|this, _, _, cx| this.refresh_workspace(cx)),
            ))
            .child(div().w(px(6.)))
            .child(fetch_btn)
            .child(push_btn)
            .child(pull_btn)
            .child(div().w(px(8.)))
            .children(caption_controls(window, cx))
    }

    pub(super) fn render_empty_state(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Discovery is user-visible work, so the character watches while the
        // roots are scanned; otherwise the derived face (a running network
        // operation focuses it too). The text still carries the meaning; the
        // character is decorative.
        let face = if self.discovering {
            brand::Face::Focus
        } else {
            self.brand_face()
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .child(brand::branchling(72., face, violet(cx), window))
            // First run only: the product's one brand
            // line, gone once a root or a repository arrives.
            .children((!self.welcomed).then(|| {
                div()
                    .pb(px(6.))
                    .text_size(px(TEXT_MD))
                    .text_color(text_muted(cx))
                    .child(t().first_run)
            }))
            .child(
                div()
                    .text_size(px(TEXT_LG))
                    .text_color(text_primary(cx))
                    .child(t().no_repository_open),
            )
            .child(
                div()
                    .text_size(px(TEXT_MD))
                    .text_color(text_muted(cx))
                    .child(t().press_ctrl_k),
            )
            .child(
                div().pt(px(6.)).child(
                    Button::new("open-repo-empty")
                        .child(app_icon::render(IconSlot::OpenRepo, text_primary(cx), cx))
                        .label(t().open_repository)
                        .small()
                        .outline()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.toggle_palette(window, cx))),
                ),
            )
    }
}

// ---- title-bar action counts ----

/// Which remote action gets the one filled (primary) treatment:
/// `Pull` while behind, `Push` while ahead and not behind, nothing while in
/// sync, diverged, or before the first status arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Primary {
    Pull,
    Push,
    None,
}

fn primary_action(ahead: u32, behind: u32) -> Primary {
    match (ahead, behind) {
        (0, b) if b > 0 => Primary::Pull,
        (a, 0) if a > 0 => Primary::Push,
        _ => Primary::None,
    }
}

/// One gis count (`⇡2`) in Geist Mono for a title-bar action. It is only
/// rendered when the count is non-zero, so the button stays snug; the tabs
/// sit left-anchored in their strip, so a count arriving never moves them.
fn count_label(count: u32, glyph: &str) -> gpui_kit::AnyElement {
    div()
        .font_family(MONO)
        .text_size(px(TEXT_SM))
        .child(format!("{glyph}{count}"))
        .into_any_element()
}

// ---- native caption controls (Zeron/Windows pattern) ----

/// Minimize / maximize-restore / close, flush right in the app bar.
///
/// On Windows each button is a platform control area: the OS answers the hit
/// test (HTMINBUTTON/HTMAXBUTTON/HTCLOSE) and owns the behavior, including
/// Snap Layouts and the close action. Other platforms (not shipped) get
/// explicit handlers so the code stays portable.
fn caption_controls(window: &Window, cx: &Context<SpurShell>) -> Vec<gpui_kit::AnyElement> {
    // macOS keeps its native traffic lights.
    if cfg!(target_os = "macos") {
        return Vec::new();
    }
    let max_icon = if window.is_maximized() {
        IconName::WindowRestore
    } else {
        IconName::WindowMaximize
    };
    vec![
        caption_button(
            "window-minimize",
            IconName::WindowMinimize,
            WindowControlArea::Min,
            false,
            cx,
        ),
        caption_button(
            "window-maximize",
            max_icon,
            WindowControlArea::Max,
            false,
            cx,
        ),
        caption_button(
            "window-close",
            IconName::WindowClose,
            WindowControlArea::Close,
            true,
            cx,
        ),
    ]
}

fn caption_button(
    id: &'static str,
    icon: IconName,
    area: WindowControlArea,
    close: bool,
    cx: &Context<SpurShell>,
) -> gpui_kit::AnyElement {
    // Native hover behavior: a white wash for min/max, the system red for
    // close. Snaps like the OS instead of blending.
    let hover_bg: Hsla = if close { rgb(0xe81123).into() } else { ink(0.08) };
    let active_bg: Hsla = if close { rgb(0xc50f1f).into() } else { ink(0.12) };
    let hover_fg: Hsla = if close { gpui_kit::white() } else { text_primary(cx) };

    let mut btn = div()
        .id(id)
        .occlude()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(px(CAPTION_W))
        .h_full()
        .text_color(text_muted(cx))
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .active(move |s| s.bg(active_bg).text_color(hover_fg));

    #[cfg(target_os = "windows")]
    {
        btn = btn.window_control_area(area);
    }
    #[cfg(not(target_os = "windows"))]
    {
        btn = btn.on_click(move |_, window, _| match area {
            WindowControlArea::Min => window.minimize_window(),
            WindowControlArea::Max => window.zoom_window(),
            WindowControlArea::Close => window.remove_window(),
            WindowControlArea::Drag => {}
        });
    }

    btn.child(Icon::new(icon).size(px(13.))).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_action_fills_at_most_one_button_and_only_when_needed() {
        assert_eq!(primary_action(0, 3), Primary::Pull, "behind pulls");
        assert_eq!(primary_action(2, 0), Primary::Push, "ahead pushes");
        assert_eq!(primary_action(0, 0), Primary::None, "in sync: all outlined");
        assert_eq!(
            primary_action(2, 3),
            Primary::None,
            "diverged: all outlined, pull tooltip explains"
        );
    }
}
