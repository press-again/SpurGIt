//! Tag dialogs: create (optionally pushing the new tag), push to a remote, and
//! delete locally and/or on remotes.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Sizable as _};
use gpui_kit::{IntoElement, SharedString, WeakEntity};
use gpui_kit::component::button::DropdownButton;

use crate::i18n::t;

/// A create-tag request while the dialog is open. Name and message live in
/// [`SpurShell::tag_name_input`] and [`SpurShell::tag_message_input`].
#[derive(Clone, Debug)]
pub(super) struct TagRequest {
    pub repo_id: String,
    pub hash: String,
    pub short: String,
    /// Push the new tag to `remote` right after creating it.
    pub push: bool,
    pub remote: String,
}

/// A tag-push request while the dialog is open: the remote is picked in the
/// dialog, everything else is fixed.
#[derive(Clone, Debug)]
pub(super) struct TagPushRequest {
    pub repo_id: String,
    pub name: String,
    pub remote: String,
}

/// A tag-delete request while the dialog is open: local plus the checked
/// remotes are the destinations.
#[derive(Clone, Debug)]
pub(super) struct TagDeleteRequest {
    pub repo_id: String,
    pub name: String,
    pub local: bool,
    pub remotes: Vec<String>,
}

/// The tag row menu: checkout, copy the name, push to a remote, delete.
pub(super) fn tag_menu(menu: PopupMenu, name: String, this: WeakEntity<SpurShell>) -> PopupMenu {
    menu.item({
        let entity = this.clone();
        let tag = name.clone();
        context_menu_item(
            t().context_checkout.into(),
            IconName::Tag,
            move |_, _, cx| {
                let tag = tag.clone();
                entity
                    .update(cx, |shell, cx| shell.checkout_tag(tag, cx))
                    .ok();
            },
        )
    })
    .item({
        let entity = this.clone();
        let tag = name.clone();
        context_menu_item(
            t().context_copy_tag_name.into(),
            IconName::ClipboardCopy,
            move |_, _, cx| {
                let tag = tag.clone();
                entity
                    .update(cx, |shell, cx| shell.copy_commit_text("tag name", tag, cx))
                    .ok();
            },
        )
    })
    .item({
        let entity = this.clone();
        let tag = name.clone();
        context_menu_item(
            t().context_push_tag.into(),
            IconName::CloudUpload,
            move |_, _, cx| {
                let tag = tag.clone();
                entity
                    .update(cx, |shell, cx| shell.request_tag_push(tag, cx))
                    .ok();
            },
        )
    })
    .item({
        let entity = this.clone();
        let tag = name.clone();
        context_menu_item(
            t().context_delete_branch.into(),
            IconName::Trash,
            move |_, _, cx| {
                let tag = tag.clone();
                entity
                    .update(cx, |shell, cx| shell.request_tag_delete(tag, cx))
                    .ok();
            },
        )
    })
}

impl SpurShell {
    /// Open the dialog for the commit the menu was opened on.
    pub(super) fn request_create_tag(
        &mut self,
        hash: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let repo_id = repo.path.to_string_lossy().into_owned();
        let short = hash.get(..7).unwrap_or(&hash).to_string();
        self.tag_name_input.update(cx, |state, cx| {
            state.set_value("", window, cx)
        });
        self.tag_message_input.update(cx, |state, cx| {
            state.set_value("", window, cx)
        });
        let remote = self.default_tag_remote();
        self.tag_request = Some(TagRequest {
            repo_id,
            hash,
            short,
            push: false,
            remote,
        });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_create_tag(&mut self, cx: &mut Context<Self>) {
        if self.tag_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: queue the creation on the shared sequential change queue.
    pub(super) fn confirm_create_tag(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.tag_request.clone() else {
            return;
        };
        let name = self.tag_name_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        let message = self.tag_message_input.read(cx).value().trim().to_string();
        self.change_ops.push_back(changes::ChangeOp::TagCreate(changes::TagCreateOp {
            repo_id: request.repo_id,
            name,
            hash: request.hash,
            message,
            push_remote: (request.push && !request.remote.is_empty()).then_some(request.remote),
        }));
        self.pump_change_ops(cx);
        self.close_modal(cx);
    }

    /// Check a tag out detached: resolve its commit on the background thread,
    /// then queue the detached checkout like a history-row checkout.
    pub(super) fn checkout_tag(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let worktree = repo.path.to_string_lossy().into_owned();
        cx.spawn(async move |this, cx| {
            let commit = cx
                .background_executor()
                .spawn(async move { crate::git::tag_commit(&worktree, &name) })
                .await;
            this.update(cx, |this, cx| match commit {
                Ok(hash) => {
                    let short = hash.get(..7).unwrap_or(&hash).to_string();
                    this.checkout_commit_detached(hash, short, cx);
                }
                Err(err) => {
                    this.note_error(t().log_action_failed("check out tag", &err), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Remote names for the tag dialogs' pickers, sorted.
    fn tag_remote_names(&self) -> Vec<String> {
        let mut names = self.remote_names();
        names.sort();
        names.dedup();
        names
    }

    /// The remote a tag dialog starts on: `origin`, else the first one.
    fn default_tag_remote(&self) -> String {
        let names = self.remote_names();
        names
            .iter()
            .find(|name| name.as_str() == "origin")
            .or_else(|| names.first())
            .cloned()
            .unwrap_or_default()
    }

    /// Open the push dialog for one tag (remote picker).
    pub(super) fn request_tag_push(&mut self, name: String, cx: &mut Context<Self>) {
        let remotes = self
            .active_details()
            .map(|details| details.remotes.clone())
            .unwrap_or_default();
        if remotes.is_empty() {
            self.note_error(t().push_no_remotes.to_string(), cx);
            return;
        }
        let Some(repo) = self.active_repo() else {
            return;
        };
        let repo_id = repo.path.to_string_lossy().into_owned();
        let remote = self.default_tag_remote();
        self.tag_push_request = Some(TagPushRequest {
            repo_id,
            name,
            remote,
        });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_tag_push(&mut self, cx: &mut Context<Self>) {
        if self.tag_push_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: queue the tag push on the shared sequential change queue.
    pub(super) fn confirm_tag_push(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.tag_push_request.clone() else {
            return;
        };
        self.change_ops.push_back(changes::ChangeOp::TagPush(changes::TagPushOp {
            repo_id: request.repo_id,
            remote: request.remote,
            name: request.name,
        }));
        self.pump_change_ops(cx);
        self.close_modal(cx);
    }

    /// Open the delete dialog: local plus every known remote as destinations.
    pub(super) fn request_tag_delete(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let repo_id = repo.path.to_string_lossy().into_owned();
        self.tag_delete_request = Some(TagDeleteRequest {
            repo_id,
            name,
            local: true,
            remotes: Vec::new(),
        });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_tag_delete(&mut self, cx: &mut Context<Self>) {
        if self.tag_delete_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: queue the delete for every checked destination.
    pub(super) fn confirm_tag_delete(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.tag_delete_request.clone() else {
            return;
        };
        if !request.local && request.remotes.is_empty() {
            return;
        }
        self.change_ops.push_back(changes::ChangeOp::TagDelete(
            changes::TagDeleteOp {
                repo_id: request.repo_id,
                name: request.name,
                local: request.local,
                remotes: request.remotes,
            },
        ));
        self.pump_change_ops(cx);
        self.close_modal(cx);
    }

    /// Full-window scrim + card for the push dialog: remote picker.
    pub(super) fn render_tag_push_dialog(
        &self,
        request: TagPushRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let this = cx.entity().downgrade();
        let remote_menu = remote_picker(
            "tag-push-remote",
            &request.remote,
            self.tag_remote_names(),
            this,
            |shell, remote| {
                if let Some(request) = shell.tag_push_request.as_mut() {
                    request.remote = remote;
                }
            },
        );
        modal_card(
            cx,
            "tag-push",
            IconName::CloudUpload,
            violet(cx),
            format!("{} {}", t().tag_push_title, request.name),
            None,
            vec![(
                t().tag_push_remote,
                select_shell(remote_menu).into_any_element(),
            )],
            t().cancel,
            Self::cancel_tag_push,
            t().tag_push_confirm,
            Self::confirm_tag_push,
            !request.remote.is_empty(),
            false,
        )
    }

    /// Full-window scrim + card for the delete dialog: local plus every
    /// known remote as checkbox destinations.
    pub(super) fn render_tag_delete_dialog(
        &self,
        request: TagDeleteRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let this = cx.entity().downgrade();
        let mut remotes: Vec<String> = self
            .active_details()
            .map(|details| {
                details
                    .remotes
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .unwrap_or_default();
        remotes.sort();
        remotes.dedup();
        let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
        {
            let entity = this.clone();
            let checked = request.local;
            rows.push(
                Checkbox::new("tag-delete-local")
                    .checked(checked)
                    .label(t().tag_delete_local)
                    .on_change(move |checked, _, cx| {
                        let checked = *checked;
                        entity
                            .update(cx, |this, cx| {
                                if let Some(request) = this.tag_delete_request.as_mut() {
                                    request.local = checked;
                                }
                                cx.notify();
                            })
                            .ok();
                    })
                    .into_any_element(),
            );
        }
        for (ix, remote) in remotes.iter().enumerate() {
            let entity = this.clone();
            let checked = request.remotes.contains(remote);
            let toggled = remote.clone();
            let check_id: SharedString = format!("tag-delete-remote-{ix}").into();
            rows.push(
                Checkbox::new(check_id)
                    .checked(checked)
                    .label(remote.clone())
                    .on_change(move |checked, _, cx| {
                        let checked = *checked;
                        let toggled = toggled.clone();
                        entity
                            .update(cx, |this, cx| {
                                if let Some(request) = this.tag_delete_request.as_mut() {
                                    if checked {
                                        if !request.remotes.contains(&toggled) {
                                            request.remotes.push(toggled.clone());
                                        }
                                    } else {
                                        request.remotes.retain(|name| name != &toggled);
                                    }
                                }
                                cx.notify();
                            })
                            .ok();
                    })
                    .into_any_element(),
            );
        }
        modal_card(
            cx,
            "tag-delete",
            IconName::Trash,
            cx.theme().danger,
            t().tag_delete_title.to_string(),
            Some(t().tag_delete_body(&request.name)),
            rows.into_iter()
                .map(|row| ("", row))
                .collect(),
            t().cancel,
            Self::cancel_tag_delete,
            t().tag_delete_confirm,
            Self::confirm_tag_delete,
            request.local || !request.remotes.is_empty(),
            true,
        )
    }

}

/// Remote dropdown shared by the tag dialogs; `pick` stores the choice.
fn remote_picker(
    id: &'static str,
    current: &str,
    names: Vec<String>,
    this: WeakEntity<SpurShell>,
    pick: fn(&mut SpurShell, String),
) -> DropdownButton {
    let current_name = current.to_string();
    DropdownButton::new(id)
        .button(
            Button::new(SharedString::from(format!("{id}-btn")))
                .label(truncate_label(current, 32))
                .ghost()
                .xsmall(),
        )
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu;
            for name in &names {
                let entity = this.clone();
                let selected = name == &current_name;
                let picked = name.clone();
                let label = name.clone();
                menu = menu.item(
                    PopupMenuItem::element(move |_window, _cx| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .self_stretch()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(truncate_label(&label, 42))
                    })
                    .checked(selected)
                    .on_click(move |_, _, cx| {
                        let picked = picked.clone();
                        entity
                            .update(cx, |this, cx| {
                                pick(this, picked);
                                cx.notify();
                            })
                            .ok();
                    }),
                );
            }
            menu
        })
}

/// Shared scrim + card for the tag push/delete dialogs: title row, optional
/// body, rows (a labeled caption renders only for non-empty labels), and
/// Cancel + action buttons.
#[allow(clippy::too_many_arguments)]
fn modal_card(
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
        if !label.is_empty() {
            card = card.child(
                div()
                    .text_size(px(TEXT_XS))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(text_faint(cx))
                    .child(label),
            );
        }
        card = card.child(row);
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

impl SpurShell {
    /// Full-window scrim + card shown while the dialog is open.
    pub(super) fn render_tag_dialog(
        &self,
        request: TagRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let can_create = !self.tag_name_input.read(cx).value().trim().is_empty();
        let remotes = self.tag_remote_names();
        let this = cx.entity().downgrade();
        let push_row = (!remotes.is_empty()).then(|| {
            let entity = this.clone();
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    Checkbox::new("tag-create-push")
                        .checked(request.push)
                        .label(t().tag_push_after)
                        .on_change(move |checked, _, cx| {
                            let checked = *checked;
                            entity
                                .update(cx, |this, cx| {
                                    if let Some(request) = this.tag_request.as_mut() {
                                        request.push = checked;
                                    }
                                    cx.notify();
                                })
                                .ok();
                        }),
                )
                .children(request.push.then(|| {
                    select_shell(remote_picker(
                        "tag-create-remote",
                        &request.remote,
                        remotes,
                        this,
                        |shell, remote| {
                            if let Some(request) = shell.tag_request.as_mut() {
                                request.remote = remote;
                            }
                        },
                    ))
                }))
        });
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
                cx.listener(|this, _, _, cx| this.cancel_create_tag(cx)),
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
                                Icon::new(IconName::Tag)
                                    .size(px(16.))
                                    .text_color(cx.theme().warning),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_LG))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(text_primary(cx))
                                    .child(format!("{} {}", t().tag_title, request.short)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().tag_name),
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
                                Input::new(&self.tag_name_input)
                                    .appearance(false)
                                    .focus_bordered(false)
                                    .w_full(),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().tag_message),
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
                                Input::new(&self.tag_message_input)
                                    .appearance(false)
                                    .focus_bordered(false)
                                    .w_full(),
                            ),
                    )
                    .children(push_row)
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("tag-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_create_tag(cx)),
                                    ),
                            )
                            .child(
                                Button::new("tag-create")
                                    .label(t().tag_create)
                                    .small()
                                    .primary()
                                    .disabled(!can_create)
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.confirm_create_tag(cx)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
