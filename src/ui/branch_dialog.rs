//! Create Branch dialog: base branch, name, what to do with
//! local changes, checkout after create, and overwrite of an existing branch.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _, DropdownButton};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Sizable as _};
use gpui_kit::{IntoElement, StatefulInteractiveElement as _};

use crate::git::BranchChanges;
use crate::i18n::t;

/// A create-branch request while the dialog is open. The branch name lives in
/// [`SpurShell::branch_name_input`].
#[derive(Clone, Debug)]
pub(super) struct CreateBranchRequest {
    pub repo_id: String,
    pub base: String,
    pub checkout: bool,
    pub overwrite: bool,
    pub changes: BranchChanges,
}

/// The queued Git operation; runs through the sequential change queue so it
/// cannot overlap another index/worktree mutation.
#[derive(Clone, Debug)]
pub(super) struct BranchCreateOp {
    pub repo_id: String,
    pub name: String,
    pub base: String,
    pub checkout: bool,
    pub overwrite: bool,
    pub changes: BranchChanges,
}

impl SpurShell {
    /// Open the dialog with the current branch (or the first local branch) as
    /// base and the remembered toggles.
    pub(super) fn request_create_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let base = self
            .active_snapshot()
            .and_then(|collected| collected.snapshot.branch.clone())
            .or_else(|| self.branches.first().map(|branch| branch.name.clone()))
            .unwrap_or_else(|| "HEAD".to_string());
        self.request_create_branch_at(base, window, cx);
    }

    /// Open the dialog with an explicit base (a commit from the graph).
    pub(super) fn request_create_branch_at(
        &mut self,
        base: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        self.branch_name_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.branch_request = Some(CreateBranchRequest {
            repo_id,
            base,
            checkout: self.create_branch_checkout,
            overwrite: self.create_branch_overwrite,
            changes: self.create_branch_changes,
        });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_create_branch(&mut self, cx: &mut Context<Self>) {
        if self.branch_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: remember the choices and queue the Git operation.
    pub(super) fn confirm_create_branch(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.branch_request.clone() else {
            return;
        };
        let name = self.branch_name_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.create_branch_checkout = request.checkout;
        self.create_branch_overwrite = request.overwrite;
        self.create_branch_changes = request.changes;
        self.queue_branch_create(
            BranchCreateOp {
                repo_id: request.repo_id,
                name,
                base: request.base,
                checkout: request.checkout,
                overwrite: request.overwrite,
                changes: request.changes,
            },
            cx,
        );
        self.close_modal(cx);
    }

    /// Queue the operation on the shared sequential change queue.
    pub(super) fn queue_branch_create(&mut self, op: BranchCreateOp, cx: &mut Context<Self>) {
        self.change_ops.push_back(changes::ChangeOp::Branch(op));
        self.pump_change_ops(cx);
    }

    /// Full-window scrim + card shown while the dialog is open.
    pub(super) fn render_branch_dialog(
        &self,
        request: CreateBranchRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let this = cx.entity().downgrade();

        // Base menu: every local branch, plus the current base when it is not
        // a local branch (unborn/detached HEAD).
        let mut names: Vec<String> = self
            .branches
            .iter()
            .map(|branch| branch.name.clone())
            .collect();
        if !names.contains(&request.base) {
            names.insert(0, request.base.clone());
        }
        let base_label = request.base.clone();
        let menu_entity = this.clone();
        let base_menu = DropdownButton::new("branch-base").button(
            Button::new("branch-base-btn")
                .label(truncate_label(&request.base, 32))
                .ghost()
                .xsmall(),
        )
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu;
            for name in &names {
                let entity = menu_entity.clone();
                let selected = name == &base_label;
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
                                    if let Some(request) = this.branch_request.as_mut() {
                                        request.base = picked.clone();
                                    }
                                    cx.notify();
                                })
                                .ok();
                        }),
                );
            }
            menu
        });

        // "What to do with the local changes": Keep / Stash / Discard.
        let options = [
            (
                BranchChanges::Keep,
                t().branch_changes_keep,
                t().branch_changes_keep_hint,
            ),
            (
                BranchChanges::Stash,
                t().branch_changes_stash,
                t().branch_changes_stash_hint,
            ),
            (
                BranchChanges::Discard,
                t().branch_changes_discard,
                t().branch_changes_discard_hint,
            ),
        ];
        let mut modes: Vec<gpui_kit::AnyElement> = Vec::new();
        for (ix, (mode, label, hint)) in options.into_iter().enumerate() {
            let selected = request.changes == mode;
            let entity = this.clone();
            modes.push(
                div()
                    .id(("branch-changes", ix))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(CONTROL_RADIUS))
                    .bg(if selected { ink(0.08) } else { ink(0.0) })
                    .hover(|style| style.bg(ink(0.06)))
                    .child(
                        Icon::new(if selected {
                            IconName::Check
                        } else {
                            IconName::Circle
                        })
                        .size(px(14.))
                        .text_color(if selected { violet(cx) } else { text_faint(cx) }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(TEXT_MD))
                                    .text_color(text_primary(cx))
                                    .child(label),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_XS))
                                    .text_color(text_faint(cx))
                                    .child(hint),
                            ),
                    )
                    .on_click(move |_, _, cx| {
                        entity
                            .update(cx, |this, cx| {
                                if let Some(request) = this.branch_request.as_mut() {
                                    request.changes = mode;
                                }
                                this.create_branch_changes = mode;
                                cx.notify();
                            })
                            .ok();
                    })
                    .into_any_element(),
            );
        }

        let checkout_entity = this.clone();
        let overwrite_entity = this.clone();
        let can_create = !self.branch_name_input.read(cx).value().trim().is_empty();

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
                cx.listener(|this, _, _, cx| this.cancel_create_branch(cx)),
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
                                Icon::new(IconName::GitBranchPlus)
                                    .size(px(16.))
                                    .text_color(violet(cx)),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_LG))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(text_primary(cx))
                                    .child(t().create_branch),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().branch_base),
                    )
                    .child(select_shell(base_menu))
                    .child(
                        div()
                            .pt(px(2.))
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().branch_name),
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
                                Input::new(&self.branch_name_input)
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
                            .child(t().branch_changes),
                    )
                    .children(modes)
                    .child(
                        Checkbox::new("branch-checkout")
                            .checked(request.checkout)
                            .label(t().branch_checkout)
                            .on_change(move |checked, _, cx| {
                                let checked = *checked;
                                checkout_entity
                                    .update(cx, |this, cx| {
                                        if let Some(request) = this.branch_request.as_mut() {
                                            request.checkout = checked;
                                        }
                                        this.create_branch_checkout = checked;
                                        cx.notify();
                                    })
                                    .ok();
                            }),
                    )
                    .child(
                        Checkbox::new("branch-overwrite")
                            .checked(request.overwrite)
                            .label(t().branch_overwrite)
                            .on_change(move |checked, _, cx| {
                                let checked = *checked;
                                overwrite_entity
                                    .update(cx, |this, cx| {
                                        if let Some(request) = this.branch_request.as_mut() {
                                            request.overwrite = checked;
                                        }
                                        this.create_branch_overwrite = checked;
                                        cx.notify();
                                    })
                                    .ok();
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("branch-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_create_branch(cx)),
                                    ),
                            )
                            .child(
                                Button::new("branch-create")
                                    .label(t().branch_create)
                                    .small()
                                    .primary()
                                    .disabled(!can_create)
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.confirm_create_branch(cx)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
