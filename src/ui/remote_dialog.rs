//! Add Remote dialog: remote name plus repository URL, opened
//! from the Remotes collapsible header.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Sizable as _};
use gpui_kit::{IntoElement, SharedString, WeakEntity};
use crate::i18n::t;

/// An add-remote request while the dialog is open. The fields live in
/// [`SpurShell::remote_name_input`] / [`SpurShell::remote_url_input`].
#[derive(Clone, Debug)]
pub(super) struct AddRemoteRequest {
    pub repo_id: String,
}

/// An edit-URL request: the remote plus its repo identity. The URL lives in
/// [`SpurShell::remote_url_input`], prefilled with the current one.
#[derive(Clone, Debug)]
pub(super) struct EditUrlRequest {
    pub repo_id: String,
    pub remote: String,
}

/// A rename request: the old name plus its repo identity. The new name lives
/// in [`SpurShell::remote_name_input`], prefilled with the old one.
#[derive(Clone, Debug)]
pub(super) struct RenameRemoteRequest {
    pub repo_id: String,
    pub old: String,
}

/// A remove request: confirmed before anything runs.
#[derive(Clone, Debug)]
pub(super) struct RemoveRemoteRequest {
    pub repo_id: String,
    pub remote: String,
}

/// The remote row menu. Fetch and prune run immediately (fetch honors
/// the remembered dialog options); edit, rename, and remove open dialogs.
pub(super) fn remote_menu(
    menu: PopupMenu,
    remote: String,
    url: String,
    this: WeakEntity<SpurShell>,
) -> PopupMenu {
    menu.item({
        let entity = this.clone();
        let name = remote.clone();
        context_menu_item(
            t().context_fetch_remote.into(),
            IconName::Download,
            move |_, _, cx| {
                let name = name.clone();
                entity
                    .update(cx, |shell, cx| shell.fetch_this_remote(name, cx))
                    .ok();
            },
        )
    })
    .item({
        let entity = this.clone();
        let name = remote.clone();
        context_menu_item(
            t().context_prune_remote.into(),
            IconName::Minus,
            move |_, _, cx| {
                let name = name.clone();
                entity
                    .update(cx, |shell, cx| shell.prune_remote(name, cx))
                    .ok();
            },
        )
    })
    .item({
        let entity = this.clone();
        let name = remote.clone();
        context_menu_item(
            t().context_edit_url.into(),
            IconName::ExternalLink,
            move |_, window, cx| {
                let name = name.clone();
                entity
                    .update(cx, |shell, cx| shell.request_edit_url(name, window, cx))
                    .ok();
            },
        )
    })
    .item({
        let entity = this.clone();
        let name = remote.clone();
        context_menu_item(
            t().context_rename_remote.into(),
            IconName::Circle,
            move |_, window, cx| {
                let name = name.clone();
                entity
                    .update(cx, |shell, cx| shell.request_rename_remote(name, window, cx))
                    .ok();
            },
        )
    })
    .item({
        let entity = this.clone();
        let name = remote.clone();
        context_menu_item(
            t().context_remove_remote.into(),
            IconName::Trash,
            move |_, _, cx| {
                let name = name.clone();
                entity
                    .update(cx, |shell, cx| shell.request_remove_remote(name, cx))
                    .ok();
            },
        )
    })
    .item({
        let entity = this.clone();
        let name = remote.clone();
        context_menu_item(
            t().context_open_in_browser.into(),
            IconName::Globe,
            move |_, _, cx| {
                let name = name.clone();
                let remote_url = url.clone();
                entity
                    .update(cx, |shell, cx| shell.open_remote_in_browser(&name, &remote_url, cx))
                    .ok();
            },
        )
    })
}

impl SpurShell {
    /// Open the dialog with empty fields.
    pub(super) fn request_add_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        self.remote_name_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.remote_url_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.remote_request = Some(AddRemoteRequest { repo_id });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_add_remote(&mut self, cx: &mut Context<Self>) {
        if self.remote_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: queue `git remote add`. Both fields are required.
    pub(super) fn confirm_add_remote(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.remote_request.clone() else {
            return;
        };
        let name = self.remote_name_input.read(cx).value().trim().to_string();
        let url = self.remote_url_input.read(cx).value().trim().to_string();
        if name.is_empty() || url.is_empty() {
            return;
        }
        self.change_ops
            .push_back(changes::ChangeOp::AddRemote(changes::AddRemoteOp {
                repo_id: request.repo_id,
                name,
                url,
            }));
        self.pump_change_ops(cx);
        self.close_modal(cx);
    }

    /// Queue a fetch of one remote with the remembered dialog options.
    pub(super) fn fetch_this_remote(&mut self, remote: String, cx: &mut Context<Self>) {
        if let Some(repo_id) = self.active_repo_id() {
            self.change_ops.push_back(changes::ChangeOp::Fetch(
                changes::FetchOp {
                    repo_id,
                    remote: Some(remote),
                    force: self.fetch_force,
                    no_tags: self.fetch_no_tags,
                    from_dialog: false,
                },
            ));
            self.pump_change_ops(cx);
        }
    }

    /// Queue a prune of one remote's stale tracking branches.
    pub(super) fn prune_remote(&mut self, remote: String, cx: &mut Context<Self>) {
        if let Some(repo_id) = self.active_repo_id() {
            self.change_ops.push_back(changes::ChangeOp::PruneRemote(
                changes::PruneRemoteOp { repo_id, remote },
            ));
            self.pump_change_ops(cx);
        }
    }

    /// Open the URL editor prefilled with the row's remote and URL.
    pub(super) fn request_edit_url(
        &mut self,
        remote: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        let url = self
            .active_details()
            .and_then(|details| {
                details
                    .remotes
                    .iter()
                    .find(|(name, _)| name == &remote)
                    .map(|(_, url)| url.clone())
            })
            .unwrap_or_default();
        self.remote_url_input.update(cx, |state, cx| {
            state.set_value(&url, window, cx)
        });
        self.edit_url_request = Some(EditUrlRequest { repo_id, remote });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_edit_url(&mut self, cx: &mut Context<Self>) {
        if self.edit_url_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: queue the URL change.
    pub(super) fn confirm_edit_url(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.edit_url_request.clone() else {
            return;
        };
        let url = self.remote_url_input.read(cx).value().trim().to_string();
        if url.is_empty() {
            return;
        }
        self.change_ops.push_back(changes::ChangeOp::SetRemoteUrl(
            changes::SetRemoteUrlOp {
                repo_id: request.repo_id,
                remote: request.remote,
                url,
            },
        ));
        self.pump_change_ops(cx);
        self.close_modal(cx);
    }

    /// Open the rename dialog prefilled with the row's remote.
    pub(super) fn request_rename_remote(
        &mut self,
        remote: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        self.remote_name_input.update(cx, |state, cx| {
            state.set_value(&remote, window, cx)
        });
        self.rename_remote_request = Some(RenameRemoteRequest {
            repo_id,
            old: remote,
        });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_rename_remote(&mut self, cx: &mut Context<Self>) {
        if self.rename_remote_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: queue the rename.
    pub(super) fn confirm_rename_remote(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.rename_remote_request.clone() else {
            return;
        };
        let new = self.remote_name_input.read(cx).value().trim().to_string();
        if new.is_empty() {
            return;
        }
        self.change_ops.push_back(changes::ChangeOp::RenameRemote(
            changes::RenameRemoteOp {
                repo_id: request.repo_id,
                old: request.old,
                new,
            },
        ));
        self.pump_change_ops(cx);
        self.close_modal(cx);
    }

    /// Remove asks first, always.
    pub(super) fn request_remove_remote(&mut self, remote: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let repo_id = repo.path.to_string_lossy().into_owned();
        self.remove_remote_request = Some(RemoveRemoteRequest { repo_id, remote });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_remove_remote(&mut self, cx: &mut Context<Self>) {
        if self.remove_remote_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: queue the removal. Local branches stay where they are.
    pub(super) fn confirm_remove_remote(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.remove_remote_request.clone() else {
            return;
        };
        self.change_ops.push_back(changes::ChangeOp::RemoveRemote(
            changes::RemoveRemoteOp {
                repo_id: request.repo_id,
                remote: request.remote,
            },
        ));
        self.pump_change_ops(cx);
        self.close_modal(cx);
    }

    /// Full-window scrim + card for the URL editor: one field, prefilled.
    pub(super) fn render_edit_url_dialog(
        &self,
        request: EditUrlRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let url = self.remote_url_input.read(cx).value().trim().to_string();
        let can_save = !url.is_empty();
        modal_card(
            cx,
            "remote-edit",
            IconName::ExternalLink,
            violet(cx),
            format!("{} {}", t().remote_edit_title, request.remote),
            None,
            vec![(t().remote_url, dialog_input(&self.remote_url_input))],
            t().cancel,
            Self::cancel_edit_url,
            t().remote_save,
            Self::confirm_edit_url,
            can_save,
            false,
        )
    }

    /// Full-window scrim + card for the rename dialog: one field, prefilled.
    pub(super) fn render_rename_remote_dialog(
        &self,
        request: RenameRemoteRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let name = self.remote_name_input.read(cx).value().trim().to_string();
        let can_rename = !name.is_empty();
        modal_card(
            cx,
            "remote-rename",
            IconName::Circle,
            text_primary(cx),
            format!("{} {}", t().remote_rename_title, request.old),
            None,
            vec![(t().rename_name, dialog_input(&self.remote_name_input))],
            t().cancel,
            Self::cancel_rename_remote,
            t().rename_confirm,
            Self::confirm_rename_remote,
            can_rename,
            false,
        )
    }

    /// Full-window scrim + card for the remove confirm.
    pub(super) fn render_remove_remote_dialog(
        &self,
        request: RemoveRemoteRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        modal_card(
            cx,
            "remote-remove",
            IconName::Trash,
            cx.theme().danger,
            t().remote_remove_title.to_string(),
            Some(t().remote_remove_body(&request.remote)),
            vec![],
            t().cancel,
            Self::cancel_remove_remote,
            t().remote_remove_confirm,
            Self::confirm_remove_remote,
            true,
            true,
        )
    }

    /// Full-window scrim + card shown while the dialog is open.
    pub(super) fn render_add_remote_dialog(
        &self,
        _request: AddRemoteRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let name = self.remote_name_input.read(cx).value().trim().to_string();
        let url = self.remote_url_input.read(cx).value().trim().to_string();
        let can_add = !name.is_empty() && !url.is_empty();

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
                cx.listener(|this, _, _, cx| this.cancel_add_remote(cx)),
            )
            .child(
                div()
                    .w(px(480.))
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
                                Icon::new(IconName::Cloud)
                                    .size(px(16.))
                                    .text_color(violet(cx)),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_LG))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(text_primary(cx))
                                    .child(t().add_remote),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().remote_name),
                    )
                    .child(
                        div()
                            .rounded(px(CONTROL_RADIUS))
                            .border_1()
                            .border_color(hairline(0.10))
                            .bg(ink(0.03))
                            .px(px(6.))
                            .py(px(2.))
                            .child(
                                Input::new(&self.remote_name_input)
                                    .appearance(false)
                                    .focus_bordered(false)
                                    .w_full(),
                            ),
                    )
                    .child(
                        div()
                            .pt(px(2.))
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().remote_url),
                    )
                    .child(
                        div()
                            .rounded(px(CONTROL_RADIUS))
                            .border_1()
                            .border_color(hairline(0.10))
                            .bg(ink(0.03))
                            .px(px(6.))
                            .py(px(2.))
                            .child(
                                Input::new(&self.remote_url_input)
                                    .appearance(false)
                                    .focus_bordered(false)
                                    .w_full(),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("remote-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_add_remote(cx)),
                                    ),
                            )
                            .child(
                                Button::new("remote-add")
                                    .label(t().remote_add)
                                    .small()
                                    .primary()
                                    .disabled(!can_add)
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.confirm_add_remote(cx)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// One bordered input row inside the small remote dialogs.
pub(super) fn dialog_input(input: &Entity<InputState>) -> gpui_kit::AnyElement {
    div()
        .rounded(px(CONTROL_RADIUS))
        .border_1()
        .border_color(hairline(0.10))
        .bg(ink(0.03))
        .px(px(6.))
        .py(px(2.))
        .child(
            Input::new(input)
                .appearance(false)
                .focus_bordered(false)
                .w_full(),
        )
        .into_any_element()
}

/// Shared scrim + card for the small remote dialogs.
#[allow(clippy::too_many_arguments)]
pub(super) fn modal_card(
    cx: &mut Context<SpurShell>,
    id_prefix: &'static str,
    icon: IconName,
    icon_color: gpui_kit::Hsla,
    title: String,
    body: Option<String>,
    rows: Vec<(&'static str, gpui_kit::AnyElement)>,
    cancel_label: &'static str,
    cancel_fn: fn(&mut SpurShell, &mut Context<SpurShell>),
    confirm_label: &'static str,
    confirm_fn: fn(&mut SpurShell, &mut Context<SpurShell>),
    confirm_enabled: bool,
    confirm_danger: bool,
) -> gpui_kit::AnyElement {
    let cancel_id: SharedString = format!("{id_prefix}-cancel").into();
    let confirm_id: SharedString = format!("{id_prefix}-confirm").into();
    let mut card = div()
        .w(px(440.))
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
                .child(Icon::new(icon).size(px(16.)).text_color(icon_color))
                .child(
                    div()
                        .text_size(px(TEXT_LG))
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(text_primary(cx))
                        .child(title),
                ),
        );
    if let Some(body) = body {
        card = card.child(
            div()
                .text_size(px(TEXT_MD))
                .text_color(text_muted(cx))
                .child(body),
        );
    }
    for (label, row) in rows {
        card = card
            .child(
                div()
                    .text_size(px(TEXT_XS))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(text_faint(cx))
                    .child(label),
            )
            .child(row);
    }
    let mut confirm_button = Button::new(confirm_id)
        .label(confirm_label)
        .small()
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| confirm_fn(this, cx)));
    confirm_button = if confirm_danger {
        confirm_button.danger()
    } else {
        confirm_button.primary()
    };
    if !confirm_enabled {
        confirm_button = confirm_button.disabled(true);
    }
    let card = card.child(
        div()
            .flex()
            .justify_end()
            .gap(px(8.))
            .child(
                Button::new(cancel_id)
                    .label(cancel_label)
                    .small()
                    .text()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| cancel_fn(this, cx))),
            )
            .child(confirm_button),
    );
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
            cx.listener(move |this, _, _, cx| cancel_fn(this, cx)),
        )
        .child(card)
        .into_any_element()
}
