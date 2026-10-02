//! Fetch dialog: remote selection plus
//! "force override local refs", "fetch all remotes" (only shown when several
//! remotes exist), and "fetch without tags".

use super::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _, DropdownButton};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, Sizable as _};
use gpui_kit::IntoElement;

use crate::i18n::t;

/// A fetch request while the dialog is open.
#[derive(Clone, Debug)]
pub(super) struct FetchRequest {
    pub repo_id: String,
    /// Selected remote; ignored when `all` is set.
    pub remote: String,
    pub force: bool,
    /// Fetch every remote instead of the selected one.
    pub all: bool,
    pub no_tags: bool,
    /// True while the queued fetch runs; the dialog stays open with a
    /// progress bar until the operation completes.
    pub busy: bool,
}

impl SpurShell {
    /// Open the fetch dialog for the active repository.
    pub(super) fn request_fetch(&mut self, cx: &mut Context<Self>) {
        let Some(repo_id) = self.active_repo_id() else {
            return;
        };
        let remotes = self.remote_names();
        if remotes.is_empty() {
            self.note_error(t().push_no_remotes.to_string(), cx);
            cx.notify();
            return;
        }
        let remote = self
            .fetch_remote
            .clone()
            .filter(|name| remotes.contains(name))
            .or_else(|| remotes.iter().any(|name| name == "origin").then(|| "origin".to_string()))
            .unwrap_or_else(|| remotes[0].clone());
        let multiple = remotes.len() > 1;
        self.fetch_request = Some(FetchRequest {
            repo_id,
            remote,
            force: self.fetch_force,
            // "Fetch all remotes" only exists when there are several.
            all: multiple && self.fetch_all_remotes,
            no_tags: self.fetch_no_tags,
            busy: false,
        });
        self.open_modal(cx);
        cx.notify();
    }

    pub(super) fn cancel_fetch(&mut self, cx: &mut Context<Self>) {
        if self.fetch_request.is_some() {
            self.close_modal(cx);
        }
    }

    /// Confirmed: remember the options and queue the fetch. The dialog stays
    /// open (busy) until the operation completes.
    pub(super) fn confirm_fetch(&mut self, cx: &mut Context<Self>) {
        let Some(mut request) = self.fetch_request.clone() else {
            return;
        };
        if request.busy {
            return;
        }
        let multiple = self.remote_names().len() > 1;
        let all = multiple && request.all;
        self.fetch_remote = Some(request.remote.clone());
        self.fetch_force = request.force;
        self.fetch_all_remotes = all;
        self.fetch_no_tags = request.no_tags;
        request.busy = true;
        self.fetch_request = Some(request.clone());
        self.change_ops
            .push_back(changes::ChangeOp::Fetch(changes::FetchOp {
                repo_id: request.repo_id,
                remote: (!all).then_some(request.remote),
                force: request.force,
                no_tags: request.no_tags,
                from_dialog: true,
            }));
        self.pump_change_ops(cx);
    }

    /// Full-window scrim + card shown while the dialog is open.
    pub(super) fn render_fetch_dialog(
        &self,
        request: FetchRequest,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let remotes = self.remote_names();
        let multiple = remotes.len() > 1;
        let selected = request.remote.clone();
        let menu_entity = cx.entity().downgrade();
        let menu_remotes = remotes.clone();
        let remote_menu = DropdownButton::new("fetch-remote")
            .button(
                Button::new("fetch-remote-btn")
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
                                    if let Some(request) = this.fetch_request.as_mut()
                                        && !request.busy
                                    {
                                        request.remote = picked.clone();
                                    }
                                    cx.notify();
                                })
                                .ok();
                        }),
                    );
                }
                menu
            });

        let force_entity = cx.entity().downgrade();
        let all_entity = cx.entity().downgrade();
        let tags_entity = cx.entity().downgrade();

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
                cx.listener(|this, _, _, cx| this.cancel_fetch(cx)),
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
                                Icon::new(IconName::Download)
                                    .size(px(16.))
                                    .text_color(violet(cx)),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_LG))
                                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                                    .text_color(text_primary(cx))
                                    .child(t().fetch_title),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_XS))
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .text_color(text_faint(cx))
                            .child(t().push_remote),
                    )
                    .child(select_shell(remote_menu))
                    .child(
                        Checkbox::new("fetch-force")
                            .checked(request.force)
                            .label(t().fetch_force)
                            .on_change(move |checked, _, cx| {
                                let checked = *checked;
                                force_entity
                                    .update(cx, |this, cx| {
                                        if let Some(request) = this.fetch_request.as_mut()
                                            && !request.busy
                                        {
                                            request.force = checked;
                                            this.fetch_force = checked;
                                        }
                                        cx.notify();
                                    })
                                    .ok();
                            }),
                    )
                    .children(multiple.then(|| {
                        Checkbox::new("fetch-all")
                            .checked(request.all)
                            .label(t().fetch_all)
                            .on_change(move |checked, _, cx| {
                                let checked = *checked;
                                all_entity
                                    .update(cx, |this, cx| {
                                        if let Some(request) = this.fetch_request.as_mut()
                                            && !request.busy
                                        {
                                            request.all = checked;
                                            this.fetch_all_remotes = checked;
                                        }
                                        cx.notify();
                                    })
                                    .ok();
                            })
                    }))
                    .child(
                        Checkbox::new("fetch-no-tags")
                            .checked(request.no_tags)
                            .label(t().fetch_no_tags)
                            .on_change(move |checked, _, cx| {
                                let checked = *checked;
                                tags_entity
                                    .update(cx, |this, cx| {
                                        if let Some(request) = this.fetch_request.as_mut()
                                            && !request.busy
                                        {
                                            request.no_tags = checked;
                                            this.fetch_no_tags = checked;
                                        }
                                        cx.notify();
                                    })
                                    .ok();
                            }),
                    )
                    .children(request.busy.then(|| {
                        let label = if request.all {
                            t().fetch_running_all()
                        } else {
                            t().fetch_running(&request.remote)
                        };
                        busy_bar(label, true, cx)
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                Button::new("fetch-cancel")
                                    .label(t().cancel)
                                    .small()
                                    .text()
                                    .cursor_pointer()
                                    .disabled(request.busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_fetch(cx))),
                            )
                            .child(
                                Button::new("fetch-confirm")
                                    .label(t().fetch_confirm)
                                    .small()
                                    .primary()
                                    .disabled(request.busy || (!multiple && remotes.is_empty()))
                                    .cursor_pointer()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.confirm_fetch(cx)),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
