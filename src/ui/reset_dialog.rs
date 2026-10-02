//! Reset dialog: move the checked-out branch to the selected commit.
//!
//! Soft keeps changes staged, mixed unstaged, hard discards them (backed up
//! by the undo safety net, so the dialog promises "Undo will be available").
//! The branch/HEAD identity captured at confirm time is rechecked by the
//! backend immediately before the reset runs. A pushed-ahead count loads in
//! the background: resetting below the upstream drops pushed commits locally,
//! which Spur never force-pushes for the user.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Sizable as _};
use gpui_kit::{IntoElement, StatefulInteractiveElement as _};

use crate::git::ResetMode;
use crate::i18n::t;

/// Pushed-ahead count for the force-push warning row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PushedState {
    /// No upstream configured: nothing could be dropped from a remote.
    NoUpstream,
    /// Upstream known; the count is still loading.
    Checking,
    /// The count failed (unresolvable ref, failed command): warn unknown.
    Unknown,
    /// Commits the upstream holds that the target lacks.
    Ahead(u64),
}

/// A reset request while the dialog is open.
#[derive(Clone, Debug)]
pub(super) struct ResetRequest {
    pub repo_id: String,
    pub target: String,
    pub short: String,
    pub subject: String,
    /// Checked-out branch at open time (`None` = detached); rechecked.
    pub branch: Option<String>,
    /// HEAD at open time; rechecked.
    pub head: String,
    pub mode: ResetMode,
    pub pushed: PushedState,
    /// True while the queued reset runs; the dialog stays open with a
    /// progress bar until the operation completes.
    pub busy: bool,
}

impl SpurShell {
    /// Open the reset dialog for one commit. Refuses with a log line when
    /// there is no snapshot, no HEAD, or no commit yet.
    pub(super) fn request_reset_to(
        &mut self,
        hash: String,
        short: String,
        subject: String,
        cx: &mut Context<Self>,
    ) {
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        let Some(snapshot) = self.active_snapshot().map(|collected| collected.snapshot.clone())
        else {
            self.ops
                .push_info(t().log_action_failed("reset", "status not collected yet"));
            cx.notify();
            return;
        };
        if snapshot.unborn {
            self.ops
                .push_info(t().log_action_failed("reset", "no commits yet"));
            cx.notify();
            return;
        }
        let Some(head) = snapshot.head_oid.clone() else {
            self.ops
                .push_info(t().log_action_failed("reset", "status not collected yet"));
            cx.notify();
            return;
        };
        let Some(row) = self.overview.row(&repo_id) else {
            return;
        };
        let worktree = row.path.to_string_lossy().into_owned();
        let branch = snapshot.branch.clone();
        let upstream = snapshot.upstream.clone();
        let pushed = if upstream.is_none() {
            PushedState::NoUpstream
        } else {
            PushedState::Checking
        };
        self.reset_request = Some(ResetRequest {
            repo_id: repo_id.clone(),
            target: hash.clone(),
            short,
            subject,
            branch,
            head,
            mode: ResetMode::Mixed,
            pushed,
            busy: false,
        });
        self.open_modal(cx);
        // The pushed-ahead count needs Git: load it in the background and
        // fill the warning row when it lands (stale dialogs are ignored).
        if let Some(upstream) = upstream {
            let target = hash;
            let target_guard = target.clone();
            cx.spawn(async move |this, cx| {
                let count = cx
                    .background_executor()
                    .spawn(async move {
                        crate::git::pushed_ahead_count(&worktree, &target, &upstream)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if let Some(request) = this.reset_request.as_mut()
                        && request.repo_id == repo_id
                        && request.target == target_guard
                    {
                        request.pushed = match count {
                            Ok(n) => PushedState::Ahead(n),
                            Err(_) => PushedState::Unknown,
                        };
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// Close unless a reset is running: the scrim must not dismiss a busy
    /// dialog (its completion closes the modal, which would then hit
    /// whatever dialog opened meanwhile).
    pub(super) fn cancel_reset(&mut self, cx: &mut Context<Self>) {
        if self.reset_request.as_ref().is_some_and(|request| !request.busy) {
            self.close_modal(cx);
        }
    }

    /// Confirmed: queue the explicit reset with its captured identity. The
    /// dialog stays open (busy) until the operation completes.
    pub(super) fn confirm_reset(&mut self, cx: &mut Context<Self>) {
        let Some(mut request) = self.reset_request.clone() else {
            return;
        };
        if request.busy {
            return;
        }
        request.busy = true;
        self.reset_request = Some(request.clone());
        self.change_ops
            .push_back(changes::ChangeOp::Reset(changes::ResetOp {
                repo_id: request.repo_id,
                target: request.target,
                short: request.short,
                mode: request.mode,
                branch: request.branch,
                head: request.head,
            }));
        self.pump_change_ops(cx);
    }

    /// Full-window scrim + card shown while the dialog is open.
    pub(super) fn render_reset_dialog(
        &self,
        request: ResetRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let branch_label = request.branch.clone().unwrap_or_else(|| "HEAD".to_string());
        let modes = [
            (
                ResetMode::Soft,
                t().reset_mode_soft(),
                t().reset_mode_soft_hint(),
                false,
            ),
            (
                ResetMode::Mixed,
                t().reset_mode_mixed(),
                t().reset_mode_mixed_hint(),
                false,
            ),
            (
                ResetMode::Hard,
                t().reset_mode_hard(),
                t().reset_mode_hard_hint(),
                true,
            ),
        ];
        let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
        for (ix, (mode, label, hint, danger)) in modes.into_iter().enumerate() {
            let selected = request.mode == mode;
            let entity = cx.entity().downgrade();
            // A selected hard reset reads danger-tinted: the row keeps the
            // consequence visible after the click.
            let row_bg = if selected && danger {
                cx.theme().danger.alpha(0.10)
            } else if selected {
                ink(0.08)
            } else {
                ink(0.0)
            };
            let hint_color = if selected && danger {
                cx.theme().danger
            } else {
                text_faint(cx)
            };
            rows.push(
                div()
                    .id(("reset-mode", ix))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(CONTROL_RADIUS))
                    .bg(row_bg)
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
                                    .text_color(hint_color)
                                    .child(hint),
                            ),
                    )
                    .on_click(move |_, _, cx| {
                        entity
                            .update(cx, |this, cx| {
                                if let Some(request) = this.reset_request.as_mut()
                                    && !request.busy
                                {
                                    request.mode = mode;
                                }
                                cx.notify();
                            })
                            .ok();
                    })
                    .into_any_element(),
            );
        }

        // The force-push warning: only pushed commits matter (local-only
        // drops need no warning beyond the mode rows).
        let warning: Option<gpui_kit::AnyElement> = match request.pushed {
            PushedState::Ahead(0) | PushedState::NoUpstream | PushedState::Checking => None,
            PushedState::Ahead(n) => Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(13.))
                            .text_color(cx.theme().warning),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_SM))
                            .text_color(cx.theme().warning)
                            .child(t().reset_pushed(n)),
                    )
                    .into_any_element(),
            ),
            PushedState::Unknown => Some(
                div()
                    .text_size(px(TEXT_SM))
                    .text_color(text_faint(cx))
                    .child(t().reset_pushed_unknown())
                    .into_any_element(),
            ),
        };

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
                cx.listener(|this, _, _, cx| this.cancel_reset(cx)),
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
                                Icon::new(IconName::Rewind)
                                    .size(px(16.))
                                    .text_color(violet(cx)),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_LG))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(text_primary(cx))
                                    .child(t().reset_title(
                                        &branch_label,
                                        &request.short,
                                        &request.subject,
                                    )),
                            ),
                    )
                    .children(rows)
                    .children(warning)
                    .child(
                        div()
                            .text_size(px(TEXT_SM))
                            .text_color(text_faint(cx))
                            .child(t().reset_undo_line()),
                    )
                    .children(request.busy.then(|| {
                        busy_bar(
                            t().reset_running(&branch_label, &request.short),
                            false,
                            cx,
                        )
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("reset-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .disabled(request.busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_reset(cx))),
                            )
                            .child(
                                Button::new("reset-confirm")
                                    .label(t().reset_confirm())
                                    .small()
                                    .primary()
                                    .disabled(request.busy)
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| this.confirm_reset(cx))),
                            ),
                    ),
            )
            .into_any_element()
    }
}
