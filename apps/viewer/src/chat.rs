use std::collections::VecDeque;

use gpui::{Context, IntoElement, Render, TitlebarOptions, Window, WindowBounds, WindowKind, WindowOptions, div, prelude::*, px, size};
use gpui_component::{
    Disableable, IconName, Root, StyledExt, Theme,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
};
use network::{AgentSession, Delivery, SessionHandle};
use protocol::{
    ChatType,
    messages::{
        ChatFromViewer,
        chat_from_viewer::{AgentData, ChatData},
    },
};

use crate::{
    devtools::{DeveloperFloater, WorldSource},
    session::{ChatMessage, WorldReader, format_avatar_name},
    window_geometry,
};

const MAX_CHAT_HISTORY_MESSAGES: usize = 500;
const MAX_CHAT_HISTORY_BYTES: usize = 1024 * 1024;

pub struct ChatScreen {
    session: Option<AgentSession>,
    region_name: Option<String>,
    session_handle: Option<SessionHandle>,
    messages: VecDeque<ChatMessage>,
    message_bytes: usize,
    transcript_scroll: gpui::ScrollHandle,
    message_input: gpui::Entity<InputState>,
    capture_stop_pending: bool,
    capture_generation: u64,
    world_source: WorldSource,
    developer_window: Option<gpui::WindowHandle<Root>>,
}

impl ChatScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message_input = cx.new(|cx| InputState::new(window, cx).placeholder("To nearby chat"));

        Self {
            session: None,
            region_name: None,
            session_handle: None,
            messages: VecDeque::new(),
            message_bytes: 0,
            transcript_scroll: gpui::ScrollHandle::new(),
            message_input,
            capture_stop_pending: false,
            capture_generation: 0,
            world_source: WorldSource::new(),
            developer_window: None,
        }
    }

    pub fn set_session(&mut self, session: AgentSession, cx: &mut Context<Self>) {
        self.session = Some(session);
        self.region_name = None;
        cx.notify();
    }

    pub fn clear_session(&mut self, cx: &mut Context<Self>) {
        self.capture_generation = self.capture_generation.wrapping_add(1);
        self.capture_stop_pending = false;
        self.session = None;
        self.region_name = None;
        self.session_handle = None;
        self.messages.clear();
        self.message_bytes = 0;
        self.transcript_scroll = gpui::ScrollHandle::new();
        self.world_source.set(None);
        if let Some(window) = self.developer_window.take() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
        cx.notify();
    }

    pub fn set_world_state(&mut self, state: WorldReader, cx: &mut Context<Self>) {
        self.world_source.set(Some(state));
        cx.notify();
    }

    pub(crate) fn titlebar_content(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global(cx).colors;

        div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().text_sm().font_semibold().text_color(theme.foreground).child("Nearby Chat"))
            .into_any_element()
    }

    pub(crate) fn titlebar_action(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::global(cx).colors;
        let capture_status = self.session_handle.as_ref().and_then(SessionHandle::capture_status);
        let capture_visible = capture_status.is_some_and(|status| status.active || status.finalizing);
        let capture_saving = self.capture_stop_pending || capture_status.is_some_and(|status| status.finalizing);

        div()
            .flex()
            .items_center()
            .gap_1()
            .when(capture_visible, |this| {
                this.child(
                    Button::new("stop-capture")
                        .ghost()
                        .compact()
                        .h(px(24.))
                        .w(px(24.))
                        .when(capture_saving, |this| this.icon(IconName::LoaderCircle).loading(true))
                        .when(!capture_saving, |this| this.child(div().size(px(8.)).rounded(px(1.)).bg(theme.danger)))
                        .disabled(capture_saving)
                        .accessibility_label(if capture_saving { "Saving capture" } else { "Stop capture" })
                        .on_click(cx.listener(|this, _event, _window, cx| this.stop_capture(cx))),
                )
            })
            .child(
                Button::new("open-developer-tools")
                    .ghost()
                    .compact()
                    .h(px(24.))
                    .relative()
                    .top(px(-1.))
                    .accessibility_label("Developer Tools")
                    .child(div().text_sm().child("Developer Tools"))
                    .on_click(cx.listener(|this, _event, window, cx| this.open_developer_tools(window, cx))),
            )
            .into_any_element()
    }

    fn open_developer_tools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(existing) = self.developer_window
            && existing.update(cx, |_, window, _| window.activate_window()).is_ok()
        {
            return;
        }

        let minimum_size = size(px(900.), px(520.));
        let display = window.display(cx).map(|display| display.id());
        let bounds = window_geometry::restore_bounds("developer-tools", display, size(px(1080.), px(720.)), minimum_size, cx);
        let world_source = self.world_source.clone();
        match cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Developer Tools".into()),
                    appears_transparent: cfg!(target_os = "windows"),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(minimum_size),
                kind: WindowKind::Normal,
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| DeveloperFloater::new(world_source, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        ) {
            Ok(handle) => self.developer_window = Some(handle),
            Err(error) => tracing::warn!(%error, "could not open developer tools"),
        }
    }

    pub fn set_region_name(&mut self, region_name: String, cx: &mut Context<Self>) {
        self.region_name = Some(region_name);
        cx.notify();
    }

    pub fn set_session_handle(&mut self, session_handle: SessionHandle, cx: &mut Context<Self>) {
        self.capture_generation = self.capture_generation.wrapping_add(1);
        self.capture_stop_pending = false;
        self.session_handle = Some(session_handle);
        cx.notify();
    }

    fn stop_capture(&mut self, cx: &mut Context<Self>) {
        if self.capture_stop_pending {
            return;
        }

        let Some(session_handle) = self.session_handle.clone() else { return };
        if !session_handle.capture_status().is_some_and(|status| status.active) {
            return;
        }

        self.capture_stop_pending = true;
        let generation = self.capture_generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            match session_handle.stop_capture().await {
                Ok(path) => {
                    tracing::info!(operation = "capture_stop", outcome = "saved", capture_path = %path.display(), "Capture finished")
                }
                Err(error) => tracing::error!(operation = "capture_stop", outcome = "failed", %error, "Could not stop capture"),
            }
            let _ = this.update(cx, |this, cx| {
                if this.capture_generation != generation {
                    return;
                }
                this.capture_stop_pending = false;
                cx.notify();
            });
        })
        .detach();
    }

    pub fn add_chat_messages(&mut self, messages: impl IntoIterator<Item = ChatMessage>, cx: &mut Context<Self>) {
        let mut added_message = false;
        for message in messages {
            added_message |= self.push_message(message);
        }
        if added_message {
            cx.notify();
        }
    }

    fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let message = self.message_input.read(cx).value().trim().to_owned();
        if message.is_empty() {
            return;
        }

        let result = match (self.session_handle.as_ref(), self.session.as_ref()) {
            (Some(session_handle), Some(session)) => session_handle
                .send(
                    ChatFromViewer {
                        agent_data: AgentData { agent_id: session.agent_id, session_id: session.session_id },
                        chat_data: ChatData { message: message.clone(), chat_type: ChatType::Normal, channel: 0 },
                    },
                    Delivery::Reliable,
                )
                .map_err(|error| error.to_string()),
            _ => Err("Connect to a simulator before sending nearby chat.".into()),
        };

        match result {
            Ok(()) => {
                let from_name = self
                    .session
                    .as_ref()
                    .map(|session| format_avatar_name(&format!("{} {}", session.first_name, session.last_name)))
                    .unwrap_or_else(|| "You".into());
                self.push_message(ChatMessage { from_name, message });
                self.message_input.update(cx, |input, cx| input.set_value("", window, cx));
                cx.notify();
            }
            Err(error) => {
                tracing::warn!(operation = "nearby_chat_send", %error, "Send failed");
            }
        }
    }

    fn push_message(&mut self, message: ChatMessage) -> bool {
        if !retain_message(&mut self.messages, &mut self.message_bytes, message) {
            return false;
        }

        let max_offset = self.transcript_scroll.max_offset().y;
        let was_at_bottom = self.transcript_scroll.offset().y <= -max_offset + px(2.);

        if was_at_bottom {
            self.transcript_scroll.scroll_to_bottom();
        }
        true
    }
}

fn retain_message(messages: &mut VecDeque<ChatMessage>, message_bytes: &mut usize, message: ChatMessage) -> bool {
    let size = message.from_name.len().saturating_add(message.message.len());
    if size > MAX_CHAT_HISTORY_BYTES {
        return false;
    }

    while messages.len() >= MAX_CHAT_HISTORY_MESSAGES || message_bytes.saturating_add(size) > MAX_CHAT_HISTORY_BYTES {
        let Some(removed) = messages.pop_front() else {
            break;
        };
        *message_bytes = message_bytes.saturating_sub(removed.from_name.len().saturating_add(removed.message.len()));
    }

    *message_bytes += size;
    messages.push_back(message);
    true
}

impl Render for ChatScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::global(cx).colors;

        let scroll_handle = self.transcript_scroll.clone();
        let transcript = div()
            .flex_1()
            .min_h_0()
            .id("chat-transcript")
            .debug_selector(|| "chat-transcript-viewport".to_owned())
            .track_scroll(&scroll_handle)
            .overflow_y_scroll()
            .scrollbar_width(px(0.))
            .child(div().flex_none().h_auto().min_h_full().flex().flex_col().px_2().child(div().flex_1()).children(
                self.messages.iter().map(|message| {
                    div()
                        .flex()
                        .items_start()
                        .gap_1()
                        .py_1()
                        .text_sm()
                        .child(div().flex_none().text_color(theme.muted_foreground).child(format!("{}:", message.from_name)))
                        .child(div().min_w_0().flex_1().text_color(theme.foreground).child(message.message.clone()))
                }),
            ))
            .into_any_element();

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .debug_selector(|| "chat-screen".to_owned())
            .child(transcript)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .bg(theme.sidebar)
                    .border_t_1()
                    .border_color(theme.border)
                    .debug_selector(|| "chat-composer".to_owned())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(
                                Input::new(&self.message_input).focus_bordered(false).aria_label("To nearby chat").h(px(38.)).w_full(),
                            ))
                            .child(
                                Button::new("send-nearby-chat")
                                    .label("Send")
                                    .primary()
                                    .h(px(32.))
                                    .on_click(cx.listener(|this, _event, window, cx| this.send_message(window, cx))),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use gpui::{
        AppContext, Context, Entity, IntoElement, Render, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, Window, div,
        point, prelude::*, px, size,
    };

    use super::{ChatMessage, ChatScreen, MAX_CHAT_HISTORY_BYTES, MAX_CHAT_HISTORY_MESSAGES, retain_message};

    struct ChatActionHarness {
        chat: Entity<ChatScreen>,
    }

    impl Render for ChatActionHarness {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let action = self.chat.update(cx, |chat, cx| chat.titlebar_action(cx));
            div().size_full().child(div().debug_selector(|| "chat-titlebar-action".to_owned()).child(action))
        }
    }

    #[test]
    fn chat_history_keeps_recent_messages() {
        let mut messages = VecDeque::new();
        let mut bytes = 0;
        for index in 0..MAX_CHAT_HISTORY_MESSAGES + 5 {
            assert!(retain_message(
                &mut messages,
                &mut bytes,
                ChatMessage { from_name: format!("Agent {index}"), message: "hello".into() },
            ));
        }

        assert_eq!(messages.len(), MAX_CHAT_HISTORY_MESSAGES);
        assert_eq!(messages.front().map(|message| message.from_name.as_str()), Some("Agent 5"));
        assert_eq!(messages.back().map(|message| message.from_name.as_str()), Some("Agent 504"));
        assert!(bytes <= MAX_CHAT_HISTORY_BYTES);
    }

    #[test]
    fn chat_history_bounds_bytes() {
        let mut messages = VecDeque::new();
        let mut bytes = 0;
        for index in 0..2 {
            assert!(retain_message(
                &mut messages,
                &mut bytes,
                ChatMessage { from_name: format!("Agent {index}"), message: "x".repeat(600 * 1024) },
            ));
        }

        assert_eq!(messages.len(), 1);
        assert_eq!(messages.front().map(|message| message.from_name.as_str()), Some("Agent 1"));
        assert_eq!(bytes, messages.front().unwrap().from_name.len() + 600 * 1024);

        assert!(!retain_message(
            &mut messages,
            &mut bytes,
            ChatMessage { from_name: "Agent".into(), message: "x".repeat(MAX_CHAT_HISTORY_BYTES) },
        ));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages.front().map(|message| message.from_name.as_str()), Some("Agent 1"));
    }

    #[cfg(not(target_os = "macos"))]
    #[gpui::test]
    fn developer_action_activates_existing_window(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (harness, cx) = cx.add_window_view(|window, cx| {
            let chat = cx.new(|cx| ChatScreen::new(window, cx));
            ChatActionHarness { chat }
        });
        let chat = cx.update(|_, app| harness.read(app).chat.clone());
        draw(cx);
        let button = cx.debug_bounds("chat-titlebar-action").expect("titlebar action should render");
        let click = point(button.origin.x + px(8.), button.origin.y + px(8.));

        cx.simulate_click(click, Default::default());
        assert_eq!(cx.windows().len(), 2);

        let windows = cx.windows();
        let main_window = windows[0];
        let developer_window = windows.into_iter().find(|window| *window != main_window).expect("Developer Tools opens a second window");
        cx.simulate_click(click, Default::default());
        assert_eq!(cx.windows().len(), 2);
        assert_eq!(cx.app.borrow().active_window(), Some(developer_window));

        cx.update(|_, app| chat.update(app, |chat, cx| chat.clear_session(cx)));
        assert_eq!(cx.windows().len(), 1);
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| _ = window.draw(cx));
    }

    #[gpui::test]
    fn chat_preserves_composer_and_conditional_scroll(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (chat, cx) = cx.add_window_view(ChatScreen::new);
        draw(cx);
        let blank_transcript_height = cx.update(|_, app| chat.read(app).transcript_scroll.bounds().size.height);
        assert!(blank_transcript_height > px(0.), "the empty transcript should stay attached to the scroll handle");

        cx.update(|_, app| {
            chat.update(app, |chat, cx| {
                chat.add_chat_messages([ChatMessage { from_name: "Ada".into(), message: "hello".into() }], cx);
            });
        });
        draw(cx);

        let screen = cx.debug_bounds("chat-screen").unwrap();
        let composer = cx.debug_bounds("chat-composer").unwrap();
        let short_max = cx.update(|_, app| chat.read(app).transcript_scroll.max_offset().y);
        assert_eq!(short_max, px(0.));
        assert_eq!(composer.bottom(), screen.bottom());

        for index in 0..56 {
            cx.update(|_, app| {
                chat.update(app, |chat, cx| {
                    chat.add_chat_messages([ChatMessage { from_name: format!("Agent {index}"), message: format!("Message {index}") }], cx);
                });
            });
            draw(cx);
            let (offset, max) = cx.update(|_, app| {
                let scroll = chat.read(app).transcript_scroll.clone();
                (scroll.offset().y, scroll.max_offset().y)
            });
            if max > px(0.) {
                assert_eq!(offset, -max, "live chat should follow the newest message while at the bottom");
            }
        }

        let screen = cx.debug_bounds("chat-screen").unwrap();
        let composer = cx.debug_bounds("chat-composer").unwrap();
        let (offset, max) = cx.update(|_, app| {
            let scroll = chat.read(app).transcript_scroll.clone();
            (scroll.offset().y, scroll.max_offset().y)
        });
        assert_eq!(composer.bottom(), screen.bottom());
        assert!(max > px(0.));
        assert_eq!(offset, -max);

        cx.update(|_, app| chat.read(app).transcript_scroll.set_offset(point(px(0.), px(0.))));
        draw(cx);
        let transcript = cx.debug_bounds("chat-transcript-viewport").unwrap();
        cx.simulate_event(ScrollWheelEvent {
            position: point(transcript.origin.x + px(8.), transcript.origin.y + px(12.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-5000.))),
            ..Default::default()
        });
        draw(cx);
        let offset = cx.update(|_, app| chat.read(app).transcript_scroll.offset().y);
        assert_eq!(offset, -max, "scrolling to the bottom should reach the latest messages");

        cx.simulate_event(ScrollWheelEvent {
            position: point(transcript.origin.x + px(8.), transcript.origin.y + px(12.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(120.))),
            ..Default::default()
        });
        draw(cx);
        let offset = cx.update(|_, app| chat.read(app).transcript_scroll.offset().y);
        assert!(offset > -max && offset < px(0.), "scrolling up should move away from the latest messages");
        let reading_position = offset;

        cx.update(|_, app| {
            chat.update(app, |chat, cx| {
                chat.add_chat_messages([ChatMessage { from_name: "New agent".into(), message: "later".into() }], cx);
            });
        });
        draw(cx);

        let offset = cx.update(|_, app| chat.read(app).transcript_scroll.offset().y);
        assert_eq!(offset, reading_position, "new chat should preserve an earlier reading position");

        cx.update(|_, app| {
            chat.update(app, |chat, cx| {
                chat.clear_session(cx);
                chat.add_chat_messages(
                    (0..80).map(|index| ChatMessage { from_name: format!("New session {index}"), message: "hello".into() }),
                    cx,
                );
            });
        });
        draw(cx);
        let (new_session_offset, new_session_max) = cx.update(|_, app| {
            let scroll = chat.read(app).transcript_scroll.clone();
            (scroll.offset().y, scroll.max_offset().y)
        });
        assert!(new_session_max > px(0.));
        assert_eq!(new_session_offset, -new_session_max, "a new session should start at its latest message");

        cx.simulate_resize(size(px(620.), px(320.)));
        draw(cx);
        let screen = cx.debug_bounds("chat-screen").unwrap();
        let composer = cx.debug_bounds("chat-composer").unwrap();
        assert_eq!(composer.bottom(), screen.bottom());
    }
}
