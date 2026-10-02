//! "Apply patch from clipboard": a palette entry reads the clipboard,
//! previews the touched files in a modal, and queues `git apply --3way` on
//! the sequential change queue.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme, Sizable as _};
use gpui_kit::IntoElement;

use crate::i18n::t;

/// Preview rows before the "+N more" line; a pasted patch can touch more
/// files than a modal can usefully list.
const APPLY_PATCH_PREVIEW_FILES: usize = 30;

/// A clipboard patch awaiting confirmation. The patch text is kept so the
/// confirm path needs no second clipboard read (the user could copy
/// something else while the modal is open).
#[derive(Clone, Debug)]
pub(super) struct ApplyPatchRequest {
    pub repo_id: String,
    pub patch: String,
    pub files: Vec<String>,
}

impl SpurShell {
    /// Palette entry: read the clipboard, preview its files, or explain why
    /// there is nothing to apply. Needs an active repository to apply into.
    pub(super) fn request_apply_patch(&mut self, cx: &mut Context<Self>) {
        let Some(repo_id) = self.active_repo_id() else {
            self.note_error(t().log_nothing_to_reveal(), cx);
            return;
        };
        let patch = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        if patch.trim().is_empty() {
            self.note_error(t().clipboard_empty.to_string(), cx);
            return;
        }
        let files = crate::git::patch_file_list(&patch);
        if files.is_empty() {
            self.note_error(t().clipboard_not_patch.to_string(), cx);
            return;
        }
        self.apply_patch_request = Some(ApplyPatchRequest {
            repo_id,
            patch,
            files,
        });
        self.open_modal(cx);
    }

    pub(super) fn cancel_apply_patch(&mut self, cx: &mut Context<Self>) {
        if self.apply_patch_request.is_some() {
            self.close_modal(cx);
        }
    }

    pub(super) fn confirm_apply_patch(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.apply_patch_request.clone() else {
            return;
        };
        self.change_ops.push_back(changes::ChangeOp::ApplyPatch(
            changes::ApplyPatchOp {
                repo_id: request.repo_id,
                patch: request.patch.into_bytes(),
                files: request.files,
            },
        ));
        self.pump_change_ops(cx);
        // `git apply` is local and fast: close like the drop confirm rather
        // than holding a busy modal. Failures surface through the alert.
        self.close_modal(cx);
    }

    /// Scrim + card with the touched-file preview and Apply/Cancel.
    pub(super) fn render_apply_patch_dialog(
        &self,
        request: ApplyPatchRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let hidden = request
            .files
            .len()
            .saturating_sub(APPLY_PATCH_PREVIEW_FILES);
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0.0, 0.0, 0.0, 0.35))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.cancel_apply_patch(cx)),
            )
            .child(
                div()
                    .w(px(460.))
                    .rounded(px(PANEL_RADIUS))
                    .border_1()
                    .border_color(hairline(0.10))
                    .bg(cx.theme().popover)
                    .shadow_lg()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                gpui_kit::component::Icon::new(IconName::ClipboardPaste)
                                    .size(px(16.))
                                    .text_color(text_muted(cx)),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_LG))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(text_primary(cx))
                                    .child(t().apply_patch_title),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_SM))
                            .text_color(text_muted(cx))
                            .child(t().file_count(request.files.len())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .children(request.files.iter().take(APPLY_PATCH_PREVIEW_FILES).map(
                                |file| {
                                    div()
                                        .font_family(MONO)
                                        .text_size(px(TEXT_SM))
                                        .text_color(text_muted(cx))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .child(file.clone())
                                },
                            ))
                            .children((hidden > 0).then(|| {
                                div()
                                    .text_size(px(TEXT_XS))
                                    .text_color(text_faint(cx))
                                    .child(t().apply_patch_more(hidden))
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("apply-patch-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_apply_patch(cx)
                                    })),
                            )
                            .child(
                                Button::new("apply-patch-confirm")
                                    .label(t().context_apply)
                                    .small()
                                    .primary()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_apply_patch(cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}
