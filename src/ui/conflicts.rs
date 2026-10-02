//! Merge-conflict resolver: the pane shown instead of the diff when the
//! selected Local Changes row is unmerged. The worktree file's conflict
//! markers are parsed into common/ours/theirs sections; each conflict gets an
//! Ours / Theirs / Both choice, and resolving rewrites the file without
//! markers and stages it (which is what marks the path resolved in Git).

use super::*;

use std::rc::Rc;

use gpui_kit::base::InteractiveElementExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme, Disableable as _, Sizable as _};
use gpui_kit::{
    App, Hsla, InteractiveElement as _, IntoElement, StatefulInteractiveElement as _,
};

use crate::git::{ConflictChoice, ConflictSection};
use crate::i18n::t;

use super::changes::Selection;

/// Conflicted-line row height (denser than the diff's; conflict files are
/// short).
const CONFLICT_LINE_H: f32 = 17.0;
/// Never build a horizontally scrollable strip wider than this.
const MAX_CONFLICT_WIDTH: f32 = 6000.0;

/// Resolver state for one unmerged path.
pub(super) struct ConflictView {
    pub path: Vec<u8>,
    pub display: String,
    pub loading: bool,
    pub error: Option<String>,
    pub sections: Rc<Vec<ConflictSection>>,
    /// One slot per conflict section (`None` = no choice made yet).
    pub choices: Vec<Option<ConflictChoice>>,
    pub scroll: ScrollHandle,
    pub h_scroll: ScrollHandle,
}

impl ConflictView {
    fn conflict_total(&self) -> usize {
        crate::git::conflict_count(&self.sections)
    }

    fn chosen(&self) -> usize {
        self.choices.iter().filter(|choice| choice.is_some()).count()
    }
}

impl SpurShell {
    /// Open (or reload) the resolver for the selected unmerged path.
    pub(super) fn open_conflict_view(&mut self, selection: Selection, cx: &mut Context<Self>) {
        if self.conflict_view.as_ref().is_some_and(|view| {
            view.path == selection.path && !view.loading && view.error.is_none()
        }) {
            return;
        }
        let Some(repo) = self.active_repo() else {
            return;
        };
        let worktree = repo.path.to_string_lossy().into_owned();
        // The regular diff state must not linger behind the resolver.
        self.diff = diff::DiffState::Empty;
        self.diff_loading = false;
        self.conflict_view = Some(ConflictView {
            path: selection.path.clone(),
            display: selection.display.clone(),
            loading: true,
            error: None,
            sections: Rc::new(Vec::new()),
            choices: Vec::new(),
            scroll: ScrollHandle::new(),
            h_scroll: ScrollHandle::new(),
        });
        self.conflict_gen = self.conflict_gen.wrapping_add(1);
        let generation = self.conflict_gen;
        let path = selection.path;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let load_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { crate::git::conflict_sections(&worktree, &load_path) })
                .await;
            this.update(cx, |this, cx| {
                if this.conflict_gen != generation {
                    return; // a newer selection owns the pane
                }
                if let Some(view) = this.conflict_view.as_mut() {
                    view.loading = false;
                    match result {
                        Ok(sections) => {
                            view.choices = vec![None; crate::git::conflict_count(&sections)];
                            view.sections = Rc::new(sections);
                        }
                        Err(err) => view.error = Some(err),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// One conflict's choice (Ours / Theirs / Both).
    pub(super) fn set_conflict_choice(
        &mut self,
        index: usize,
        choice: ConflictChoice,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self.conflict_view.as_mut()
            && let Some(slot) = view.choices.get_mut(index)
        {
            *slot = Some(choice);
            cx.notify();
        }
    }

    /// Queue the resolution; every conflict must have a choice.
    pub(super) fn confirm_conflict(&mut self, cx: &mut Context<Self>) {
        let Some(view) = &self.conflict_view else {
            return;
        };
        if view.loading || view.error.is_some() || view.choices.is_empty() {
            return;
        }
        if view.choices.iter().any(|choice| choice.is_none()) {
            return;
        }
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        let choices: Vec<ConflictChoice> = view.choices.iter().flatten().copied().collect();
        let path = view.path.clone();
        self.change_ops
            .push_back(changes::ChangeOp::Resolve(changes::ResolveConflictOp {
                repo_id,
                path,
                choices,
            }));
        self.pump_change_ops(cx);
    }

    /// The resolver pane (replaces the diff pane for unmerged rows).
    pub(super) fn render_conflict_pane(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let Some(view) = &self.conflict_view else {
            return div().into_any_element();
        };
        let total = view.conflict_total();
        let chosen = view.chosen();
        let can_apply = !view.loading && view.error.is_none() && total > 0 && chosen == total;

        let header = div()
            .flex_none()
            .h(px(HEADER_H))
            .min_w_0()
            .overflow_hidden()
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(hairline(0.05))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(MONO)
                    .text_size(px(TEXT_SM))
                    .text_color(text_primary(cx))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(truncate_path(&view.display, 72)),
            )
            .child(chip(t().conflict_chip, cx.theme().warning));

        // The action strip sits at the pane's left edge, so it stays visible
        // even when the pane is wider than the window.
        let mut footer = div()
            .flex_none()
            .min_w_0()
            .overflow_hidden()
            .px(px(12.))
            .py(px(8.))
            .flex()
            .items_center()
            .gap(px(10.))
            .border_t_1()
            .border_color(hairline(0.05));
        let mut apply = Button::new("conflict-apply")
            .label(t().conflict_apply)
            .small()
            .primary();
        if can_apply {
            let this = cx.entity().downgrade();
            apply = apply.cursor_pointer().on_click(move |_, _, cx| {
                this.update(cx, |this, cx| this.confirm_conflict(cx)).ok();
            });
        } else {
            apply = apply.disabled(true);
        }
        footer = footer.child(apply);
        if total > 0 {
            footer = footer.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(TEXT_XS))
                    .text_color(text_muted(cx))
                    .child(t().conflict_progress(chosen, total)),
            );
        }

        let body: gpui_kit::AnyElement = if view.loading {
            conflict_note(t().conflict_loading, cx)
        } else if let Some(err) = &view.error {
            conflict_error_note(err, cx)
        } else if total == 0 {
            conflict_note(t().conflict_no_markers, cx)
        } else {
            self.conflict_body(view, cx)
        };

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex_col()
            .child(header)
            .child(body)
            .child(footer)
            .into_any_element()
    }

    fn conflict_body(&self, view: &ConflictView, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let this = cx.entity().downgrade();
        let mut children: Vec<gpui_kit::AnyElement> = Vec::new();
        let mut conflict_ix = 0usize;
        let total = view.conflict_total();
        for section in view.sections.iter() {
            match section {
                ConflictSection::Common(bytes) => {
                    let lines = ConflictSection::lines(bytes);
                    children.push(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .py(px(2.))
                            .children(
                                lines
                                    .into_iter()
                                    .map(|line| common_line(&line, cx)),
                            )
                            .into_any_element(),
                    );
                }
                ConflictSection::Conflict {
                    ours_label,
                    theirs_label,
                    ours,
                    theirs,
                } => {
                    let ours_lines = ConflictSection::lines(ours);
                    let theirs_lines = ConflictSection::lines(theirs);
                    let col_chars = ours_lines
                        .iter()
                        .chain(theirs_lines.iter())
                        .map(|line| line.chars().count())
                        .max()
                        .unwrap_or(0);
                    let col_w = (col_chars as f32 * MONO_CHAR_W + 24.0).clamp(140.0, 3000.0);
                    let choice = view.choices.get(conflict_ix).copied().flatten();
                    children.push(conflict_card(
                        conflict_ix,
                        total,
                        ours_label,
                        theirs_label,
                        ours_lines,
                        theirs_lines,
                        col_w,
                        choice,
                        this.clone(),
                        cx,
                    ));
                    conflict_ix += 1;
                }
            }
        }

        let content_chars = view
            .sections
            .iter()
            .map(|section| match section {
                ConflictSection::Common(bytes) => ConflictSection::lines(bytes)
                    .iter()
                    .map(|line| line.chars().count())
                    .max()
                    .unwrap_or(0),
                ConflictSection::Conflict { ours, theirs, .. } => {
                    let side = ConflictSection::lines(ours)
                        .iter()
                        .chain(ConflictSection::lines(theirs).iter())
                        .map(|line| line.chars().count())
                        .max()
                        .unwrap_or(0);
                    side * 2 + 2
                }
            })
            .max()
            .unwrap_or(0);
        let content_w =
            (96.0 + content_chars as f32 * MONO_CHAR_W).clamp(360.0, MAX_CONFLICT_WIDTH);

        div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(
                // Horizontal outside, vertical inside — the proven diff-pane
                // nesting: the inner scroller gets a definite width and the
                // cards fill it instead of being sized by their min-content.
                div()
                    .id("conflict-h")
                    .size_full()
                    .min_w_0()
                    .overflow_x_scroll()
                    .lock_scroll_axis()
                    .track_scroll(&view.h_scroll)
                    .child(
                        div()
                            .id("conflict-v")
                            .w_full()
                            .min_w(px(content_w))
                            .h_full()
                            .overflow_y_scroll()
                            .track_scroll(&view.scroll)
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .children(children),
                            ),
                    ),
            )
            .child(scrollbar_overlay("conflict-scrollbar", &view.scroll))
            .child(h_scrollbar_overlay("conflict-h-scrollbar", &view.h_scroll))
            .into_any_element()
    }
}

/// One mono line of a common (unconflicted) region.
fn common_line(text: &str, cx: &App) -> gpui_kit::AnyElement {
    div()
        .flex_none()
        .h(px(CONFLICT_LINE_H))
        .px(px(8.))
        .flex()
        .items_center()
        .font_family(MONO)
        .text_size(px(TEXT_XS))
        .text_color(text_faint(cx))
        .whitespace_nowrap()
        .child(text.to_string())
        .into_any_element()
}

/// One mono line inside an ours/theirs cell of a fixed width.
fn side_line(text: &str, width: f32, color: Hsla, bg: Hsla) -> gpui_kit::AnyElement {
    div()
        .w(px(width))
        .flex_none()
        .h(px(CONFLICT_LINE_H))
        .px(px(8.))
        .flex()
        .items_center()
        .bg(bg)
        .font_family(MONO)
        .text_size(px(TEXT_XS))
        .text_color(color)
        .whitespace_nowrap()
        .overflow_hidden()
        .child(text.to_string())
        .into_any_element()
}

/// One conflict's card: label + choice buttons, then the ours/theirs columns.
#[allow(clippy::too_many_arguments)]
fn conflict_card(
    index: usize,
    total: usize,
    ours_label: &str,
    theirs_label: &str,
    ours_lines: Vec<String>,
    theirs_lines: Vec<String>,
    col_w: f32,
    choice: Option<ConflictChoice>,
    this: gpui_kit::WeakEntity<SpurShell>,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    let mut choice_row = div().flex().items_center().gap(px(4.)).flex_none();
    for (choice_value, label) in [
        (ConflictChoice::Ours, t().conflict_ours),
        (ConflictChoice::Theirs, t().conflict_theirs),
        (ConflictChoice::Both, t().conflict_both),
    ] {
        let selected = choice == Some(choice_value);
        let id = match choice_value {
            ConflictChoice::Ours => ("conflict-ours", index),
            ConflictChoice::Theirs => ("conflict-theirs", index),
            ConflictChoice::Both => ("conflict-both", index),
        };
        let click_entity = this.clone();
        choice_row = choice_row.child(
            div()
                .id(id)
                .cursor_pointer()
                .h(px(20.))
                .px(px(9.))
                .flex()
                .items_center()
                .rounded(px(CONTROL_RADIUS))
                .bg(if selected {
                    violet(cx).alpha(0.18)
                } else {
                    ink(0.05)
                })
                .border_1()
                .border_color(if selected {
                    violet(cx).alpha(0.60)
                } else {
                    hairline(0.08)
                })
                .text_size(px(TEXT_XS))
                .text_color(if selected {
                    text_primary(cx)
                } else {
                    text_muted(cx)
                })
                .child(label.to_string())
                .on_click(move |_, _, cx| {
                    click_entity
                        .update(cx, |this, cx| {
                            this.set_conflict_choice(index, choice_value, cx)
                        })
                        .ok();
                }),
        );
    }

    let chosen_bg = |side: ConflictChoice| {
        if choice == Some(side) {
            violet(cx).alpha(0.08)
        } else {
            ink(0.0)
        }
    };
    let mut grid = div().flex().flex_col().w_full().child(
        div()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(hairline(0.05))
            .child(side_line(
                &t().conflict_ours_label(ours_label),
                col_w,
                text_faint(cx),
                ink(0.02),
            ))
            .child(div().w(px(1.)).flex_none().bg(hairline(0.05)))
            .child(side_line(
                &t().conflict_theirs_label(theirs_label),
                col_w,
                text_faint(cx),
                ink(0.02),
            )),
    );
    let rows = ours_lines.len().max(theirs_lines.len());
    for row in 0..rows {
        let ours = ours_lines.get(row).map(String::as_str).unwrap_or("");
        let theirs = theirs_lines.get(row).map(String::as_str).unwrap_or("");
        grid = grid.child(
            div()
                .flex()
                .items_center()
                .child(side_line(
                    ours,
                    col_w,
                    text_primary(cx),
                    chosen_bg(ConflictChoice::Ours),
                ))
                .child(div().w(px(1.)).flex_none().bg(hairline(0.05)))
                .child(side_line(
                    theirs,
                    col_w,
                    text_primary(cx),
                    chosen_bg(ConflictChoice::Theirs),
                )),
        );
    }

    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(CONTROL_RADIUS))
        .border_1()
        .border_color(hairline(0.10))
        .bg(ink(0.02))
        .overflow_hidden()
        .child(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(5.))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(TEXT_XS))
                        .font_weight(gpui_kit::FontWeight::MEDIUM)
                        .text_color(text_muted(cx))
                        .child(t().conflict_section(index + 1, total)),
                )
                .child(choice_row),
        )
        .child(grid)
        .into_any_element()
}

fn conflict_note(text: &str, cx: &App) -> gpui_kit::AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(TEXT_SM))
        .text_color(text_muted(cx))
        .child(text.to_string())
        .into_any_element()
}

fn conflict_error_note(err: &str, cx: &App) -> gpui_kit::AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(TEXT_SM))
                .text_color(cx.theme().danger)
                .child(t().conflict_error),
        )
        .child(
            div()
                .max_w(px(520.))
                .font_family(MONO)
                .text_size(px(TEXT_XS))
                .text_color(text_faint(cx))
                .child(err.to_string()),
        )
        .into_any_element()
}
