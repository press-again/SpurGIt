//! Scan-roots manager: list/add/remove the paths discovery scans.
//! Roots may be Linux (WSL) or Windows paths, mixed in one list.
//! Shares the palette's card surface and row vocabulary.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{ActiveTheme, Icon, Sizable as _};
use gpui_kit::{InteractiveElement as _, KeyDownEvent, StatefulInteractiveElement as _};

use crate::i18n::t;
use crate::model::RootKind;

impl SpurShell {
    pub(super) fn open_roots(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.roots_open = true;
        self.roots_focus_pending = true;
        self.palette_open = false;
        self.set_dialog_width(ROOTS_W, cx); // palette -> roots morphs
        crate::logging::log!("roots: open");
        cx.notify();
        let _ = window;
    }

    /// Esc (or clicking the scrim) inside the roots dialog goes *back* to the
    /// palette instead of closing every overlay.
    pub(super) fn close_roots_to_palette(&mut self, cx: &mut Context<Self>) {
        if self.roots_open {
            self.roots_open = false;
            self.palette_open = true;
            self.palette_active = 0;
            self.palette_focus_pending = true;
            self.set_dialog_width(PALETTE_W, cx); // roots -> palette morphs back
            crate::logging::log!("roots: back to palette");
            cx.notify();
        }
    }

    pub(super) fn add_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.root_input.read(cx).value().trim().to_string();
        if path.is_empty() {
            return;
        }
        // Typed Windows folders and WSL UNC paths become Linux roots.
        match to_root_string(&path) {
            Ok(root) if !root.is_empty() && !self.roots.contains(&root) => {
                self.ops.push_info(t().log_scan_root_added(&root));
                self.roots.push(root);
                self.persist_roots(cx);
                self.rescan_workspace(cx);
            }
            Ok(_) => {}
            Err(distro) => self.note_error(t().log_foreign_distro(&distro), cx),
        }
        self.root_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    fn remove_root(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.roots.len() {
            self.roots.remove(ix);
            self.persist_roots(cx);
            self.rescan_workspace(cx);
            cx.notify();
        }
    }

    pub(super) fn render_roots_card(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        if std::mem::take(&mut self.roots_focus_pending) {
            self.root_input
                .update(cx, |state, cx| state.focus(window, cx));
        }
        let count = self.roots.len();
        let rows: Vec<gpui_kit::AnyElement> = if count == 0 {
            vec![
                div()
                    .py(px(14.))
                    .flex()
                    .justify_center()
                    .text_size(px(TEXT_SM))
                    .text_color(text_muted(cx))
                    .child(t().no_roots)
                    .into_any_element(),
            ]
        } else {
            self.roots
                .iter()
                .enumerate()
                .map(|(ix, root)| {
                    let kind = crate::model::root_kind(root);
                    let kind_label = match kind {
                        RootKind::Wsl => t().root_kind_wsl,
                        RootKind::Windows => t().root_kind_windows,
                    };
                    // Environment markers stay neutral.
                    let kind_color = text_muted(cx);
                    let open_key = format!("root-open-{ix}");
                    let remove_key = format!("root-remove-{ix}");
                    div()
                        .id(("root", ix))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(32.))
                        .px(px(10.))
                        .rounded(px(CONTROL_RADIUS))
                        .bg(hover_blend(
                            &format!("root-row-{ix}"),
                            ink(0.0),
                            ink(0.04),
                        ))
                        .on_hover(hover_listener(format!("root-row-{ix}")))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family(MONO)
                                .text_size(px(TEXT_SM))
                                .text_color(text_muted(cx))
                                .overflow_hidden()
                                .child(root.clone()),
                        )
                        .children(cfg!(windows).then(|| chip(kind_label, kind_color)))
                        .child(
                            div()
                                .id(("root-open", ix))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(22.))
                                .h(px(22.))
                                .rounded(px(5.))
                                .bg(hover_blend(&open_key, ink(0.0), ink(0.08)))
                                .on_hover(hover_listener(open_key))
                                .on_click(cx.listener({
                                    let root = root.clone();
                                    move |this, _, _, cx| {
                                        this.open_in_explorer(&root, cx);
                                    }
                                }))
                                .child(
                                    Icon::new(IconName::FolderOpen)
                                        .size(px(13.))
                                        .text_color(text_muted(cx)),
                                ),
                        )
                        .child(
                            div()
                                .id(("root-remove", ix))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(22.))
                                .h(px(22.))
                                .rounded(px(5.))
                                .bg(hover_blend(&remove_key, ink(0.0), ink(0.08)))
                                .on_hover(hover_listener(remove_key))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove_root(ix, cx)
                                }))
                                .child(
                                    Icon::new(IconName::Close)
                                        .size(px(12.))
                                        .text_color(text_muted(cx)),
                                ),
                        )
                        .into_any_element()
                })
                .collect()
        };

        div()
            .id("roots-dialog")
            .w(px(ROOTS_W))
            .max_h(px(520.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(PALETTE_RADIUS))
            .border_1()
            .border_color(hairline(0.10))
            .bg(cx.theme().popover)
            .shadow_lg()
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.stop_propagation()),
            )
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key.as_str() == "escape" {
                    this.close_roots_to_palette(cx);
                }
            }))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_h(px(44.))
                    .px(px(16.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(hairline(0.06))
                    .child(
                        Icon::new(IconName::FolderOpen)
                            .size(px(16.))
                            .flex_none()
                            .text_color(text_muted(cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(TEXT_MD))
                            .text_color(text_primary(cx))
                            .child(t().scan_roots),
                    )
                    .child(kbd_chip("esc", cx)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px(px(16.))
                    .pt(px(8.))
                    .text_size(px(11.5))
                    .text_color(text_muted(cx))
                    .child(t().roots_hint_native()),
            )
            .child(
                div()
                    .id("roots-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .p(px(6.))
                    .children(rows),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(hairline(0.06))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .px(px(10.))
                            .py(px(6.))
                            .rounded(px(8.))
                            .border_1()
                            .border_color(hairline(0.08))
                            .bg(ink(0.04))
                            .child(
                                Input::new(&self.root_input)
                                    .appearance(false)
                                    .focus_bordered(false)
                                    .w_full(),
                            ),
                    )
                    .child(
                        Button::new("browse-root")
                            .icon(Icon::new(IconName::FolderOpen).size(px(14.)))
                            .label(t().browse)
                            .small()
                            .ghost()
                            .cursor_pointer()
                            .tooltip(t().tooltip_browse_native())
                            .on_click(cx.listener(|this, _, _, cx| this.browse_for_root(cx))),
                    )
                    .child(
                        Button::new("add-root")
                            .icon(Icon::new(IconName::Plus).size(px(14.)))
                            .label(t().add)
                            .small()
                            .primary()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, window, cx| this.add_root(window, cx))),
                    ),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .h(px(32.))
                    .px(px(12.))
                    .border_t_1()
                    .border_color(hairline(0.06))
                    .child(key_hint("↵", t().hint_add, cx))
                    .child(key_hint("esc", t().hint_back, cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(text_faint(cx))
                            .child(t().roots_count(count)),
                    ),
            )
    }
}
