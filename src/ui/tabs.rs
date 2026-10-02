//! Repository tab strip: open repositories as switchable tabs inside the app
//! bar. Idle, hover, active, and pressed states share one wash vocabulary with
//! the sidebar rows and the palette.

use super::*;

use gpui_kit::base::InteractiveElementExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::{
    App, Focusable as _, InteractiveElement as _, IntoElement, SharedString,
    StatefulInteractiveElement as _, Window, WindowControlArea,
};

use crate::i18n::t;

gpui_kit::actions!(spur, [NextTab, PrevTab, CloseTab]);

/// Per-tab work context remembered across tab switches: the section, the
/// selected change rows, and the stash view. Pending conflict choices are
/// NOT remembered on this base: its resolver has no content digest, so a
/// restored choice list could describe stale content; choices are dropped
/// when the resolver is left.
pub(super) struct TabContext {
    section: repo::RepoSection,
    change_selection: Option<changes::Selection>,
    change_selected: Vec<changes::Selection>,
    change_anchor: Option<(bool, usize)>,
    selected_stash: Option<String>,
    selected_stash_file: Option<usize>,
}

/// True while a text field owns keyboard focus; navigation shortcuts must
/// not hijack typing in the commit message or any filter input.
pub(super) fn text_input_focused(shell: &SpurShell, window: &Window, cx: &App) -> bool {
    let Some(focused) = window.focused(cx) else {
        return false;
    };
    let inputs = [
        &shell.root_input,
        &shell.history_search_input,
        &shell.change_filter,
        &shell.stash_message_input,
        &shell.branch_name_input,
        &shell.branch_rename_input,
        &shell.stash_branch_input,
        &shell.tag_name_input,
        &shell.tag_message_input,
        &shell.tag_filter_input,
        &shell.remote_name_input,
        &shell.remote_url_input,
        &shell.profile_label_input,
        &shell.profile_name_input,
        &shell.profile_email_input,
        &shell.profile_github_input,
        &shell.palette_input,
        &shell.theme_name_input,
    ];
    inputs
        .iter()
        .any(|input| input.read(cx).focus_handle(cx) == focused)
        || shell.commit_message.read(cx).focus_handle(cx) == focused
}

impl SpurShell {
    /// Save the leaving tab's work context (never its requests: generations
    /// are bumped by the reset, so a late reply cannot land in the new tab).
    fn save_tab_context(&mut self, id: &str) {
        let context = TabContext {
            section: self.section,
            change_selection: self.change_selection.clone(),
            change_selected: self.change_selected.clone(),
            change_anchor: self.change_anchor,
            selected_stash: self.selected_stash.clone(),
            selected_stash_file: self.stash_file_selected,
        };
        self.tab_contexts.insert(id.to_string(), context);
    }

    /// Restore one tab's saved context: section, change selection, and stash
    /// view. A conflicted selection reopens the resolver fresh (no stale
    /// choices on this base, which has no content digest).
    fn restore_tab_context(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(context) = self.tab_contexts.remove(id) else {
            return;
        };
        self.section = context.section;
        self.change_selection = context.change_selection;
        self.change_selected = context.change_selected;
        self.change_anchor = context.change_anchor;
        self.load_diff(cx);
        if self.section == repo::RepoSection::Stashes
            && let Some(stash_id) = context.selected_stash
        {
            self.select_stash(stash_id, cx);
            if let Some(ix) = context.selected_stash_file {
                self.open_stash_file(ix, cx);
            }
        }
        cx.notify();
    }

    /// Switch the active repository. `leaving` is the repository identity
    /// captured BEFORE `self.active` moved to the destination: resolving it
    /// afterwards would return the entering tab and silently skip the save.
    fn switch_active_to(
        &mut self,
        key: PathKey,
        leaving: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(leaving) = leaving.filter(|id| id != &key.0) {
            self.save_tab_context(&leaving);
        }
        self.reset_changes_view();
        self.open_history(cx);
        self.refresh_active_details(false, cx);
        self.restore_tab_context(&key.0, cx);
    }

    /// Open a repository: switch to its tab or append a new one.
    pub(super) fn open_repo(&mut self, key: PathKey, cx: &mut Context<Self>) {
        self.settings_open = false;
        // Opening a repository is the end of the first run: the welcome line
        // goes.
        self.mark_welcomed(cx);
        let leaving = self.active_repo_id();
        if let Some(ix) = self.tabs.iter().position(|t| *t == key) {
            self.active = Some(ix);
            crate::logging::log!("tab: switch {}", key.0);
        } else {
            crate::logging::log!("tab: open {}", key.0);
            self.tabs.push(key.clone());
            self.active = Some(self.tabs.len() - 1);
        }
        self.section = repo::RepoSection::History;
        self.switch_active_to(key, leaving, cx);
        self.scroll_active_tab_into_view();
        self.persist_tabs(cx);
        cx.notify();
    }

    /// Provisional row + tab key for a path that discovery has not seen yet.
    fn known_row_key(&mut self, path_str: &str) -> PathKey {
        match self
            .repos
            .iter()
            .find(|r| r.path.to_string_lossy() == path_str)
        {
            Some(existing) => PathKey::new(&existing.path),
            None => {
                let name = path_str
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("repository")
                    .to_string();
                self.repos.push(RepoEntry {
                    name,
                    path: std::path::PathBuf::from(path_str),
                    branch: None,
                    unborn: false,
                    upstream: None,
                    ahead: 0,
                    behind: 0,
                    extra_branches: 0,
                    flags: vec![],
                    last_fetch: None,
                    last_collected: None,
                    state: RowState::Loading,
                });
                PathKey::new(std::path::Path::new(path_str))
            }
        }
    }

    /// Register a path for immediate active-first status even before the
    /// workspace scan discovers it. The row starts unknown —
    /// never clean — and discovery later merges by identity.
    fn register_known_path(&mut self, path_str: &str) {
        let name = path_str
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("repository");
        let path = std::path::Path::new(path_str);
        if self.overview.upsert_known(path_str, name, path, path) {
            crate::logging::log!("tab: status {path_str} (pre-discovery)");
        }
    }

    /// Open a repository by path: reuse its row when known (discovery fixtures
    /// or a previous drop), otherwise register it as a loading row.
    pub(super) fn open_repo_path(&mut self, path_str: &str, cx: &mut Context<Self>) {
        let key = self.known_row_key(path_str);
        self.register_known_path(path_str);
        self.open_repo(key, cx);
        // The status round is gated on discovery; the active repository's
        // query is not: start it now so flags and the dirty dot arrive
        // promptly.
        if let Some(id) = self.active_repo_id() {
            self.refresh_repo(id, true, cx);
        }
    }

    /// Add a remembered tab without loading its content. Only the chosen tab
    /// is activated later; the rest load when first opened.
    pub(super) fn restore_tab(&mut self, path_str: &str, _cx: &mut Context<Self>) {
        let key = self.known_row_key(path_str);
        if !self.tabs.contains(&key) {
            self.tabs.push(key);
        }
    }

    /// Activate one restored tab: reset the per-repository view, load its
    /// history/details, and start its status query.
    pub(super) fn activate_restored_tab(&mut self, path_str: &str, cx: &mut Context<Self>) {
        let Some(ix) = self.tabs.iter().position(|key| key.0 == path_str) else {
            return;
        };
        self.active = Some(ix);
        self.register_known_path(path_str);
        self.reset_changes_view();
        self.open_history(cx);
        self.refresh_active_details(false, cx);
        self.refresh_repo(path_str.to_string(), true, cx);
        self.scroll_active_tab_into_view();
        cx.notify();
    }

    /// Switch to an already-open tab: reset the per-repository view state,
    /// load its history, and fetch its inspector details (remotes, tags,
    /// stashes) so the sections are populated immediately instead of waiting
    /// for the next refresh round.
    pub(super) fn activate_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() || self.active == Some(ix) {
            return;
        }
        let leaving = self.active_repo_id();
        self.active = Some(ix);
        let key = self.tabs[ix].clone();
        self.switch_active_to(key.clone(), leaving, cx);
        // Switching to a tab the scan has not discovered yet must register
        // its row and start its status now, not when discovery finishes
        // (SPEEDUP_REVIEW P2).
        self.register_known_path(&key.0);
        self.refresh_repo(key.0.clone(), false, cx);
        self.scroll_active_tab_into_view();
        // Activation is a real navigation: persist it so a restart restores
        // the last visited repository.
        self.persist_tabs(cx);
        cx.notify();
    }

    pub(super) fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        let closed = self.tabs[ix].0.clone();
        crate::logging::log!("tab: close {closed}");
        // The closed tab's cached history and inspector data are no longer
        // reachable.
        self.drop_cached_history(&closed);
        self.drop_cached_details(&closed);
        self.tab_contexts.remove(&closed);
        // A repository opened outside discovery lives only as long as its
        // tab: forget it so it is no longer listed or polled.
        if self.overview.forget_known(&closed) {
            self.sync_repo_rows();
        }
        let active_before = self.active.and_then(|active| self.tabs.get(active).cloned());
        // Closing the active tab must not re-cache its session when the next
        // tab opens: `open_history` stores the session it is leaving, so the
        // closed one has to be disowned first.
        if self
            .history_for
            .as_ref()
            .is_some_and(|key| key.0 == closed)
        {
            self.history_for = None;
        }
        self.tabs.remove(ix);
        self.active = match self.active {
            None => None,
            Some(_) if self.tabs.is_empty() => None,
            Some(a) if ix < a => Some(a - 1),
            Some(a) if ix == a => Some(a.min(self.tabs.len() - 1)),
            other => other,
        };
        // Closing an UNRELATED tab must not touch the active repository's
        // work context: only a changed active identity resets the view and
        // loads the next tab's content (pending conflict choices live in
        // `conflict_view`).
        let active_after = self.active.and_then(|active| self.tabs.get(active).cloned());
        if active_before != active_after {
            self.reset_changes_view();
            if self.active.is_some() {
                self.open_history(cx);
                self.refresh_active_details(false, cx);
                if let Some(id) = self.active_repo_id() {
                    self.restore_tab_context(&id, cx);
                }
            } else {
                self.clear_history();
            }
            self.scroll_active_tab_into_view();
        }
        self.persist_tabs(cx);
        cx.notify();
    }

    /// Cycle to the next/previous tab (keyboard navigation).
    pub(super) fn cycle_tab(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.tabs.len() < 2 {
            return;
        }
        let current = self.active.unwrap_or(0);
        let next = if forward {
            (current + 1) % self.tabs.len()
        } else {
            (current + self.tabs.len() - 1) % self.tabs.len()
        };
        self.activate_tab(next, cx);
    }

    /// Estimated horizontal position of one tab in the scroller; labels are
    /// at most 22 chars, so a per-tab estimate is within a few pixels and is
    /// only used to nudge the active tab into view.
    fn estimated_tab_offset(&self, ix: usize) -> f32 {
        self.tabs
            .iter()
            .take(ix)
            .map(|key| {
                let repo = self.repos.iter().find(|r| PathKey::new(&r.path) == *key);
                let name_len = repo
                    .map(|r| r.name.chars().count().min(22))
                    .unwrap_or(1) as f32;
                let wsl = repo
                    .map(|r| crate::model::is_wsl_worktree(&r.path.to_string_lossy()))
                    .unwrap_or(false);
                name_len * 7.3 + 30.0 + 24.0 + if wsl { 34.0 } else { 0.0 } + 4.0
            })
            .sum()
    }

    /// Nudge the tab scroller so the active tab is inside the viewport
    /// (approximate; the wheel still reaches every tab).
    fn scroll_active_tab_into_view(&self) {
        let Some(ix) = self.active else {
            return;
        };
        let start = self.estimated_tab_offset(ix);
        let width = self
            .tabs
            .get(ix)
            .map(|key| {
                let _ = key;
                self.estimated_tab_offset(ix + 1) - start
            })
            .unwrap_or(0.0);
        let current = self.tabs_scroll.offset().x.as_f32();
        let viewport = 600.0; // conservative estimate; the wheel covers the rest
        let visible_left = -current;
        if start < visible_left {
            self.tabs_scroll.set_offset(point(px(-start), px(0.)));
        } else if start + width > visible_left + viewport {
            self.tabs_scroll
                .set_offset(point(px(-(start + width - viewport)), px(0.)));
        }
    }

    /// Tabs + the "open repository" button, sized to share the app bar with
    /// the identity and the window actions. The tab strip scrolls
    /// horizontally so many open repositories cannot hide later tabs;
    /// the leftover space is a window drag surface (Zeron's
    /// titlebar drag strip).
    pub(super) fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut strip = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.));

        for (ix, key) in self.tabs.iter().enumerate() {
            let repo = self.repos.iter().find(|r| PathKey::new(&r.path) == *key);
            let name: SharedString = repo
                .map(|r| truncate_label(&r.name, 22).into())
                .unwrap_or_else(|| "?".into());
            let is_wsl = repo
                .map(|r| crate::model::is_wsl_worktree(&r.path.to_string_lossy()))
                .unwrap_or(false);
            let dirty = repo.map(|r| r.has_local_changes()).unwrap_or(false);
            // A restored/open repository without status yet shows a faint dot
            // (unknown, never clean); the slot is always reserved so the
            // label never shifts when the real state arrives.
            let unknown = repo
                .map(|r| matches!(r.state, RowState::Loading) && !dirty)
                .unwrap_or(false);
            // The marker carries meaning, not just colour — a quiet dot
            // for local changes, a gis glyph when the repository is behind or
            // diverged, the faint dot while status is still collecting. The
            // "needs action" colour reads as information, never as an error.
            let (ahead, behind) = repo.map(|r| (r.ahead, r.behind)).unwrap_or((0, 0));
            let needs_action = behind > 0;
            let diverged = ahead > 0 && behind > 0;
            // The tooltip counts the files behind the dot and names the sync
            // gap; it only exists when there is something to say.
            let local = repo
                .filter(|_| dirty)
                .and_then(|row| {
                    let id = row.path.to_string_lossy();
                    self.overview.row(id.as_ref()).and_then(|r| r.snapshot.as_ref())
                })
                .map(|collected| {
                    collected.snapshot.entries.len() + collected.snapshot.untracked.len()
                });
            let tip = t().tab_status_tooltip(local, behind, ahead);
            let is_active = self.active == Some(ix);
            let fade_key = format!("tab-{ix}");
            let mut tab = div()
                .id(("tab", ix))
                .cursor_pointer()
                .relative()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.))
                .h(px(26.))
                .pl(px(10.))
                .pr(px(5.))
                .rounded(px(6.))
                .text_size(px(TEXT_MD));
            tab = if is_active {
                // The raised surface plus the violet underline marks the
                // active tab without moving any row.
                tab.bg(hover_blend(&fade_key, cx.theme().muted, ink(0.13)))
                    .text_color(text_primary(cx))
            } else {
                tab.bg(hover_blend(&fade_key, ink(0.0), ink(0.05)))
                    .text_color(hover_blend(&fade_key, text_muted(cx), text_primary(cx)))
            };
            if !tip.is_empty() {
                tab = tab.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx));
            }
            let name_el = if is_active {
                div().font_weight(gpui_kit::FontWeight::MEDIUM).child(name)
            } else {
                div().child(name)
            };
            strip = strip.child(
                tab.child(
                    // One fixed slot for every state: a gis glyph when
                    // the repository needs an update, a quiet dot for local
                    // changes, a faint ring while status collects, empty
                    // otherwise — the name never shifts between them.
                    div()
                        .flex_none()
                        .w(px(10.))
                        .h(px(12.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(tab_marker(needs_action, diverged, dirty, unknown, cx)),
                )
                .child(name_el)
                // Environment marker, not selection.
                .children(is_wsl.then(|| chip(t().root_kind_wsl, text_muted(cx))))
                .children(is_active.then(|| {
                    div()
                        .absolute()
                        .bottom(px(0.))
                        .left(px(6.))
                        .right(px(6.))
                        .h(px(2.))
                        .rounded(px(1.))
                        .bg(violet(cx))
                }))
                .child(
                    div()
                        .id(("tab-close", ix))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(px(18.))
                        .h(px(18.))
                        .rounded(px(4.))
                        .bg(hover_blend(&format!("tab-close-{ix}"), ink(0.0), ink(0.10)))
                        .on_hover(hover_listener(format!("tab-close-{ix}")))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.close_tab(ix, cx);
                        }))
                        .child(
                            Icon::new(IconName::Close)
                                .size(px(12.))
                                .text_color(text_muted(cx)),
                        ),
                )
                    .on_hover(hover_listener(fade_key))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_tab(ix, cx);
                    })),
            );
        }

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .gap(px(4.))
            .child(
                // The scroller keeps later tabs reachable instead of clipping
                // them. It shrinks (to zero) before anything
                // else does, so a narrow window scrolls the tabs instead of
                // letting the title-bar buttons paint over them.
                div()
                    .id("tab-scroll")
                    .flex_shrink_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .overflow_x_scroll()
                    .lock_scroll_axis()
                    .track_scroll(&self.tabs_scroll)
                    .child(strip),
            )
            .child(
                Button::new("open-repo-palette")
                    .icon(Icon::new(IconName::Plus).size(px(14.)))
                    .text()
                    .small()
                    .flex_none()
                    .tooltip(t().tooltip_open_repo)
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_palette(window, cx))),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
    }
}

/// The tab marker: `⇣`/`⇕` in the theme's information colour while the
/// repository is behind or diverged, a quiet dot for local changes, a faint
/// ring while status is still collecting, nothing when clean. Glyph and shape
/// carry the meaning, so colour never has to.
fn tab_marker(
    needs_action: bool,
    diverged: bool,
    dirty: bool,
    unknown: bool,
    cx: &App,
) -> gpui_kit::AnyElement {
    if needs_action {
        return div()
            .text_size(px(TEXT_XS))
            .text_color(cx.theme().info)
            .child(if diverged { "\u{21d5}" } else { "\u{21e3}" })
            .into_any_element();
    }
    if dirty {
        return div()
            .size(px(6.))
            .rounded(px(3.))
            .bg(text_primary(cx).alpha(0.5))
            .into_any_element();
    }
    if unknown {
        // A hollow ring, not a filled dot: at 50 % the local-changes dot and
        // the faint colour are nearly the same grey, so the two states must
        // differ in shape to be told apart.
        return div()
            .size(px(6.))
            .rounded(px(3.))
            .border_1()
            .border_color(text_faint(cx))
            .into_any_element();
    }
    div().into_any_element()
}
