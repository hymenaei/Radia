use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px};
#[cfg(target_os = "linux")]
use gpui_component::StyledExt;
use gpui_component::{
    Disableable, Icon, IconName, Theme,
    button::{Button, ButtonVariant, ButtonVariants as _},
    checkbox::Checkbox,
    input::Input,
    select::Select,
};
use network::Grid;

#[cfg(target_os = "linux")]
use crate::gui::components::window_chrome::linux_titlebar;
#[cfg(target_os = "windows")]
use crate::gui::components::window_chrome::windows_titlebar;
use crate::remembered_login;

use super::{
    CredentialWarning, LoginPhase, LoginScreen,
    spawn::{SpawnChoice, spawn_options},
};
impl LoginScreen {
    fn render_heading(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx).colors;
        div().flex().items_center().gap_3().child(Icon::new(IconName::Globe).size_5().text_color(theme.primary)).child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_size(px(22.)).text_color(theme.foreground).child("Sign in"))
                .child(div().text_size(px(13.)).text_color(theme.muted_foreground).child("Connect to your Second Life account.")),
        )
    }

    fn render_credentials_form(&self, form_disabled: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = Theme::global(cx).colors.foreground;
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(13.)).text_color(foreground).child("Username"))
                    .child(Input::new(&self.username).w_full().h(px(42.)).focus_bordered(false).disabled(form_disabled)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(13.)).text_color(foreground).child("Password"))
                    .child(Input::new(&self.password).w_full().h(px(42.)).focus_bordered(false).disabled(form_disabled)),
            )
    }

    fn render_mfa_prompt(&self, show_mfa: bool, connecting: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = Theme::global(cx).colors.foreground;
        div().when(show_mfa, |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(13.)).text_color(foreground).child("Authentication code"))
                    .child(Input::new(&self.mfa_code).w_full().h(px(42.)).focus_bordered(false).disabled(connecting)),
            )
        })
    }

    fn render_spawn_selection(&self, form_disabled: bool, show_region_input: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = Theme::global(cx).colors.foreground;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_size(px(13.)).text_color(foreground).child("Start at"))
            .child(Select::new(&self.spawn_location).w_full().h(px(40.)).disabled(form_disabled))
            .when(show_region_input, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_size(px(13.)).text_color(foreground).child("Region name"))
                        .child(Input::new(&self.region_name).w_full().h(px(42.)).focus_bordered(false).disabled(form_disabled)),
                )
            })
    }

    fn render_grid_selection(&self, form_disabled: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = Theme::global(cx).colors.foreground;
        div().flex().items_center().justify_between().child(div().text_size(px(13.)).text_color(foreground).child("Grid")).child(
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("grid-agni")
                        .label("Main grid")
                        .with_variant(if self.grid == Grid::Agni { ButtonVariant::Primary } else { ButtonVariant::Secondary })
                        .h(px(34.))
                        .disabled(form_disabled)
                        .on_click(cx.listener(|this, _event, window, cx| this.select_grid(Grid::Agni, window, cx))),
                )
                .child(
                    Button::new("grid-aditi")
                        .label("Beta grid")
                        .with_variant(if self.grid == Grid::Aditi { ButtonVariant::Primary } else { ButtonVariant::Secondary })
                        .h(px(34.))
                        .disabled(form_disabled)
                        .on_click(cx.listener(|this, _event, window, cx| this.select_grid(Grid::Aditi, window, cx))),
                ),
        )
    }

    fn render_remember_control(&self, form_disabled: bool, cx: &mut Context<Self>) -> impl IntoElement {
        Checkbox::new("remember-login")
            .checked(self.remember_login)
            .label("Remember username, password and MFA")
            .items_center()
            .disabled(form_disabled)
            .on_click({
                let screen = cx.entity().downgrade();
                move |checked, window, app| {
                    if screen
                        .update(app, |this, cx| {
                            this.remember_login = *checked;
                            if !*checked {
                                this.remembered_login = None;
                                this.spawn_choice = SpawnChoice::Last;
                                this.spawn_location.update(cx, |state, cx| {
                                    state.set_items(spawn_options(&[]), window, cx);
                                    state.set_selected_value(&SpawnChoice::Last, window, cx);
                                });
                                let grid = this.grid;
                                let generation = this.session_generation;
                                let write_lock = this.credential_write_lock.clone();
                                let background = cx.background_executor().clone();
                                cx.spawn(async move |this, cx| {
                                    let result = background
                                        .spawn(async move {
                                            let _guard = write_lock.lock().await;
                                            remembered_login::clear(grid)
                                        })
                                        .await;
                                    if let Err(error) = result
                                        && this
                                            .update(cx, |view, cx| {
                                                if view.session_generation == generation {
                                                    view.status.session_failure = None;
                                                    view.status.credential_warning = Some(CredentialWarning::Visible(error));
                                                    cx.notify();
                                                }
                                            })
                                            .is_err()
                                    {
                                        tracing::trace!("login screen was already unavailable after clearing saved credentials");
                                    }
                                })
                                .detach();
                            }
                            cx.notify();
                        })
                        .is_err()
                    {
                        tracing::trace!("login screen was already unavailable after changing the remember setting");
                    }
                }
            })
    }

    fn render_status(&self, form_disabled: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx).colors;
        let session_active = self.active_session.as_ref().is_some_and(|active| active.session.is_some());
        div().when_some(self.status.displayed(self.favorites.status_messages(), session_active), |this, status| {
            let color = if session_active {
                theme.success
            } else if form_disabled {
                theme.muted_foreground
            } else {
                theme.danger
            };
            this.child(div().text_size(px(13.)).text_color(color).child(status))
        })
    }

    fn render_submit_button(&self, can_submit: bool, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("login")
            .label(match self.login_phase {
                LoginPhase::Idle => "Log in",
                LoginPhase::Connecting => "Connecting…",
                LoginPhase::MfaRequired(_) => "Verify",
                LoginPhase::VerifyingMfa => "Verifying…",
            })
            .primary()
            .w_full()
            .h(px(42.))
            .disabled(!can_submit)
            .on_click(cx.listener(|this, _event, window, cx| this.submit(window, cx)))
    }
}

impl Render for LoginScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.favorites.needs_display_refresh() {
            let options = spawn_options(self.favorites.locations());
            self.spawn_location.update(cx, |state, cx| {
                state.set_items(options, window, cx);
                state.set_selected_value(&SpawnChoice::Last, window, cx);
            });
            self.spawn_choice = SpawnChoice::Last;
            self.favorites.mark_displayed();
        }

        let username = self.username.read(cx).value();
        let password = self.password.read(cx).value();
        let mfa_code = self.mfa_code.read(cx).value();
        let show_region_input = self.spawn_choice == SpawnChoice::Region;
        let connecting = matches!(self.login_phase, LoginPhase::Connecting | LoginPhase::VerifyingMfa);
        let show_mfa = matches!(self.login_phase, LoginPhase::MfaRequired(_) | LoginPhase::VerifyingMfa);
        let can_submit = !self.storage_loading
            && match self.login_phase {
                LoginPhase::Idle => !username.trim().is_empty() && !password.is_empty(),
                LoginPhase::MfaRequired(_) => !mfa_code.trim().is_empty(),
                LoginPhase::Connecting | LoginPhase::VerifyingMfa => false,
            };
        let form_disabled = self.storage_loading || !matches!(self.login_phase, LoginPhase::Idle);

        let theme = Theme::global(cx).colors;
        let content = div().size_full().flex().flex_col().bg(theme.background).text_color(theme.foreground);

        #[cfg(target_os = "windows")]
        let content = content.child(windows_titlebar(window.is_maximized(), div().into_any_element(), div().into_any_element(), theme));

        #[cfg(target_os = "linux")]
        let content = {
            let titlebar_content = div().text_sm().font_semibold().text_color(theme.foreground).child("Radia").into_any_element();
            let titlebar_action = div().into_any_element();
            if let Some(titlebar) = linux_titlebar(window, titlebar_content, titlebar_action) { content.child(titlebar) } else { content }
        };

        let login_body = div().id("login-scroll").flex_1().min_h_0().overflow_y_scroll().child(
            div().w_full().flex().justify_center().px_4().py_6().child(
                div()
                    .w_full()
                    .max_w(px(420.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.muted)
                    .p_5()
                    .child(self.render_heading(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(self.render_credentials_form(form_disabled, cx))
                            .child(self.render_mfa_prompt(show_mfa, connecting, cx))
                            .child(self.render_spawn_selection(form_disabled, show_region_input, cx))
                            .child(self.render_grid_selection(form_disabled, cx))
                            .child(self.render_remember_control(form_disabled, cx)),
                    )
                    .child(self.render_status(form_disabled, cx))
                    .child(self.render_submit_button(can_submit, cx)),
            ),
        );

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        if self.active_session.as_ref().is_some_and(|active| active.session.is_some()) {
            content.child(div().flex_1().min_h_0().child(self.chat_screen.clone()))
        } else {
            content.child(login_body)
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        content.child(login_body)
    }
}
