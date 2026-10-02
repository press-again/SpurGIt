//! Push dialog: explicit destination — the checked-out branch is shown
//! read-only, the remote is a dropdown, and the target branch is editable.
//! The refspec is sent explicitly and no force option exists.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _, DropdownButton};
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Sizable as _};
use gpui_kit::IntoElement;

use crate::i18n::t;

/// A push request while the dialog is open.
#[derive(Clone, Debug)]
pub(super) struct PushRequest {
    pub repo_id: String,
    /// Checked-out branch being pushed.
    pub branch: String,
    /// Configured upstream (`origin/main`), when any.
    pub upstream: Option<String>,
    /// Currently selected remote.
    pub remote: String,
    /// Selected destination branch on that remote.
    pub target: String,
    /// True while the queued push runs; the dialog stays open with a
    /// progress bar until the operation completes.
    pub busy: bool,
}

impl SpurShell {
    /// Remotes of the active repository: the inspector list plus any remote
    /// only seen in the ref data (details still loading).
    pub(super) fn remote_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .active_details()
            .map(|details| {
                details
                    .remotes
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .unwrap_or_default();
        for branch in self.remote_branches.iter() {
            if !names.contains(&branch.remote) {
                names.push(branch.remote.clone());
            }
        }
        names
    }

    /// Open the push dialog for the active repository. Refuses with a log
    /// line when there is no branch, no commit, or no remote.
    pub(super) fn request_push(&mut self, cx: &mut Context<Self>) {
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        let Some(snapshot) = self.active_snapshot().map(|collected| collected.snapshot.clone())
        else {
            self.ops
                .push_info(t().log_action_failed("push", "status not collected yet"));
            cx.notify();
            return;
        };
        let Some(branch) = snapshot.branch.clone() else {
            self.ops
                .push_info(t().log_action_failed("push", "no branch checked out"));
            cx.notify();
            return;
        };
        if snapshot.unborn {
            self.ops
                .push_info(t().log_action_failed("push", "no commits yet"));
            cx.notify();
            return;
        }
        let upstream = snapshot.upstream.clone();
        // Remote names come from the inspector, with the upstream's remote as
        // a fallback while the details are still loading.
        let mut remotes = self.remote_names();
        if remotes.is_empty()
            && let Some((upstream_remote, _)) = upstream.as_deref().and_then(split_upstream)
        {
            remotes.push(upstream_remote.to_string());
        }
        if remotes.is_empty() {
            self.note_error(t().push_no_remotes.to_string(), cx);
            cx.notify();
            return;
        }
        // Prefer the remembered remote, then the upstream's remote, then the
        // first configured one.
        let remote = self
            .push_remote
            .clone()
            .filter(|name| remotes.contains(name))
            .or_else(|| {
                upstream
                    .as_deref()
                    .and_then(split_upstream)
                    .map(|(remote, _)| remote.to_string())
            })
            .or_else(|| remotes.iter().any(|name| name == "origin").then(|| "origin".to_string()))
            .unwrap_or_else(|| remotes[0].clone());
        // Target: the upstream branch when it lives on the chosen remote,
        // otherwise the local branch name.
        let target = upstream
            .as_deref()
            .and_then(split_upstream)
            .filter(|(upstream_remote, _)| *upstream_remote == remote.as_str())
            .map(|(_, branch)| branch.to_string())
            .unwrap_or_else(|| branch.clone());
        self.push_request = Some(PushRequest {
            repo_id,
            branch,
            upstream,
            remote,
            target,
            busy: false,
        });
        self.open_modal(cx);
        cx.notify();
    }

    /// Branch candidates offered for one remote: its remote-tracking
    /// branches, the upstream branch (when it is not fetched yet), and the
    /// local branch name (first push / default pull). Shared by the push and
    /// pull dialogs.
    pub(super) fn remote_branch_options(
        &self,
        remote: &str,
        branch: &str,
        upstream: Option<&str>,
    ) -> Vec<String> {
        let mut options: Vec<String> = self
            .remote_branches
            .iter()
            .filter(|item| item.remote == remote)
            .map(|item| item.name.clone())
            .collect();
        if let Some((upstream_remote, upstream_branch)) = upstream.and_then(split_upstream)
            && upstream_remote == remote
            && !options.iter().any(|name| name == upstream_branch)
        {
            options.push(upstream_branch.to_string());
        }
        if !options.iter().any(|name| name == branch) {
            options.push(branch.to_string());
        }
        options.sort();
        options.dedup();
        options
    }

    pub(super) fn cancel_push(&mut self, cx: &mut Context<Self>) {
        if self.push_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: queue the explicit refspec push. The upstream is set when
    /// the branch has none or the chosen destination differs from it. The
    /// dialog stays open (busy) until the operation completes.
    pub(super) fn confirm_push(&mut self, cx: &mut Context<Self>) {
        let Some(mut request) = self.push_request.clone() else {
            return;
        };
        if request.target.is_empty() || request.busy {
            return;
        }
        let destination = format!("{}/{}", request.remote, request.target);
        let set_upstream = request.upstream.as_deref() != Some(destination.as_str());
        self.push_remote = Some(request.remote.clone());
        request.busy = true;
        self.push_request = Some(request.clone());
        self.change_ops.push_back(changes::ChangeOp::Push(changes::PushOp {
            repo_id: request.repo_id,
            remote: request.remote,
            local: request.branch,
            target: request.target,
            set_upstream,
        }));
        self.pump_change_ops(cx);
    }

    /// Full-window scrim + card shown while the dialog is open.
    pub(super) fn render_push_dialog(
        &self,
        request: PushRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let remotes = self.remote_names();
        let menu_remotes = remotes.clone();
        let selected = request.remote.clone();
        let menu_entity = cx.entity().downgrade();
        let remote_menu = DropdownButton::new("push-remote")
            .button(
                Button::new("push-remote-btn")
                    .label(truncate_label(&selected, 28))
                    .ghost()
                    .xsmall()
                    .disabled(request.busy),
            )
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for name in &menu_remotes {
                    let entity = menu_entity.clone();
                    let label = name.clone();
                    let picked = name.clone();
                    let is_selected = name == &selected;
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
                                .child(truncate_label(&label, 32))
                        })
                        .checked(is_selected)
                        .on_click(move |_, _, cx| {
                            let picked = picked.clone();
                            entity
                                .update(cx, |this, cx| {
                                    let (branch, upstream) = this
                                        .push_request
                                        .as_ref()
                                        .map(|request| {
                                            (request.branch.clone(), request.upstream.clone())
                                        })
                                        .unwrap_or_default();
                                    let options = this.remote_branch_options(
                                        &picked,
                                        &branch,
                                        upstream.as_deref(),
                                    );
                                    if let Some(request) = this.push_request.as_mut() {
                                        request.remote = picked.clone();
                                        // A destination that is not offered by
                                        // the new remote falls back to the
                                        // first option (upstream or local).
                                        if !options.contains(&request.target) {
                                            request.target = options
                                                .first()
                                                .cloned()
                                                .unwrap_or_else(|| branch.clone());
                                        }
                                    }
                                    cx.notify();
                                })
                                .ok();
                        }),
                    );
                }
                menu
            });
        let target_options = self.remote_branch_options(
            &request.remote,
            &request.branch,
            request.upstream.as_deref(),
        );
        let target_label = request.target.clone();
        let target_menu_entity = cx.entity().downgrade();
        let target_menu = DropdownButton::new("push-target-select")
            .button(
                Button::new("push-target-btn")
                    .label(truncate_label(&target_label, 28))
                    .ghost()
                    .xsmall()
                    .disabled(request.busy),
            )
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu;
                for name in &target_options {
                    let entity = target_menu_entity.clone();
                    let label = name.clone();
                    let picked = name.clone();
                    let is_selected = name == &target_label;
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
                                .child(truncate_label(&label, 32))
                        })
                        .checked(is_selected)
                        .on_click(move |_, _, cx| {
                            let picked = picked.clone();
                            entity
                                .update(cx, |this, cx| {
                                    if let Some(request) = this.push_request.as_mut() {
                                        request.target = picked.clone();
                                    }
                                    cx.notify();
                                })
                                .ok();
                        }),
                    );
                }
                menu
            });
        let can_push = !request.target.is_empty() && !remotes.is_empty() && !request.busy;

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
                cx.listener(|this, _, _, cx| this.cancel_push(cx)),
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
                                Icon::new(IconName::CloudUpload)
                                    .size(px(16.))
                                    .text_color(violet(cx)),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_LG))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(text_primary(cx))
                                    .child(t().push_title),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().push_local),
                    )
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(TEXT_SM))
                            .text_color(text_primary(cx))
                            .child(request.branch.clone()),
                    )
                    .child(
                        div()
                            .pt(px(2.))
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().push_remote),
                    )
                    .child(select_shell(remote_menu))
                    .child(
                        div()
                            .pt(px(2.))
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().push_target),
                    )
                    .child(select_shell(target_menu))
                    .children(request.busy.then(|| {
                        busy_bar(
                            t().push_running(&request.branch, &request.remote, &request.target),
                            true,
                            cx,
                        )
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("push-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .disabled(request.busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_push(cx))),
                            )
                            .child(
                                Button::new("push-confirm")
                                    .label(t().push_confirm)
                                    .small()
                                    .primary()
                                    .disabled(!can_push)
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.confirm_push(cx)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// Split a configured upstream (`origin/feature/x`) into remote and branch.
pub(super) fn split_upstream(upstream: &str) -> Option<(&str, &str)> {
    let (remote, branch) = upstream.split_once('/')?;
    (!remote.is_empty() && !branch.is_empty()).then_some((remote, branch))
}
