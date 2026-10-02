//! Stash dialog (context menu): SourceGit's three "changes after stashing"
//! modes (`DealWithChangesAfterStashing`) plus an optional description.
//! The Stashes page itself lives here too: a selectable stash list, the
//! changed files of the selected entry, and a read-only diff beside them.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Sizable as _};
use gpui_kit::{IntoElement, StatefulInteractiveElement as _};

use crate::git::StashMode;
use crate::i18n::t;

use super::changes::{ChangeOpKind, ChangeTarget};

/// Left column width of the Stashes page (list + files).
const STASH_LIST_W: f32 = 340.0;
/// One stash entry row.
const STASH_ROW_H: f32 = 36.0;
/// One changed-file row of the selected stash.
const STASH_FILE_ROW_H: f32 = 28.0;

/// A stash request while the dialog is open.
#[derive(Clone, Debug)]
pub(super) struct StashRequest {
    pub repo_id: String,
    pub targets: Vec<ChangeTarget>,
    pub display: String,
    pub mode: StashMode,
}

/// A branch-from-stash request while the dialog is open. The branch name
/// lives in [`SpurShell::stash_branch_input`].
#[derive(Clone, Debug)]
pub(super) struct StashBranchRequest {
    pub repo_id: String,
    pub id: String,
}

impl StashRequest {
    /// Any untracked target makes the stash include untracked content.
    fn untracked(&self) -> bool {
        self.targets.iter().any(|target| target.untracked)
    }
}

impl SpurShell {
    /// Open the stash dialog for the selected paths and pre-fill the
    /// description.
    pub(super) fn request_stash(
        &mut self,
        repo_id: String,
        targets: Vec<ChangeTarget>,
        display: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if targets.is_empty() {
            return;
        }
        let default = if targets.len() == 1 {
            t().stash_message(&display)
        } else {
            t().stash_message_files(targets.len())
        };
        self.stash_message_input
            .update(cx, |state, cx| state.set_value(default.as_str(), window, cx));
        self.stash_request = Some(StashRequest {
            repo_id,
            targets,
            display,
            mode: self.stash_mode,
        });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_stash(&mut self, cx: &mut Context<Self>) {
        if self.stash_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed stash: remembers the chosen mode and queues the operation.
    pub(super) fn confirm_stash(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.stash_request.clone() else {
            return;
        };
        self.stash_mode = request.mode;
        let message = self.stash_message_input.read(cx).value().trim().to_string();
        let untracked = request.untracked();
        let paths = request
            .targets
            .iter()
            .map(|target| target.path.clone())
            .collect();
        self.queue_change_op(
            request.repo_id,
            paths,
            ChangeOpKind::Stash {
                untracked,
                mode: request.mode,
            },
            Some(message),
            cx,
        );
        self.close_modal(cx);
    }

    pub(super) fn render_stash_dialog(
        &self,
        request: StashRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let this = cx.entity().downgrade();
        let options = [
            (
                StashMode::Discard,
                t().stash_mode_discard,
                t().stash_mode_discard_hint,
            ),
            (
                StashMode::KeepIndex,
                t().stash_mode_keep_index,
                t().stash_mode_keep_index_hint,
            ),
            (
                StashMode::KeepAll,
                t().stash_mode_keep_all,
                t().stash_mode_keep_all_hint,
            ),
        ];
        let mut modes: Vec<gpui_kit::AnyElement> = Vec::new();
        for (ix, (mode, label, hint)) in options.into_iter().enumerate() {
            let selected = request.mode == mode;
            let entity = this.clone();
            modes.push(
                div()
                    .id(("stash-mode", ix))
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
                                if let Some(request) = this.stash_request.as_mut() {
                                    request.mode = mode;
                                }
                                this.stash_mode = mode;
                                cx.notify();
                            })
                            .ok();
                    })
                    .into_any_element(),
            );
        }

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
                cx.listener(|this, _, _, cx| this.cancel_stash(cx)),
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
                                Icon::new(IconName::Archive)
                                    .size(px(16.))
                                    .text_color(violet(cx)),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_LG))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(text_primary(cx))
                                    .child(t().stash_title),
                            ),
                    )
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(TEXT_SM))
                            .text_color(text_muted(cx))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(request.display.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().stash_mode),
                    )
                    .children(modes)
                    .child(
                        div()
                            .pt(px(2.))
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().stash_description),
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
                                Input::new(&self.stash_message_input)
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
                                Button::new("stash-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_stash(cx)),
                                    ),
                            )
                            .child(
                                Button::new("stash-confirm")
                                    .label(t().stash_confirm)
                                    .small()
                                    .primary()
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.confirm_stash(cx)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

// ---- Stashes page: selectable entries, changed files, read-only diff ----

/// What one immutable stash object remembers across reselections: the last
/// opened file and the list/diff scroll offsets.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct StashMemory {
    pub file: usize,
    pub files_offset: f32,
    pub diff_offset: f32,
}

impl SpurShell {
    /// Select a stash entry and load its changed files. The stash commit's
    /// object id is resolved first and used for every read and cache key: a
    /// stale inspector label (`stash@{n}`) must never make a read resolve to
    /// a different entry, and an immutable object is served from cache.
    /// Both requests are invalidated at this boundary so an old stash's
    /// pending reply cannot land under the new selection.
    pub(super) fn select_stash(&mut self, id: String, cx: &mut Context<Self>) {
        if self.selected_stash.as_deref() == Some(id.as_str())
            && self.stash_files_for.as_deref() == Some(id.as_str())
            && self.selected_stash_object.is_some()
            && self.stash_files_error.is_none()
        {
            return;
        }
        let Some(object) = self.stash_object(&id) else {
            // The list no longer knows this label; reading `stash@{n}` would
            // be a different entry.
            self.reset_stash_view();
            cx.notify();
            return;
        };
        self.remember_stash_view();
        // Invalidate the previous stash's file and diff requests immediately:
        // neither may land while this selection loads or after it failed.
        self.stash_files_gen = self.stash_files_gen.wrapping_add(1);
        self.stash_diff_gen = self.stash_diff_gen.wrapping_add(1);
        self.selected_stash = Some(id.clone());
        self.selected_stash_object = Some(object.clone());
        self.stash_file_selected = None;
        self.stash_diff = commit_detail::FileDiffState::Empty;
        let cached = self.active_repo().map(|repo| repo.path.to_string_lossy().into_owned())
            .and_then(|worktree| {
                self.stash_files_cache
                    .get(&(worktree, object.clone()))
                    .cloned()
            });
        if let Some(changes) = cached {
            // Invalidate any in-flight load; this selection is already served
            // by the cache's own immutable model (no deep copy).
            self.stash_files_gen = self.stash_files_gen.wrapping_add(1);
            self.stash_files_for = Some(id);
            self.stash_files_loading = false;
            self.stash_files_error = None;
            self.stash_changes = Some(changes);
            self.restore_stash_scrolls(&object);
            if let Some(changes) = &self.stash_changes
                && !changes.files.is_empty()
            {
                let ix = self.stash_memory(&object).file.min(changes.files.len() - 1);
                self.open_stash_file(ix, cx);
            }
            cx.notify();
            return;
        }
        self.load_stash_files(id, object, cx);
    }

    fn stash_memory(&self, object: &str) -> StashMemory {
        self.stash_view_memory
            .get(object)
            .copied()
            .unwrap_or_default()
    }

    /// Capture the visible scroll offsets for the stash being left.
    fn remember_stash_view(&mut self) {
        let Some(object) = self.selected_stash_object.clone() else {
            return;
        };
        let files_offset = self
            .stash_files_scroll
            .0
            .borrow()
            .base_handle
            .offset()
            .y
            .into();
        let diff_offset = self
            .stash_diff_scroll
            .0
            .borrow()
            .base_handle
            .offset()
            .y
            .into();
        let file = self.stash_file_selected.unwrap_or(0);
        // Bounded per-session memory for immutable stash objects.
        if self.stash_view_memory.len() > 256 {
            self.stash_view_memory.clear();
        }
        self.stash_view_memory.insert(
            object,
            StashMemory {
                file,
                files_offset,
                diff_offset,
            },
        );
    }

    /// Fresh handles for a newly selected stash, positioned at the offsets
    /// remembered for its immutable object.
    fn restore_stash_scrolls(&mut self, object: &str) {
        let memory = self.stash_memory(object);
        self.stash_files_scroll = UniformListScrollHandle::new();
        self.stash_diff_scroll = UniformListScrollHandle::new();
        self.stash_diff_h_scroll = ScrollHandle::new();
        self.stash_files_scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(memory.files_offset)));
        self.stash_diff_scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(memory.diff_offset)));
    }

    /// FIFO-capped bookkeeping for immutable stash file lists.
    fn cache_stash_files(&mut self, key: (String, String), changes: Rc<crate::git::StashChanges>) {
        self.stash_files_cache.insert(key.clone(), changes);
        self.stash_files_cache_order.retain(|cached| cached != &key);
        self.stash_files_cache_order.push_back(key);
        while self.stash_files_cache_order.len() > STASH_FILES_CACHE_MAX {
            if let Some(oldest) = self.stash_files_cache_order.pop_front() {
                self.stash_files_cache.remove(&oldest);
            }
        }
    }

    fn load_stash_files(&mut self, id: String, object: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let worktree = repo.path.to_string_lossy().into_owned();
        self.stash_files_gen = self.stash_files_gen.wrapping_add(1);
        let generation = self.stash_files_gen;
        self.stash_files_for = Some(id.clone());
        self.stash_files_loading = true;
        self.stash_files_error = None;
        self.stash_changes = None;
        // Fresh handles positioned from this object's remembered offsets: a
        // reused handle still reports the previous stash's content/bounds for
        // the first frame, which made the always-visible scrollbar flash.
        self.restore_stash_scrolls(&object);
        cx.notify();
        let worktree_key = worktree.clone();
        let cache_object = object.clone();
        let check_object = object.clone();
        cx.spawn(async move |this, cx| {
            let log_id = id.clone();
            let load_object = object;
            let result = cx
                .background_executor()
                .spawn(async move { crate::git::stash_changes(&worktree, &load_object) })
                .await;
            this.update(cx, |this, cx| {
                if this.stash_files_gen != generation {
                    return; // a newer stash owns the list
                }
                // Completion must still describe the selected immutable
                // object and label, not merely a matching generation.
                if this.selected_stash_object.as_deref() != Some(check_object.as_str())
                    || this.stash_files_for.as_deref() != Some(log_id.as_str())
                {
                    return;
                }
                this.stash_files_loading = false;
                match result {
                    Ok(changes) => {
                        let changes = Rc::new(changes);
                        // The view and the cache share ONE immutable model;
                        // eviction cannot invalidate the visible rows.
                        this.stash_changes = Some(changes.clone());
                        this.cache_stash_files((worktree_key.clone(), cache_object.clone()), changes);
                        // Show the remembered file's diff right away:
                        // selecting a stash must not leave the pane on the
                        // "select a file" note.
                        if let Some(changes) = &this.stash_changes
                            && !changes.files.is_empty()
                        {
                            let ix = this
                                .stash_memory(&cache_object)
                                .file
                                .min(changes.files.len() - 1);
                            this.open_stash_file(ix, cx);
                        }
                    }
                    Err(err) => {
                        crate::logging::log!("stash: {log_id}: {err}");
                        this.stash_files_error = Some(err);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Single click on a stashed file loads its read-only diff.
    pub(super) fn open_stash_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(object) = self.selected_stash_object.clone() else {
            return;
        };
        let Some(file) = self
            .stash_changes
            .as_ref()
            .and_then(|changes| changes.files.get(ix))
            .cloned()
        else {
            return;
        };
        // Reopening the already-visible file keeps its scroll position and
        // avoids a needless loading flash.
        if self.stash_file_selected == Some(ix)
            && matches!(self.stash_diff, commit_detail::FileDiffState::Ready(_))
        {
            return;
        }
        let Some(repo) = self.active_repo() else {
            return;
        };
        let worktree = repo.path.to_string_lossy().into_owned();
        let untracked = self
            .stash_changes
            .as_ref()
            .is_some_and(|changes| changes.untracked.iter().any(|path| path == &file.path));
        // Restore this object's saved diff offset only when its remembered
        // file is reopened; a different file starts at the top.
        let keep_offset = self.stash_memory(&object).file == ix;
        let remembered_offset = self.stash_memory(&object).diff_offset;
        self.stash_file_selected = Some(ix);
        self.stash_view_memory
            .entry(object.clone())
            .or_default()
            .file = ix;
        self.stash_diff_gen = self.stash_diff_gen.wrapping_add(1);
        let generation = self.stash_diff_gen;
        // Immutable object pair (stash commit + path): cached previews are
        // served without a Git read, and the read itself uses the same object
        // the file list came from.
        let key = super::diff::DiffKey::Object {
            worktree: worktree.clone(),
            object: object.clone(),
            path: file.path.clone(),
        };
        if let Some(cached) = self.diff_cache.get(&key).cloned() {
            self.stash_diff = commit_detail::FileDiffState::Ready(cached);
            self.new_stash_diff_scrolls(if keep_offset { remembered_offset } else { 0.0 });
            cx.notify();
            return;
        }
        self.stash_diff = commit_detail::FileDiffState::Loading;
        // Another file starts at the top, with no stale handle geometry.
        self.new_stash_diff_scrolls(if keep_offset { remembered_offset } else { 0.0 });
        cx.notify();
        let check_object = object.clone();
        let check_ix = ix;
        let check_path = file.path.clone();
        cx.spawn(async move |this, cx| {
            let load_object = object;
            let result = cx
                .background_executor()
                .spawn(async move {
                    crate::git::stash_file_diff(&worktree, &load_object, &file.path, untracked)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.stash_diff_gen != generation {
                    return;
                }
                // A late diff must still belong to the selected object, the
                // selected row, and the same path.
                if this.selected_stash_object.as_deref() != Some(check_object.as_str())
                    || this.stash_file_selected != Some(check_ix)
                    || this
                        .stash_changes
                        .as_ref()
                        .and_then(|changes| changes.files.get(check_ix))
                        .map(|file| file.path.clone())
                        != Some(check_path)
                {
                    return;
                }
                this.stash_diff = match result {
                    Ok(diff) => {
                        let diff = Rc::new(diff);
                        this.cache_diff(key, diff.clone());
                        commit_detail::FileDiffState::Ready(diff)
                    }
                    Err(err) => commit_detail::FileDiffState::Error(err),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Fresh diff scroll handles; the vertical list starts at `offset`.
    fn new_stash_diff_scrolls(&mut self, offset: f32) {
        self.stash_diff_scroll = UniformListScrollHandle::new();
        self.stash_diff_h_scroll = ScrollHandle::new();
        if offset != 0.0 {
            self.stash_diff_scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.), px(offset)));
        }
    }

    /// Drop the stash view state (repository switch, entry disappeared).
    pub(super) fn reset_stash_view(&mut self) {
        self.stash_files_gen = self.stash_files_gen.wrapping_add(1);
        self.stash_diff_gen = self.stash_diff_gen.wrapping_add(1);
        self.selected_stash = None;
        self.selected_stash_object = None;
        self.stash_files_for = None;
        self.stash_files_loading = false;
        self.stash_files_error = None;
        self.stash_changes = None;
        self.stash_file_selected = None;
        self.stash_diff = commit_detail::FileDiffState::Empty;
    }

    /// Stashes page: selectable stash entries, their changed files, and the
    /// read-only diff of the clicked file (SourceGit layout).
    pub(super) fn section_stashes(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // The details cache owns the immutable entry list; rendering shares
        // it instead of cloning every id/object/message.
        let stashes = self
            .active_details()
            .map(|details| details.stashes.clone())
            .unwrap_or_default();
        let count = stashes.len();
        let selected = self.selected_stash.clone();
        let this = cx.entity().downgrade();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(self.section_strip(
                div().child(t().stashes_count(count)),
                self.live_note(cx),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(STASH_LIST_W))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .min_h_0()
                            .border_r_1()
                            .border_color(hairline(0.05))
                            .child(
                                // The scrollbar overlay must stay OUTSIDE the
                                // scroller: as a child, its absolute bounds
                                // are counted in the scrollable content and a
                                // nearly-full scrollbar shows even with space
                                // to spare.
                                div()
                                    .relative()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .child(scrollbar_overlay(
                                        "stashes-scrollbar",
                                        &self.stashes_scroll,
                                    ))
                                    .child(if count == 0 {
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .px(px(6.))
                                            .py(px(4.))
                                            .child(empty_note(t().no_stashes, cx))
                                            .into_any_element()
                                    } else {
                                        // Virtualized: only the visible stash
                                        // entries are built (repositories can
                                        // hold thousands). The closure keeps
                                        // the shared list alive.
                                        let this = this.clone();
                                        gpui_kit::uniform_list(
                                            "stash-list",
                                            count,
                                            move |range, _window, cx| {
                                                range
                                                    .map(|ix| {
                                                        let (id, _object, msg) = &stashes[ix];
                                                        stash_row(
                                                            ix,
                                                            id,
                                                            msg,
                                                            selected.as_deref()
                                                                == Some(id.as_str()),
                                                            this.clone(),
                                                            cx,
                                                        )
                                                    })
                                                    .collect()
                                            },
                                        )
                                        .track_scroll(&self.stashes_scroll)
                                        .flex_1()
                                        .min_h_0()
                                        .px(px(6.))
                                        .py(px(4.))
                                        .into_any_element()
                                    }),
                            )
                            .child(div().h(px(1.)).bg(hairline(0.05)))
                            .child(
                                div()
                                    .flex_none()
                                    .h(px(30.))
                                    .px(px(8.))
                                    .flex()
                                    .items_center()
                                    .bg(ink(0.04))
                                    .border_b_1()
                                    .border_color(hairline(0.04))
                                    .child(
                                        div()
                                            .text_size(px(TEXT_XS))
                                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                                            .text_color(text_faint(cx))
                                            .child(t().section_count(
                                                t().commit_tab_changes,
                                                self.stash_changes
                                                    .as_ref()
                                                    .map(|changes| changes.files.len())
                                                    .unwrap_or(0),
                                            )),
                                    ),
                            )
                            .child(self.stash_file_list(cx)),
                    )
                    .child(self.stash_diff_pane(cx)),
            )
    }

    fn stash_file_list(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if self.selected_stash.is_none() {
            return empty_note(t().stash_select, cx).into_any_element();
        }
        if self.stash_files_loading {
            return empty_note(t().commit_loading, cx).into_any_element();
        }
        if let Some(err) = &self.stash_files_error {
            return stash_error_note(err, self.selected_stash.clone(), cx);
        }
        let Some(changes) = self.stash_changes.clone() else {
            return empty_note(t().commit_no_files, cx).into_any_element();
        };
        if changes.files.is_empty() {
            return empty_note(t().commit_no_files, cx).into_any_element();
        }
        let count = changes.files.len();
        let selected = self.stash_file_selected;
        let stash_id = self.selected_stash.clone();
        let this = cx.entity().downgrade();
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px(px(6.))
            .py(px(4.))
            .child(scrollbar_overlay(
                "stash-files-scrollbar",
                &self.stash_files_scroll,
            ))
            .child(
                gpui_kit::uniform_list("stash-files", count, move |range, _window, cx| {
                range
                    .map(|ix| {
                        let file = &changes.files[ix];
                        stash_file_row(
                            ix,
                            file,
                            changes
                                .untracked
                                .iter()
                                .any(|path| path == &file.path),
                            selected == Some(ix),
                            stash_id.clone(),
                            this.clone(),
                            cx,
                        )
                    })
                    .collect()
                })
                .track_scroll(&self.stash_files_scroll)
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }

    fn stash_diff_pane(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let selection = self.stash_file_selected.and_then(|ix| {
            self.stash_changes
                .as_ref()
                .and_then(|changes| changes.files.get(ix))
                .cloned()
        });
        let stats = match &self.stash_diff {
            commit_detail::FileDiffState::Ready(diff) => Some((diff.additions, diff.deletions)),
            _ => None,
        };
        let mut header = div()
            .flex_none()
            .h(px(32.))
            .min_w_0()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(hairline(0.05));
        if let Some(file) = &selection {
            header = header.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(MONO)
                    .text_size(px(TEXT_SM))
                    .text_color(text_primary(cx))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(truncate_path(&file.display_path(), 64)),
            );
        } else {
            header = header.child(div().flex_1());
        }
        if let Some((additions, deletions)) = stats {
            header = header.child(
                div()
                    .flex_none()
                    .font_family(MONO)
                    .text_size(px(TEXT_SM))
                    .text_color(cx.theme().success)
                    .child(format!("+{additions}")),
            );
            header = header.child(
                div()
                    .flex_none()
                    .font_family(MONO)
                    .text_size(px(TEXT_SM))
                    .text_color(cx.theme().danger)
                    .child(format!("-{deletions}")),
            );
        }
        let body: gpui_kit::AnyElement = match (&self.stash_diff, &selection) {
            // A cold selection shows the same centered note while its file
            // list loads, instead of the select-a-file placeholder
            // (SPEEDUP_REVIEW P2).
            (_, None) if self.selected_stash.is_some() && self.stash_files_loading => {
                empty_note(t().commit_loading, cx).into_any_element()
            }
            (_, None) => empty_note(t().diff_select, cx).into_any_element(),
            // A stable centered note while the diff loads (no empty flash).
            (commit_detail::FileDiffState::Loading, _) => {
                empty_note(t().commit_loading, cx).into_any_element()
            }
            (commit_detail::FileDiffState::Empty, Some(_)) => {
                empty_note(t().diff_select, cx).into_any_element()
            }
            (commit_detail::FileDiffState::Error(err), _) => stash_error_note(err, None, cx),
            (commit_detail::FileDiffState::Ready(diff), Some(_)) if diff.binary => {
                empty_note(t().diff_binary, cx).into_any_element()
            }
            (commit_detail::FileDiffState::Ready(diff), Some(_)) if diff.lines.is_empty() => {
                empty_note(t().no_changes, cx).into_any_element()
            }
            (commit_detail::FileDiffState::Ready(diff), Some(_)) => readonly_diff_list(
                ReadonlyDiffIds {
                    list: "stash-diff",
                    v_scrollbar: "stash-diff-v-scrollbar",
                    h_wrapper: "stash-diff-h",
                    h_scrollbar: "stash-diff-h-scrollbar",
                },
                diff.clone(),
                &self.stash_diff_scroll,
                &self.stash_diff_h_scroll,
            ),
        };
        div()
            .flex()
            .flex_1()
            // Definite flex basis (the Local Changes diff column's recipe):
            // without it the pane grows to the diff's min-content width, so
            // the horizontal scroller never overflows and cannot scroll.
            .w(px(0.))
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// Active repository plus the listed commit of stash `id` (as shown);
    /// queued stash ops carry it and re-resolve by identity when they run.
    fn stash_target(&mut self, id: &str, cx: &mut Context<Self>) -> Option<(String, String)> {
        let repo_id = self.active_repo_id()?;
        let oid = self
            .details
            .get(&repo_id)
            .and_then(|details| details.stashes.iter().find(|(entry, _, _)| entry == id))
            .map(|(_, hash, _)| hash.clone());
        match oid {
            Some(oid) => Some((repo_id, oid)),
            None => {
                self.note_error(t().log_action_failed("stash", "entry is no longer listed"), cx);
                None
            }
        }
    }

    /// Queue `git stash apply` for one entry (non-destructive, keeps it).
    pub(super) fn apply_stash(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some((repo_id, oid)) = self.stash_target(&id, cx) {
            self.change_ops
                .push_back(changes::ChangeOp::StashApply(changes::StashOp { repo_id, id, oid }));
            self.pump_change_ops(cx);
        }
    }

    /// Queue `git stash pop` for one entry (apply, then drop on success).
    pub(super) fn pop_stash(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some((repo_id, oid)) = self.stash_target(&id, cx) {
            self.change_ops.push_back(changes::ChangeOp::StashPop(
                changes::StashPopOp { repo_id, id, oid },
            ));
            self.pump_change_ops(cx);
        }
    }

    /// Queue a single-file restore from one stash entry. The worktree gains
    /// the file; the index and the stash entry are untouched.
    pub(super) fn apply_stash_file(
        &mut self,
        id: String,
        path: Vec<u8>,
        untracked: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some((repo_id, oid)) = self.stash_target(&id, cx) {
            self.change_ops.push_back(changes::ChangeOp::StashApplyFile(
                changes::StashApplyFileOp {
                    repo_id,
                    id,
                    oid,
                    path,
                    untracked,
                },
            ));
            self.pump_change_ops(cx);
        }
    }

    /// Copy one stashed file as an apply-able patch: read on the
    /// background thread, then to the clipboard like a plain copy.
    pub(super) fn copy_stash_file_patch(
        &mut self,
        id: String,
        path: Vec<u8>,
        untracked: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let worktree = repo.path.to_string_lossy().into_owned();
        cx.spawn(async move |this, cx| {
            let patch = cx
                .background_executor()
                .spawn(async move { crate::git::stash_file_patch(&worktree, &id, &path, untracked) })
                .await;
            this.update(cx, |this, cx| match patch {
                Ok(text) => {
                    let bytes = text.len();
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                    this.ops.push_info(t().log_copied_patch(bytes));
                    cx.notify();
                }
                Err(err) => {
                    this.note_error(t().log_action_failed("copy patch", &err), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Open the branch-from-stash dialog for one entry.
    pub(super) fn request_stash_branch(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        self.stash_branch_input.update(cx, |state, cx| {
            state.set_value("", window, cx)
        });
        self.stash_branch_request = Some(StashBranchRequest { repo_id, id });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_stash_branch(&mut self, cx: &mut Context<Self>) {
        if self.stash_branch_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: queue `git stash branch` on the sequential change queue.
    pub(super) fn confirm_stash_branch(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.stash_branch_request.clone() else {
            return;
        };
        let branch = self
            .stash_branch_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        if branch.is_empty() {
            return;
        }
        let Some((_, oid)) = self.stash_target(&request.id, cx) else {
            return;
        };
        self.change_ops.push_back(changes::ChangeOp::StashBranch(
            changes::StashBranchOp {
                repo_id: request.repo_id,
                id: request.id,
                oid,
                branch,
            },
        ));
        self.pump_change_ops(cx);
        self.close_modal(cx);
    }

    /// Drop is destructive: ask before running it.
    pub(super) fn request_stash_drop(&mut self, id: String, cx: &mut Context<Self>) {
        self.stash_drop_confirm = Some(id);
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_stash_drop(&mut self, cx: &mut Context<Self>) {
        if self.stash_drop_confirm.is_some() {
            self.close_modal(cx);
        }
    }

    pub(super) fn confirm_stash_drop(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.stash_drop_confirm.clone() else {
            return;
        };
        if let Some((repo_id, oid)) = self.stash_target(&id, cx) {
            self.change_ops
                .push_back(changes::ChangeOp::StashDrop(changes::StashOp { repo_id, id, oid }));
            self.pump_change_ops(cx);
        }
        self.close_modal(cx);
    }

    /// Full-window scrim + card for the branch-from-stash dialog.
    pub(super) fn render_stash_branch_dialog(
        &self,
        request: StashBranchRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let branch = self
            .stash_branch_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        let can_create = !branch.is_empty();
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
                cx.listener(|this, _, _, cx| this.cancel_stash_branch(cx)),
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
                                    .child(format!("{} {}", t().stash_branch_title, request.id)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().stash_branch_name),
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
                                Input::new(&self.stash_branch_input)
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
                                Button::new("stash-branch-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_stash_branch(cx)
                                    })),
                            )
                            .child(
                                Button::new("stash-branch-create")
                                    .label(t().stash_branch_confirm)
                                    .small()
                                    .primary()
                                    .disabled(!can_create)
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_stash_branch(cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Full-window scrim + card shown while a stash Drop awaits confirmation.
    pub(super) fn render_stash_drop_confirm(
        &self,
        id: String,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let body = t().stash_drop_body(&id);
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
                cx.listener(|this, _, _, cx| this.cancel_stash_drop(cx)),
            )
            .child(
                div()
                    .w(px(420.))
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
                            .text_size(px(TEXT_LG))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_primary(cx))
                            .child(t().stash_drop_title),
                    )
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(TEXT_MD))
                            .text_color(text_muted(cx))
                            .child(body),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("stash-drop-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_stash_drop(cx)
                                    })),
                            )
                            .child(
                                Button::new("stash-drop-confirm")
                                    .label(t().context_drop)
                                    .small()
                                    .danger()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_stash_drop(cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}

/// One stash entry: `stash@{n}` and its message, selectable.
fn stash_row(
    ix: usize,
    id: &str,
    message: &str,
    selected: bool,
    this: gpui_kit::WeakEntity<SpurShell>,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    let key = format!("stash-row-{ix}");
    let id_owned = id.to_string();
    let menu_this = this.clone();
    let menu_id = id.to_string();
    div()
        .id(("stash-row", ix))
        .w_full()
        .cursor_pointer()
        .flex_none()
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(1.))
        .h(px(STASH_ROW_H))
        .px(px(8.))
        .rounded(px(CONTROL_RADIUS))
        .bg(if selected {
            ink(0.08)
        } else {
            hover_blend(&key, ink(0.0), ink(0.04))
        })
        .on_hover(hover_listener(key))
        .child(
            div()
                .font_family(MONO)
                .text_size(px(TEXT_SM))
                .text_color(if selected { text_primary(cx) } else { violet(cx) })
                .child(id.to_string()),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(TEXT_XS))
                .text_color(text_muted(cx))
                .child(message.to_string()),
        )
        .on_click(move |_, _, cx| {
            let id = id_owned.clone();
            this.update(cx, |shell, cx| shell.select_stash(id, cx)).ok();
        })
        .context_menu(move |menu, _, _| {
            let apply_entity = menu_this.clone();
            let drop_entity = menu_this.clone();
            let pop_entity = menu_this.clone();
            let branch_entity = menu_this.clone();
            let apply_id = menu_id.clone();
            let drop_id = menu_id.clone();
            let pop_id = menu_id.clone();
            let branch_id = menu_id.clone();
            menu.item(context_menu_item(
                t().context_apply.into(),
                IconName::ArchiveRestore,
                move |_, _, cx| {
                    let id = apply_id.clone();
                    apply_entity
                        .update(cx, |shell, cx| shell.apply_stash(id, cx))
                        .ok();
                },
            ))
            .item(context_menu_item(
                t().context_pop.into(),
                IconName::Archive,
                move |_, _, cx| {
                    let id = pop_id.clone();
                    pop_entity
                        .update(cx, |shell, cx| shell.pop_stash(id, cx))
                        .ok();
                },
            ))
            .item(context_menu_item(
                t().context_stash_branch.into(),
                IconName::GitBranchPlus,
                move |_, window, cx| {
                    let id = branch_id.clone();
                    branch_entity
                        .update(cx, |shell, cx| shell.request_stash_branch(id, window, cx))
                        .ok();
                },
            ))
            .item(context_menu_item(
                t().context_drop.into(),
                IconName::Trash,
                move |_, _, cx| {
                    let id = drop_id.clone();
                    drop_entity
                        .update(cx, |shell, cx| shell.request_stash_drop(id, cx))
                        .ok();
                },
            ))
        })
        .into_any_element()
}

/// One changed file of the selected stash; a single click opens its diff.
fn stash_file_row(
    ix: usize,
    file: &crate::git::CommitFile,
    untracked: bool,
    selected: bool,
    stash_id: Option<String>,
    this: gpui_kit::WeakEntity<SpurShell>,
    cx: &mut gpui_kit::App,
) -> gpui_kit::AnyElement {
    let key = format!("stash-file-{ix}");
    // Owned menu target: the row borrows `file`, which is gone by right-click.
    let apply_target: Option<(String, Vec<u8>, bool)> =
        stash_id.map(|id| (id, file.path.clone(), untracked));
    // Modified stays neutral: the letter carries the meaning.
    let color = match file.status {
        'A' => cx.theme().success,
        'D' => cx.theme().danger,
        'R' | 'C' => cx.theme().warning,
        _ => text_muted(cx),
    };
    div()
        .id(("stash-file", ix))
        .w_full()
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(STASH_FILE_ROW_H))
        .px(px(8.))
        .rounded(px(CONTROL_RADIUS))
        .bg(if selected {
            ink(0.08)
        } else {
            hover_blend(&key, ink(0.0), ink(0.055))
        })
        .on_hover(hover_listener(key))
        .child(
            div()
                .w(px(12.))
                .flex_none()
                .font_family(MONO)
                .text_size(px(TEXT_SM))
                .text_color(color)
                .child(file.status.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(MONO)
                .text_size(px(TEXT_SM))
                .text_color(if selected {
                    text_primary(cx)
                } else {
                    text_muted(cx)
                })
                .overflow_hidden()
                .whitespace_nowrap()
                .child(file.display_path()),
        )
        .children(untracked.then(|| chip("u", text_muted(cx))))
        .on_click({
            let entity = this.clone();
            move |_, _, cx| {
                entity
                    .update(cx, |shell, cx| shell.open_stash_file(ix, cx))
                    .ok();
            }
        })
        .context_menu(move |menu, _, _| {
            let Some((id, path, untracked)) = apply_target.clone() else {
                return menu;
            };
            let entity = this.clone();
            let copy_entity = this.clone();
            let copy_id = id.clone();
            let copy_path = path.clone();
            menu.item(context_menu_item(
                t().context_apply_file.into(),
                IconName::Archive,
                move |_, _, cx| {
                    let id = id.clone();
                    let path = path.clone();
                    entity
                        .update(cx, |shell, cx| {
                            shell.apply_stash_file(id, path, untracked, cx)
                        })
                        .ok();
                },
            ))
            .item(context_menu_item(
                t().context_copy_patch.into(),
                IconName::ClipboardCopy,
                move |_, _, cx| {
                    let id = copy_id.clone();
                    let path = copy_path.clone();
                    copy_entity
                        .update(cx, |shell, cx| {
                            shell.copy_stash_file_patch(id, path, untracked, cx)
                        })
                        .ok();
                },
            ))
        })
        .into_any_element()
}

fn stash_error_note(
    err: &str,
    retry_id: Option<String>,
    cx: &mut Context<SpurShell>,
) -> gpui_kit::AnyElement {
    let this = cx.entity().downgrade();
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
                .child(t().stash_error),
        )
        .child(
            div()
                .max_w(px(520.))
                .font_family(MONO)
                .text_size(px(TEXT_XS))
                .text_color(text_faint(cx))
                .child(err.to_string()),
        )
        .children(retry_id.map(|id| {
            Button::new("stash-retry")
                .label(t().retry)
                .small()
                .outline()
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    let id = id.clone();
                    this.update(cx, |shell, cx| shell.select_stash(id, cx)).ok();
                })
        }))
        .into_any_element()
}