//! Commit context menu: per-commit actions for history rows.
//!
//! The safe subset ships first: detached checkout, branch/tag creation,
//! cherry-pick, revert, and clipboard copies.

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::WeakEntity;

use crate::i18n::t;

/// What the menu acts on: one loaded commit plus the checked-out branch name
/// (for the cherry-pick label).
#[derive(Clone)]
pub(super) struct CommitTarget {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub current_branch: Option<String>,
}

impl CommitTarget {
    pub(super) fn of(
        commit: &crate::model::HistoryCommit,
        current_branch: Option<SharedString>,
    ) -> Self {
        let short = commit.hash.get(..7).unwrap_or(&commit.hash).to_string();
        CommitTarget {
            hash: commit.hash.clone(),
            short,
            subject: commit.subject.clone(),
            current_branch: current_branch.map(|branch| branch.to_string()),
        }
    }
}

/// The full commit menu for a row right-click. Every item clones what it
/// needs; handlers resolve the repository through the shell like the stash
/// menu does.
pub(super) fn commit_menu(
    menu: PopupMenu,
    target: CommitTarget,
    this: WeakEntity<SpurShell>,
) -> PopupMenu {
    let pick = target.current_branch.clone().unwrap_or_else(|| "HEAD".to_string());
    menu    .item(commit_item(
        t().context_checkout_detached.into(),
        IconName::GitCommitHorizontal,
        ItemExplainer {
            kind: explainer::ExplainerKind::CheckoutDetached,
            ctx_a: String::new(),
        },
        this.clone(),
        target.hash.clone(),
        target.short.clone(),
        |shell, hash, short, _, cx| shell.checkout_commit_detached(hash, short, cx),
    ))
    .item(commit_item(
        t().context_create_branch_here.into(),
        IconName::GitBranchPlus,
        ItemExplainer {
            kind: explainer::ExplainerKind::CreateBranch,
            ctx_a: String::new(),
        },
        this.clone(),
        target.hash.clone(),
        target.short.clone(),
        |shell, hash, _, window, cx| shell.request_create_branch_at(hash, window, cx),
    ))
    .item(commit_item(
        t().context_create_tag_here.into(),
        IconName::Tag,
        ItemExplainer {
            kind: explainer::ExplainerKind::CreateTag,
            ctx_a: String::new(),
        },
        this.clone(),
        target.hash.clone(),
        target.short.clone(),
        |shell, hash, _, window, cx| shell.request_create_tag(hash, window, cx),
    ))
    .item(commit_item(
        t().cherry_pick_onto(&pick),
        IconName::Plus,
        ItemExplainer {
            kind: explainer::ExplainerKind::CherryPick,
            ctx_a: pick.clone(),
        },
        this.clone(),
        target.hash.clone(),
        target.short.clone(),
        |shell, hash, short, _, cx| shell.cherry_pick_commit(hash, short, cx),
    ))
    .item(commit_item(
        t().context_revert_commit.into(),
        IconName::RotateCcw,
        ItemExplainer {
            kind: explainer::ExplainerKind::Revert,
            ctx_a: String::new(),
        },
        this.clone(),
        target.hash.clone(),
        target.short.clone(),
        |shell, hash, short, _, cx| shell.revert_commit(hash, short, cx),
    ))
    .item({
        let entity = this.clone();
        let target = target.clone();
        let label = t().reset_here_action(target.current_branch.as_deref().unwrap_or("HEAD"));
        explainer::explained_menu_item(
            label.clone(),
            IconName::Rewind,
            explainer::ExplainerKind::Reset,
            label,
            target
                .current_branch
                .clone()
                .unwrap_or_else(|| "HEAD".to_string()),
            entity.clone(),
            move |_, _, cx| {
                entity
                    .update(cx, |shell, cx| {
                        shell.clear_explainer(cx);
                        shell.request_reset_to(
                            target.hash.clone(),
                            target.short.clone(),
                            target.subject.clone(),
                            cx,
                        )
                    })
                    .ok();
            },
        )
    })
    .item(copy_item(
        t().context_copy_sha.into(),
        explainer::ExplainerKind::CopySha,
        this.clone(),
        target.hash.clone(),
        "SHA",
    ))
    .item(copy_item(
        t().context_copy_short_sha.into(),
        explainer::ExplainerKind::CopyShortSha,
        this.clone(),
        target.short.clone(),
        "short SHA",
    ))
    .item(copy_item(
        t().context_copy_subject.into(),
        explainer::ExplainerKind::CopySubject,
        this.clone(),
        target.subject.clone(),
        "subject",
    ))
    .item({
        let entity = this.clone();
        let hash = target.hash.clone();
        let label: String = t().context_copy_patch.into();
        explainer::explained_menu_item(
            label.clone(),
            IconName::ClipboardCopy,
            explainer::ExplainerKind::CopyPatch,
            label,
            String::new(),
            entity.clone(),
            move |_, _, cx| {
                entity
                    .update(cx, |shell, cx| {
                        shell.clear_explainer(cx);
                        shell.copy_commit_patch(hash.clone(), cx)
                    })
                    .ok();
            },
        )
    })
}

/// Hover explainer for one menu item: which card plus its context.
#[derive(Clone)]
struct ItemExplainer {
    kind: explainer::ExplainerKind,
    ctx_a: String,
}

/// One menu item running a shell operation against the active repository,
/// with a hover explainer.
fn commit_item(
    label: String,
    icon: IconName,
    explainer: ItemExplainer,
    this: WeakEntity<SpurShell>,
    hash: String,
    short: String,
    run: impl Fn(&mut SpurShell, String, String, &mut Window, &mut Context<SpurShell>) + 'static,
) -> PopupMenuItem {
    let title = label.clone();
    explainer::explained_menu_item(
        label,
        icon,
        explainer.kind,
        title,
        explainer.ctx_a,
        this.clone(),
        move |_, window, cx| {
            let hash = hash.clone();
            let short = short.clone();
            this.update(cx, |shell, cx| {
                shell.clear_explainer(cx);
                run(shell, hash, short, &mut *window, cx)
            })
            .ok();
        },
    )
}

/// One clipboard copy: silent except for the operation-log line.
fn copy_item(
    label: String,
    kind: explainer::ExplainerKind,
    this: WeakEntity<SpurShell>,
    text: String,
    what: &'static str,
) -> PopupMenuItem {
    let title = label.clone();
    explainer::explained_menu_item(
        label,
        IconName::ClipboardCopy,
        kind,
        title,
        String::new(),
        this.clone(),
        move |_, _, cx| {
            let text = text.clone();
            this.update(cx, |shell, cx| {
                shell.clear_explainer(cx);
                shell.copy_commit_text(what, text, cx)
            })
            .ok();
        },
    )
}

impl SpurShell {
    /// Queue `git checkout --detach` for one commit.
    pub(super) fn checkout_commit_detached(
        &mut self,
        hash: String,
        short: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(repo_id) = self.active_repo_id() {
            self.change_ops.push_back(changes::ChangeOp::CheckoutDetached(
                changes::CheckoutDetachedOp {
                    repo_id,
                    hash,
                    short,
                },
            ));
            self.pump_change_ops(cx);
        }
    }

    /// Queue `git cherry-pick` onto the checked-out branch.
    pub(super) fn cherry_pick_commit(
        &mut self,
        hash: String,
        short: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(repo_id) = self.active_repo_id() {
            self.change_ops.push_back(changes::ChangeOp::CherryPick(
                changes::CherryPickOp {
                    repo_id,
                    hash,
                    short,
                },
            ));
            self.pump_change_ops(cx);
        }
    }

    /// Queue `git revert` of one commit.
    pub(super) fn revert_commit(&mut self, hash: String, short: String, cx: &mut Context<Self>) {
        if let Some(repo_id) = self.active_repo_id() {
            self.change_ops.push_back(changes::ChangeOp::Revert(changes::RevertOp {
                repo_id,
                hash,
                short,
            }));
            self.pump_change_ops(cx);
        }
    }

    /// Copy a commit field to the clipboard (SHA, short SHA, subject).
    pub(super) fn copy_commit_text(
        &mut self,
        what: &'static str,
        text: String,
        cx: &mut Context<Self>,
    ) {
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
        self.ops.push_info(t().log_copied_commit(what));
        cx.notify();
    }

    /// Copy an apply-able patch of one commit: the patch is read on the
    /// background thread, then lands on the clipboard like a plain copy.
    pub(super) fn copy_commit_patch(&mut self, hash: String, cx: &mut Context<Self>) {
        let Some(repo) = self.active_repo() else {
            return;
        };
        let worktree = repo.path.to_string_lossy().into_owned();
        cx.spawn(async move |this, cx| {
            let patch = cx
                .background_executor()
                .spawn(async move { crate::git::commit_patch(&worktree, &hash) })
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

    /// The selected commit as a menu target (palette entries).
    pub(super) fn selected_commit_target(&self) -> Option<CommitTarget> {
        let hash = self.selected_commit.clone()?;
        let short = hash.get(..7).unwrap_or(&hash).to_string();
        let subject = self
            .history
            .iter()
            .find(|commit| commit.hash == hash)
            .map(|commit| commit.subject.clone())
            .unwrap_or_default();
        let current_branch = self
            .active_snapshot()
            .and_then(|collected| collected.snapshot.branch.clone());
        Some(CommitTarget {
            hash,
            short,
            subject,
            current_branch,
        })
    }
}
