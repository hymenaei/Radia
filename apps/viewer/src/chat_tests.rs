use std::collections::VecDeque;

use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext, point, px, size};

use super::{ChatMessage, ChatScreen, MAX_CHAT_HISTORY_BYTES, MAX_CHAT_HISTORY_MESSAGES, retain_message};

#[test]
fn chat_history_keeps_recent_messages() {
    let mut messages = VecDeque::new();
    let mut bytes = 0;
    for index in 0..MAX_CHAT_HISTORY_MESSAGES + 5 {
        assert!(retain_message(&mut messages, &mut bytes, ChatMessage { from_name: format!("Agent {index}"), message: "hello".into() },));
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

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| _ = window.draw(cx));
}

#[gpui::test]
fn chat_preserves_composer_and_scroll(cx: &mut TestAppContext) {
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
