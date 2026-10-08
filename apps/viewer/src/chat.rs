use std::collections::VecDeque;

use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px, rgb};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    scroll::ScrollableElement,
};
use network::{AgentSession, Delivery, SessionHandle};
use protocol::{
    Audible, ChatSourceType, ChatType,
    messages::{
        ChatFromViewer,
        chat_from_simulator::ChatData,
        chat_from_viewer::{AgentData, ChatData as ViewerChatData},
    },
};

const MAX_CHAT_HISTORY_MESSAGES: usize = 500;
const MAX_CHAT_HISTORY_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct ChatMessage {
    pub from_name: String,
    pub message: String,
}

pub struct ChatScreen {
    session: Option<AgentSession>,
    region_name: Option<String>,
    session_handle: Option<SessionHandle>,
    messages: VecDeque<ChatMessage>,
    message_bytes: usize,
    transcript_scroll: gpui::ScrollHandle,
    message_input: gpui::Entity<InputState>,
    send_error: Option<String>,
    connection_error: Option<String>,
    favorite_status: Option<String>,
}

impl ChatScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message_input = cx.new(|cx| InputState::new(window, cx).placeholder("Message nearby chat"));

        Self {
            session: None,
            region_name: None,
            session_handle: None,
            messages: VecDeque::new(),
            message_bytes: 0,
            transcript_scroll: gpui::ScrollHandle::new(),
            message_input,
            send_error: None,
            connection_error: None,
            favorite_status: None,
        }
    }

    pub fn set_session(&mut self, session: AgentSession, cx: &mut Context<Self>) {
        self.session = Some(session);
        self.region_name = None;
        self.send_error = None;
        self.connection_error = None;
        cx.notify();
    }

    pub fn clear_session(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.region_name = None;
        self.session_handle = None;
        self.messages.clear();
        self.message_bytes = 0;
        self.send_error = None;
        self.connection_error = None;
        self.favorite_status = None;
        cx.notify();
    }

    pub fn set_region_name(&mut self, region_name: String, cx: &mut Context<Self>) {
        self.region_name = Some(region_name);
        cx.notify();
    }

    pub fn set_session_handle(&mut self, session_handle: SessionHandle, cx: &mut Context<Self>) {
        self.session_handle = Some(session_handle);
        cx.notify();
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

    pub fn report_send_error(&mut self, error: impl Into<String>, cx: &mut Context<Self>) {
        self.send_error = Some(error.into());
        cx.notify();
    }

    pub fn report_connection_error(&mut self, error: impl Into<String>, cx: &mut Context<Self>) {
        self.connection_error = Some(error.into());
        cx.notify();
    }

    pub fn report_favorite_status(&mut self, status: impl Into<String>, cx: &mut Context<Self>) {
        self.favorite_status = Some(status.into());
        cx.notify();
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
                        chat_data: ViewerChatData { message: message.clone(), chat_type: ChatType::Normal, channel: 0 },
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
                    .map(|session| format!("{} {}", session.first_name, session.last_name))
                    .unwrap_or_else(|| "You".into());
                self.push_message(ChatMessage { from_name, message });
                self.message_input.update(cx, |input, cx| input.set_value("", window, cx));
                self.send_error = None;
            }
            Err(error) => self.send_error = Some(error),
        }
        cx.notify();
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

pub(crate) fn display_message(chat: ChatData, session: Option<&AgentSession>) -> Option<ChatMessage> {
    let ChatData { from_name, source_id, source_type, chat_type, audible, message, .. } = chat;
    if audible != Audible::Fully
        || matches!(chat_type, ChatType::StartTyping | ChatType::StopTyping)
        || (source_type == ChatSourceType::Agent && session.is_some_and(|session| session.agent_id == source_id))
    {
        return None;
    }

    Some(ChatMessage { from_name, message })
}

impl Render for ChatScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let account = self
            .session
            .as_ref()
            .map(|session| format!("{} {}", session.first_name, session.last_name))
            .unwrap_or_else(|| "Waiting for session".into());
        let grid = self.session.as_ref().map(|session| session.grid.label()).unwrap_or("—");
        let location = self
            .session
            .as_ref()
            .map(|session| self.region_name.clone().unwrap_or_else(|| session.simulator_address.to_string()))
            .unwrap_or_else(|| "Not connected".into());

        let scroll_handle = self.transcript_scroll.clone();
        let transcript = if self.messages.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.))
                .text_color(rgb(0x8f8f8f))
                .child("Nearby messages will appear here.")
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .id("chat-transcript")
                .track_scroll(&scroll_handle)
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_2()
                .px_4()
                .py_3()
                .children(self.messages.iter().map(|message| {
                    div()
                        .w_full()
                        .flex()
                        .items_start()
                        .gap_3()
                        .p_3()
                        .bg(rgb(0x252525))
                        .rounded_md()
                        .child(div().w(px(132.)).text_size(px(13.)).text_color(rgb(0x9bc7e5)).child(message.from_name.clone()))
                        .child(div().flex_1().text_size(px(14.)).text_color(rgb(0xe6e6e6)).child(message.message.clone()))
                }))
                .vertical_scrollbar(&scroll_handle)
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x202020))
            .text_color(rgb(0xe6e6e6))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .bg(rgb(0x252525))
                    .border_b_1()
                    .border_color(rgb(0x383838))
                    .child(div().text_size(px(17.)).child("Nearby chat"))
                    .child(
                        div()
                            .flex()
                            .gap_6()
                            .child(header_item("ACCOUNT", account))
                            .child(header_item("GRID", grid))
                            .child(header_item("LOCATION", location)),
                    )
                    .when_some(self.favorite_status.clone(), |this, status| {
                        this.child(div().text_size(px(12.)).text_color(rgb(0xb4b4b4)).child(status))
                    }),
            )
            .child(transcript)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .bg(rgb(0x252525))
                    .border_t_1()
                    .border_color(rgb(0x383838))
                    .child(div().text_size(px(12.)).text_color(rgb(0xb4b4b4)).child("Message"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.message_input).h(px(40.)).w_full()))
                            .child(
                                Button::new("send-nearby-chat")
                                    .label("Send")
                                    .primary()
                                    .h(px(40.))
                                    .on_click(cx.listener(|this, _event, window, cx| this.send_message(window, cx))),
                            ),
                    )
                    .when_some(self.send_error.clone(), |this, error| {
                        this.child(div().text_size(px(12.)).text_color(rgb(0xf0a0a0)).child(format!("Send failed: {error}")))
                    })
                    .when_some(self.connection_error.clone(), |this, error| {
                        this.child(div().text_size(px(12.)).text_color(rgb(0xf0a0a0)).child(format!("Disconnected: {error}")))
                    }),
            )
    }
}

fn header_item(label: &'static str, value: impl Into<String>) -> impl IntoElement {
    let value = value.into();

    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_size(px(10.)).text_color(rgb(0x8f8f8f)).child(label))
        .child(div().text_size(px(12.)).text_color(rgb(0xd0d0d0)).child(value))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use base::UUID;
    use protocol::{Audible, ChatSourceType, ChatType, Vector3, messages::chat_from_simulator::ChatData};

    use super::{ChatMessage, MAX_CHAT_HISTORY_BYTES, MAX_CHAT_HISTORY_MESSAGES, display_message, retain_message};

    fn chat(audible: Audible, chat_type: ChatType, message: &str) -> ChatData {
        ChatData {
            from_name: "Resident".into(),
            source_id: UUID::nil(),
            owner_id: UUID::nil(),
            source_type: ChatSourceType::Agent,
            chat_type,
            audible,
            position: Vector3::default(),
            message: message.into(),
        }
    }

    #[test]
    fn ignores_inaudible() {
        for audible in [Audible::Barely, Audible::Not] {
            assert!(display_message(chat(audible, ChatType::Shout, ""), None).is_none());
            assert!(display_message(chat(audible, ChatType::Normal, "hello"), None).is_none());
        }
    }

    #[test]
    fn keeps_empty_audible() {
        let message = display_message(chat(Audible::Fully, ChatType::Shout, ""), None).expect("fully audible chat");
        assert!(message.message.is_empty());
    }

    #[test]
    fn hides_typing() {
        for chat_type in [ChatType::StartTyping, ChatType::StopTyping] {
            assert!(display_message(chat(Audible::Fully, chat_type, ""), None).is_none());
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
}
