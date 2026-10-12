use base::UUID;
use math::{Quat, Vec3};
use network::OutboundMessage;
use protocol::{
    ChatType,
    lludp::Message,
    messages::{
        AgentUpdate, CacheMissType, ChatFromViewer, MapBlockRequest, ObjectRequest, RegionHandleRequest, RequestAgentData,
        RequestMultipleObjects, agent_update::AgentData, chat_from_viewer, chat_from_viewer::ChatData, map_block_request,
        map_block_request::PositionData, region_handle_request::RequestBlock,
    },
};

fn into_message<M: OutboundMessage>(message: M) -> Message {
    message.into()
}

#[test]
fn chat() {
    let message = ChatFromViewer {
        agent_data: chat_from_viewer::AgentData { agent_id: UUID::nil(), session_id: UUID::nil() },
        chat_data: ChatData { message: "hello".into(), chat_type: ChatType::Normal, channel: 0 },
    };

    assert!(matches!(into_message(message), Message::ChatFromViewer(_)));
}

#[test]
fn agent_update() {
    let message = AgentUpdate {
        agent_data: AgentData {
            agent_id: UUID::nil(),
            session_id: UUID::nil(),
            body_rotation: Quat::IDENTITY,
            head_rotation: Quat::IDENTITY,
            state: 0,
            camera_center: Vec3::default(),
            camera_at_axis: Vec3::default(),
            camera_left_axis: Vec3::default(),
            camera_up_axis: Vec3::default(),
            far: 0.0,
            control_flags: 0,
            flags: 0,
        },
    };

    assert!(matches!(into_message(message), Message::AgentUpdate(_)));
}

#[test]
fn map_request() {
    let message = MapBlockRequest {
        agent_data: map_block_request::AgentData { agent_id: UUID::nil(), session_id: UUID::nil(), flags: 0, estate_id: 0, godlike: false },
        position_data: PositionData { min_x: 1, max_x: 2, min_y: 3, max_y: 4 },
    };

    assert!(matches!(into_message(message), Message::MapBlockRequest(_)));
}

#[test]
fn region_request() {
    let message = RegionHandleRequest { request_block: RequestBlock { region_id: UUID::nil() } };

    assert!(matches!(into_message(message), Message::RegionHandleRequest(_)));
}

#[test]
fn object_cache_miss_request() {
    let message = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: UUID::nil(), session_id: UUID::nil() },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }],
    };

    assert!(matches!(into_message(message), Message::RequestMultipleObjects(_)));
}
