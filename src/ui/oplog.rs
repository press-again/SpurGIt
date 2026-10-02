//! Operation log panel: structured rows (time, repo, op, result) with
//! expandable command records (exact argv + capped output), the running
//! operation with Cancel, and the transient alert's Details destination.
//! Informational lines render as quiet rows; only ran operations expand.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Icon, Sizable as _};
use gpui_kit::{IntoElement, StatefulInteractiveElement as _};

use crate::i18n::t;
use crate::model::{OpEntry, OpResult};

gpui_kit::actions!(spur, [ToggleOpLog]);

/// The queued operation currently executing (set by the pump, cleared on
/// completion). Drives the panel's running row and its Cancel button.
#[derive(Clone, Debug)]
pub(super) struct RunningOp {
    pub repo: String,
    /// Op-log verb, display only.
    pub op: String,
    /// A fetch, pull or push: talks to a remote (the branchling's Focus).
    /// Typed here so rewording a log verb cannot silently change behavior.
    pub network: bool,
    pub started: std::time::Instant,
}

impl SpurShell {
    /// Open the panel, optionally expanded/highlighted at one entry (the
    /// alert's Details link). Newest first, so the entry is at the top.
    pub(super) fn open_oplog(&mut self, entry: Option<u64>, cx: &mut Context<Self>) {
        self.oplog_open = true;
        self.oplog_expanded = entry;
        self.oplog_highlight = entry;
        self.oplog_scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    pub(super) fn close_oplog(&mut self, cx: &mut Context<Self>) {
        if self.oplog_open {
            self.oplog_open = false;
            cx.notify();
        }
    }

    pub(super) fn toggle_oplog(&mut self, cx: &mut Context<Self>) {
        if self.oplog_open {
            self.close_oplog(cx);
        } else {
            self.open_oplog(None, cx);
        }
    }

    /// Cancel queued operations and ask the running process to terminate.
    /// The running op reports `Cancelled` (no error toast) when its death
    /// lands; queued work simply never runs.
    pub(super) fn cancel_change_ops(&mut self, cx: &mut Context<Self>) {
        let queued = self.change_ops.len();
        self.change_ops.clear();
        if queued > 0 {
            self.ops.push_info(t().log_cancel_queued(queued));
        }
        if self.change_busy {
            crate::process::cancel_running_command();
        }
        cx.notify();
    }

    /// Scrim-less popover anchored under the app bar on the right: a
    /// transparent click-catcher plus the card, so a click anywhere outside
    /// closes it without dimming the workspace.
    pub(super) fn render_oplog(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.close_oplog(cx)),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(px(TITLEBAR_H + 6.))
                    .right(px(12.))
                    .w(px(560.))
                    .max_h(px(480.))
                    .rounded(px(PANEL_RADIUS))
                    .border_1()
                    .border_color(hairline(0.10))
                    .bg(cx.theme().popover)
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .on_mouse_down(
                        gpui_kit::MouseButton::Left,
                        cx.listener(|_, _, _, cx| cx.stop_propagation()),
                    )
                    .child(self.oplog_header(cx))
                    .child(self.oplog_body(cx)),
            )
            .into_any_element()
    }

    fn oplog_header(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .h(px(36.))
            .border_b_1()
            .border_color(hairline(0.05))
            .child(
                div()
                    .flex_1()
                    .text_size(px(TEXT_MD))
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .text_color(text_primary(cx))
                    .child(t().oplog_title),
            )
            .child(
                div()
                    .id("oplog-close")
                    .aria_label(t().oplog_close)
                    .cursor_pointer()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(18.))
                    .h(px(18.))
                    .rounded(px(4.))
                    .bg(hover_blend("oplog-close", ink(0.0), ink(0.10)))
                    .on_hover(hover_listener("oplog-close"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_oplog(cx)))
                    .child(
                        Icon::new(IconName::Close)
                            .size(px(12.))
                            .text_color(text_muted(cx)),
                    ),
            )
            .into_any_element()
    }

    fn oplog_body(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let mut rows: Vec<gpui_kit::AnyElement> = Vec::new();
        if self.change_busy
            && let Some(running) = self.running_op.clone()
        {
            rows.push(self.oplog_running_row(&running, cx));
        }
        for entry in self.ops.log.iter().rev() {
            rows.push(self.oplog_row(entry, cx));
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .py(px(18.))
                    .flex()
                    .justify_center()
                    .text_size(px(TEXT_SM))
                    .text_color(text_muted(cx))
                    .child(t().oplog_empty)
                    .into_any_element(),
            );
        }
        div()
            .id("oplog-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.oplog_scroll)
            .py(px(6.))
            .flex()
            .flex_col()
            .children(rows)
            .into_any_element()
    }

    /// The executing operation: spinner, repo, verb, elapsed, Cancel.
    fn oplog_running_row(
        &self,
        running: &RunningOp,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let secs = running.started.elapsed().as_secs();
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(6.))
            .child(Spinner::new().with_size(px(12.)).color(violet(cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_family(MONO)
                    .text_size(px(TEXT_SM))
                    .text_color(text_primary(cx))
                    .child(format!(
                        "{} · {} · {}",
                        truncate_path(&running.repo, 28),
                        running.op,
                        t().oplog_elapsed(secs)
                    )),
            )
            .child(
                Button::new("oplog-cancel")
                    .label(t().oplog_cancel)
                    .small()
                    .outline()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_change_ops(cx))),
            )
            .into_any_element()
    }

    /// One log row: time, repo, verb, result mark; ran operations expand to
    /// their command records on click.
    fn oplog_row(&self, entry: &OpEntry, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let (mark, color) = match entry.result {
            OpResult::Success => ("✓", cx.theme().success),
            OpResult::Failed => ("✗", cx.theme().danger),
            OpResult::Cancelled => ("○", cx.theme().warning),
            OpResult::Info => ("·", text_faint(cx)),
        };
        let time = ago_label(entry.at.elapsed());
        let title = match &entry.repo {
            Some(repo) => format!("{} · {} · {}", time, repo, entry.op),
            None => entry.detail.clone(),
        };
        let id = entry.id;
        let expandable = !entry.commands.is_empty();
        let expanded = self.oplog_expanded == Some(id);
        let highlighted = self.oplog_highlight == Some(id);
        // The whole row toggles: the detail line below the title is inside
        // the click target, and a chevron marks expandable rows so quiet
        // info rows never look broken.
        let mut row = div()
            .id(("oplog-row", id))
            .flex()
            .flex_col()
            .px(px(12.))
            .py(px(5.))
            .gap(px(2.))
            .rounded(px(6.))
            .mx(px(6.));
        if highlighted {
            row = row.bg(violet(cx).alpha(0.10));
        }
        if expandable {
            row = row.cursor_pointer().on_click(cx.listener(
                move |this, _, _, cx| {
                    this.oplog_expanded =
                        if this.oplog_expanded == Some(id) { None } else { Some(id) };
                    this.oplog_highlight = None;
                    cx.notify();
                },
            ));
        }
        row = row
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(14.))
                            .flex_none()
                            .text_size(px(TEXT_SM))
                            .text_color(color)
                            .child(mark.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .font_family(MONO)
                            .text_size(px(TEXT_XS))
                            .text_color(if highlighted {
                                text_primary(cx)
                            } else {
                                text_muted(cx)
                            })
                            .child(title),
                    )
                    .children(expandable.then(|| {
                        div().flex_none().child(
                            Icon::new(if expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(px(12.))
                            .text_color(text_faint(cx)),
                        )
                    })),
            )
            .child(
                div()
                    .pl(px(22.))
                    .pr(px(4.))
                    .overflow_hidden()
                    .text_size(px(TEXT_SM))
                    .text_color(text_muted(cx))
                    .child(entry.detail.clone()),
            );
        if expanded {
            for command in &entry.commands {
                row = row.child(Self::oplog_command(command, cx));
            }
        }
        row.into_any_element()
    }

    /// One command record: the exact argv plus capped stdout/stderr.
    fn oplog_command(
        command: &crate::process::CommandRecord,
        cx: &Context<SpurShell>,
    ) -> gpui_kit::AnyElement {
        let mut body = format!("$ {}", command.argv.join(" "));
        if !command.stdout.trim().is_empty() {
            body.push('\n');
            body.push_str(command.stdout.trim_end());
        }
        if !command.stderr.trim().is_empty() {
            body.push('\n');
            body.push_str(command.stderr.trim_end());
        }
        if command.truncated {
            body.push('\n');
            body.push_str(t().oplog_truncated);
        }
        div()
            .mt(px(4.))
            .p(px(8.))
            .rounded(px(6.))
            .bg(ink(0.04))
            .font_family(MONO)
            .text_size(px(TEXT_XS))
            .text_color(text_muted(cx))
            .child(body)
            .into_any_element()
    }
}
