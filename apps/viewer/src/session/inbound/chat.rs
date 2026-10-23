use network::AgentSession;
use protocol::{Audible, ChatSourceType, ChatType, messages::chat::chat_from_simulator::ChatData};

#[derive(Clone, Debug)]
pub(crate) struct ChatMessage {
    pub from_name: String,
    pub message: String,
}

pub(crate) fn display_message(chat: ChatData, session: Option<&AgentSession>) -> Option<ChatMessage> {
    let ChatData { from_name, source_id, source_type, chat_type, audible, message, .. } = chat;
    if audible != Audible::Fully
        || matches!(chat_type, ChatType::StartTyping | ChatType::StopTyping)
        || (source_type == ChatSourceType::Agent && session.is_some_and(|session| session.agent_id == source_id))
    {
        return None;
    }

    Some(ChatMessage { from_name: format_avatar_name(&from_name), message })
}

pub(crate) fn format_avatar_name(name: &str) -> String {
    let mut parts = name.split_whitespace().collect::<Vec<_>>();
    if parts.len() > 1 && parts.last().is_some_and(|part| part.eq_ignore_ascii_case("Resident")) {
        parts.pop();
    }

    parts.into_iter().map(|part| part.trim_matches('"')).filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use math::Vec3;
    use protocol::{Audible, ChatSourceType, ChatType, messages::chat::chat_from_simulator::ChatData};
    use uuid::Uuid;

    use super::display_message;

    fn chat(from_name: &str, audible: Audible, chat_type: ChatType, message: &str) -> ChatData {
        ChatData {
            from_name: from_name.into(),
            source_id: Uuid::nil(),
            owner_id: Uuid::nil(),
            source_type: ChatSourceType::Agent,
            chat_type,
            audible,
            position: Vec3::default(),
            message: message.into(),
        }
    }

    #[test]
    fn ignores_inaudible_chat() {
        for audible in [Audible::Barely, Audible::Not] {
            assert!(display_message(chat("Resident", audible, ChatType::Shout, ""), None).is_none());
            assert!(display_message(chat("Resident", audible, ChatType::Normal, "hello"), None).is_none());
        }
    }

    #[test]
    fn formats_avatar_names_and_hides_typing() {
        let one_name =
            display_message(chat("\"researched\" Resident", Audible::Fully, ChatType::Normal, "hello"), None).expect("fully audible chat");
        let two_names =
            display_message(chat("\"Ada\" Lovelace", Audible::Fully, ChatType::Normal, "hello"), None).expect("fully audible chat");

        assert_eq!(one_name.from_name, "researched");
        assert_eq!(two_names.from_name, "Ada Lovelace");

        for chat_type in [ChatType::StartTyping, ChatType::StopTyping] {
            assert!(display_message(chat("Resident", Audible::Fully, chat_type, ""), None).is_none());
        }
    }
}
