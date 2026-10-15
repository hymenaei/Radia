use protocol::{BlockInstance, BlockValues, FieldValue, lludp::Message};

pub(crate) fn schema_conforming_fields(message: &Message) -> Option<Vec<BlockValues>> {
    let one = |fields| BlockValues::single(fields);
    let many = |instances: Vec<Vec<FieldValue>>| BlockValues::new(instances.into_iter().map(BlockInstance::new).collect());

    match message {
        Message::UseCircuitCode(message) => Some(vec![one(vec![
            FieldValue::U32(message.circuit_code.code),
            FieldValue::UUID(message.circuit_code.session_id),
            FieldValue::UUID(message.circuit_code.agent_id),
        ])]),
        Message::CompleteAgentMovement(message) => Some(vec![one(vec![
            FieldValue::UUID(message.agent_data.agent_id),
            FieldValue::UUID(message.agent_data.session_id),
            FieldValue::U32(message.agent_data.circuit_code),
        ])]),
        Message::RegionHandshake(_) => None,
        Message::RegionHandshakeReply(message) => Some(vec![
            one(vec![FieldValue::UUID(message.agent_data.agent_id), FieldValue::UUID(message.agent_data.session_id)]),
            one(vec![FieldValue::U32(message.region_info.flags)]),
        ]),
        Message::AgentMovementComplete(_) => None,
        Message::AgentUpdate(message) => Some(vec![one(agent_update_fields(message))]),
        Message::AgentThrottle(message) => {
            let rates = message.throttle.throttles;
            let rate_bytes = [
                rates.resend_bits_per_second,
                rates.land_bits_per_second,
                rates.wind_bits_per_second,
                rates.cloud_bits_per_second,
                rates.task_bits_per_second,
                rates.texture_bits_per_second,
                rates.asset_bits_per_second,
            ]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
            Some(vec![
                one(vec![
                    FieldValue::UUID(message.agent_data.agent_id),
                    FieldValue::UUID(message.agent_data.session_id),
                    FieldValue::U32(message.agent_data.circuit_code),
                ]),
                one(vec![FieldValue::U32(message.throttle.gen_counter), FieldValue::Bytes(rate_bytes)]),
            ])
        }
        Message::StartPingCheck(message) => {
            Some(vec![one(vec![FieldValue::U8(message.ping_id.ping_id), FieldValue::U32(message.oldest_unacked)])])
        }
        Message::CompletePingCheck(message) => Some(vec![one(vec![FieldValue::U8(message.ping_id.ping_id)])]),
        Message::PacketAck(message) => Some(vec![many(message.packets.iter().map(|packet| vec![FieldValue::U32(packet.id)]).collect())]),
        Message::TeleportCancel(message) => Some(vec![one(vec![FieldValue::UUID(message.agent_id), FieldValue::UUID(message.session_id)])]),
        Message::LogoutRequest(message) => {
            Some(vec![one(vec![FieldValue::UUID(message.agent_data.agent_id), FieldValue::UUID(message.agent_data.session_id)])])
        }
        Message::LogoutReply(_) => None,
        Message::ChatFromViewer(_) => None,
        Message::ChatFromSimulator(_) => None,
        Message::RegionHandleRequest(message) => Some(vec![one(vec![FieldValue::UUID(message.request_block.region_id)])]),
        Message::RegionIdAndHandleReply(message) => {
            Some(vec![one(vec![FieldValue::UUID(message.reply_block.region_id), FieldValue::U64(message.reply_block.region_handle)])])
        }
        Message::MapBlockRequest(message) => Some(vec![
            one(vec![
                FieldValue::UUID(message.agent_data.agent_id),
                FieldValue::UUID(message.agent_data.session_id),
                FieldValue::U32(message.agent_data.flags),
                FieldValue::U32(message.agent_data.estate_id),
                FieldValue::Bool(message.agent_data.godlike),
            ]),
            one(vec![
                FieldValue::U16(message.position_data.min_x),
                FieldValue::U16(message.position_data.max_x),
                FieldValue::U16(message.position_data.min_y),
                FieldValue::U16(message.position_data.max_y),
            ]),
        ]),
        Message::MapBlockReply(_) => None,
        Message::EnableSimulator(message) => Some(vec![one(vec![
            FieldValue::U64(message.simulator_info.handle),
            FieldValue::IpAddr(message.simulator_info.ip.octets()),
            FieldValue::IpPort(message.simulator_info.port.to_be_bytes()),
        ])]),
        Message::DisableSimulator(_) => Some(vec![]),
        Message::ConfirmEnableSimulator(message) => {
            Some(vec![one(vec![FieldValue::UUID(message.agent_data.agent_id), FieldValue::UUID(message.agent_data.session_id)])])
        }
        Message::ObjectUpdate(_) => None,
        Message::ObjectUpdateCompressed(_) => None,
        Message::ImprovedTerseObjectUpdate(_) => None,
        Message::ObjectUpdateCached(message) => Some(vec![
            one(vec![FieldValue::U64(message.region_data.region_handle), FieldValue::U16(message.region_data.time_dilation)]),
            many(
                message
                    .objects
                    .iter()
                    .map(|object| vec![FieldValue::U32(object.local_id), FieldValue::U32(object.crc), FieldValue::U32(object.update_flags)])
                    .collect(),
            ),
        ]),
        Message::KillObject(message) => {
            Some(vec![many(message.object_data.iter().map(|object| vec![FieldValue::U32(object.local_id)]).collect())])
        }
        Message::RequestMultipleObjects(message) => Some(vec![
            one(vec![FieldValue::UUID(message.agent_data.agent_id), FieldValue::UUID(message.agent_data.session_id)]),
            many(
                message
                    .objects
                    .iter()
                    .map(|object| vec![FieldValue::U8(object.cache_miss_type.to_wire()), FieldValue::U32(object.local_id)])
                    .collect(),
            ),
        ]),
        Message::Schema(_) | Message::Unknown { .. } => None,
    }
}

pub(crate) fn agent_update_fields(message: &protocol::messages::AgentUpdate) -> Vec<FieldValue> {
    let data = &message.agent_data;
    vec![
        FieldValue::UUID(data.agent_id),
        FieldValue::UUID(data.session_id),
        FieldValue::Quaternion(data.body_rotation),
        FieldValue::Quaternion(data.head_rotation),
        FieldValue::U8(data.state),
        FieldValue::Vector3(data.camera_center),
        FieldValue::Vector3(data.camera_at_axis),
        FieldValue::Vector3(data.camera_left_axis),
        FieldValue::Vector3(data.camera_up_axis),
        FieldValue::F32(data.far),
        FieldValue::U32(data.control_flags),
        FieldValue::U8(data.flags),
    ]
}
