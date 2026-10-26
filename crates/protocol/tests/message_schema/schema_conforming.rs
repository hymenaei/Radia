use protocol::{
    BlockInstance, BlockValues, FieldValue,
    lludp::Message,
    messages::{InventoryItemBlock, MoveInventoryFolderBlock, MoveInventoryItemBlock},
};

pub(crate) fn schema_conforming_fields(message: &Message) -> Option<Vec<BlockValues>> {
    let one = |fields| BlockValues::single(fields);
    let many = |instances: Vec<Vec<FieldValue>>| BlockValues::new(instances.into_iter().map(BlockInstance::new).collect());
    let ll_string = |value: &str| {
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        FieldValue::Bytes(bytes)
    };
    let item_fields = |item: &InventoryItemBlock, bulk: bool| {
        let mut fields = vec![FieldValue::Uuid(item.item_id)];
        if bulk {
            fields.push(FieldValue::U32(item.callback_id));
            fields.push(FieldValue::Uuid(item.folder_id));
        } else {
            fields.push(FieldValue::Uuid(item.folder_id));
            fields.push(FieldValue::U32(item.callback_id));
        }
        fields.extend([
            FieldValue::Uuid(item.creator_id),
            FieldValue::Uuid(item.owner_id),
            FieldValue::Uuid(item.group_id),
            FieldValue::U32(item.base_mask),
            FieldValue::U32(item.owner_mask),
            FieldValue::U32(item.group_mask),
            FieldValue::U32(item.everyone_mask),
            FieldValue::U32(item.next_owner_mask),
            FieldValue::Bool(item.group_owned),
            FieldValue::Uuid(item.asset_id),
            FieldValue::S8(item.asset_type),
            FieldValue::S8(item.inventory_type),
            FieldValue::U32(item.flags),
            FieldValue::U8(item.sale_type),
            FieldValue::S32(item.sale_price),
            ll_string(&item.name),
            ll_string(&item.description),
            FieldValue::S32(item.creation_date),
            FieldValue::U32(item.crc),
        ]);
        fields
    };

    match message {
        Message::UseCircuitCode(message) => Some(vec![one(vec![
            FieldValue::U32(message.circuit_code.code),
            FieldValue::Uuid(message.circuit_code.session_id),
            FieldValue::Uuid(message.circuit_code.agent_id),
        ])]),
        Message::CompleteAgentMovement(message) => Some(vec![one(vec![
            FieldValue::Uuid(message.agent_data.agent_id),
            FieldValue::Uuid(message.agent_data.session_id),
            FieldValue::U32(message.agent_data.circuit_code),
        ])]),
        Message::RegionHandshake(_) => None,
        Message::RegionHandshakeReply(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.agent_data.agent_id), FieldValue::Uuid(message.agent_data.session_id)]),
            one(vec![FieldValue::U32(message.region_info.flags)]),
        ]),
        Message::RegionInfo(message) => {
            let info = &message.region_info;
            let info2 = &message.region_info2;
            let region_info3 = message
                .region_info3
                .iter()
                .map(|info| Some(vec![FieldValue::U64(info.region_flags_extended?)]))
                .collect::<Option<Vec<_>>>()?;
            let region_info5 = message
                .region_info5
                .iter()
                .map(|info| {
                    Some(vec![
                        FieldValue::F32(info.chat_whisper_range?),
                        FieldValue::F32(info.chat_normal_range?),
                        FieldValue::F32(info.chat_shout_range?),
                        FieldValue::F32(info.chat_whisper_offset?),
                        FieldValue::F32(info.chat_normal_offset?),
                        FieldValue::F32(info.chat_shout_offset?),
                        FieldValue::U32(info.chat_flags?),
                    ])
                })
                .collect::<Option<Vec<_>>>()?;
            let combat_settings = message
                .combat_settings
                .iter()
                .map(|settings| {
                    Some(vec![
                        FieldValue::U32(settings.combat_flags?),
                        FieldValue::U8(settings.on_death?),
                        FieldValue::F32(settings.damage_throttle?),
                        FieldValue::F32(settings.regeneration_rate?),
                        FieldValue::F32(settings.invulnerability_time?),
                        FieldValue::F32(settings.damage_limit?),
                    ])
                })
                .collect::<Option<Vec<_>>>()?;
            Some(vec![
                one(vec![FieldValue::Uuid(message.agent_data.agent_id), FieldValue::Uuid(message.agent_data.session_id)]),
                one(vec![
                    FieldValue::Bytes(info.sim_name.clone()),
                    FieldValue::U32(info.estate_id?),
                    FieldValue::U32(info.parent_estate_id?),
                    FieldValue::U32(info.region_flags?),
                    FieldValue::U8(info.sim_access?),
                    FieldValue::U8(info.max_agents?),
                    FieldValue::F32(info.billable_factor?),
                    FieldValue::F32(info.object_bonus_factor?),
                    FieldValue::F32(info.water_height?),
                    FieldValue::F32(info.terrain_raise_limit?),
                    FieldValue::F32(info.terrain_lower_limit?),
                    FieldValue::S32(info.price_per_meter?),
                    FieldValue::S32(info.redirect_grid_x?),
                    FieldValue::S32(info.redirect_grid_y?),
                    FieldValue::Bool(info.use_estate_sun?),
                    FieldValue::F32(info.sun_hour?),
                ]),
                one(vec![
                    FieldValue::Bytes(info2.product_sku.clone()),
                    FieldValue::Bytes(info2.product_name.clone()),
                    FieldValue::U32(info2.max_agents32?),
                    FieldValue::U32(info2.hard_max_agents?),
                    FieldValue::U32(info2.hard_max_objects?),
                ]),
                many(region_info3),
                many(region_info5),
                many(combat_settings),
            ])
        }
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
                    FieldValue::Uuid(message.agent_data.agent_id),
                    FieldValue::Uuid(message.agent_data.session_id),
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
        Message::TeleportCancel(message) => Some(vec![one(vec![FieldValue::Uuid(message.agent_id), FieldValue::Uuid(message.session_id)])]),
        Message::TeleportFinish(message) => Some(vec![one(vec![
            FieldValue::Uuid(message.agent_id),
            FieldValue::U32(message.location_id),
            FieldValue::IpAddr(message.sim_ip),
            FieldValue::IpPort(message.sim_port),
            FieldValue::U64(message.region_handle),
            FieldValue::Bytes(message.seed_capability.clone()),
            FieldValue::U8(message.sim_access?),
            FieldValue::U32(message.teleport_flags?),
        ])]),
        Message::TeleportFailed(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.agent_id), FieldValue::Bytes(message.reason.clone())]),
            many(
                message
                    .alert_info
                    .iter()
                    .map(|info| vec![FieldValue::Bytes(info.message.clone()), FieldValue::Bytes(info.extra_params.clone())])
                    .collect(),
            ),
        ]),
        Message::LogoutRequest(message) => {
            Some(vec![one(vec![FieldValue::Uuid(message.agent_data.agent_id), FieldValue::Uuid(message.agent_data.session_id)])])
        }
        Message::LogoutReply(_) => None,
        Message::ChatFromViewer(_) => None,
        Message::ChatFromSimulator(_) => None,
        Message::RegionHandleRequest(message) => Some(vec![one(vec![FieldValue::Uuid(message.request_block.region_id)])]),
        Message::RegionIdAndHandleReply(message) => {
            Some(vec![one(vec![FieldValue::Uuid(message.reply_block.region_id), FieldValue::U64(message.reply_block.region_handle)])])
        }
        Message::MapBlockRequest(message) => Some(vec![
            one(vec![
                FieldValue::Uuid(message.agent_data.agent_id),
                FieldValue::Uuid(message.agent_data.session_id),
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
            Some(vec![one(vec![FieldValue::Uuid(message.agent_data.agent_id), FieldValue::Uuid(message.agent_data.session_id)])])
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
            one(vec![FieldValue::Uuid(message.agent_data.agent_id), FieldValue::Uuid(message.agent_data.session_id)]),
            many(
                message
                    .objects
                    .iter()
                    .map(|object| vec![FieldValue::U8(object.cache_miss_type.to_wire()), FieldValue::U32(object.local_id)])
                    .collect(),
            ),
        ]),
        Message::ObjectExtraParams(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.agent_id), FieldValue::Uuid(message.session_id)]),
            many(
                message
                    .object_data
                    .iter()
                    .map(|update| {
                        vec![
                            FieldValue::U32(update.local_id),
                            FieldValue::U16(update.parameter_type),
                            FieldValue::Bool(update.in_use),
                            FieldValue::U32(u32::try_from(update.data.len()).expect("fixture ParamSize fits")),
                            FieldValue::Bytes(update.data.clone()),
                        ]
                    })
                    .collect(),
            ),
        ]),
        Message::GenericStreamingMessage(message) => {
            Some(vec![one(vec![FieldValue::U16(message.method)]), one(vec![FieldValue::Bytes(message.data.clone())])])
        }
        Message::AvatarAppearance(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.sender.id), FieldValue::Bool(message.sender.is_trial)]),
            one(vec![FieldValue::Bytes(message.object_data.texture_entry.clone())]),
            many(message.visual_params.iter().map(|value| vec![FieldValue::U8(value.param_value)]).collect()),
            many(
                message
                    .appearance_data
                    .iter()
                    .map(|value| {
                        vec![FieldValue::U8(value.appearance_version), FieldValue::S32(value.cof_version), FieldValue::U32(value.flags)]
                    })
                    .collect(),
            ),
            many(message.appearance_hover.iter().map(|value| vec![FieldValue::Vector3(value.hover_height)]).collect()),
            many(
                message
                    .attachment_blocks
                    .as_ref()
                    .into_iter()
                    .flatten()
                    .map(|value| vec![FieldValue::Uuid(value.id), FieldValue::U8(value.attachment_point)])
                    .collect(),
            ),
        ]),
        Message::AvatarSitResponse(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.sit_object.id?)]),
            one(vec![
                FieldValue::Bool(message.sit_transform.auto_pilot?),
                FieldValue::Vector3(message.sit_transform.sit_position?),
                FieldValue::Quaternion(message.sit_transform.sit_rotation?),
                FieldValue::Vector3(message.sit_transform.camera_eye_offset?),
                FieldValue::Vector3(message.sit_transform.camera_at_offset?),
                FieldValue::Bool(message.sit_transform.force_mouselook?),
            ]),
        ]),
        Message::ParcelProperties(message) => {
            let data = &message.parcel_data;
            Some(vec![
                one(vec![
                    FieldValue::S32(data.request_result?),
                    FieldValue::S32(data.sequence_id?),
                    FieldValue::Bool(data.snap_selection?),
                    FieldValue::S32(data.self_count?),
                    FieldValue::S32(data.other_count?),
                    FieldValue::S32(data.public_count?),
                    FieldValue::S32(data.local_id?),
                    FieldValue::Uuid(data.owner_id?),
                    FieldValue::Bool(data.is_group_owned?),
                    FieldValue::U32(data.auction_id?),
                    FieldValue::S32(data.claim_date?),
                    FieldValue::S32(data.claim_price?),
                    FieldValue::S32(data.rent_price?),
                    FieldValue::Vector3(data.aabb_min?),
                    FieldValue::Vector3(data.aabb_max?),
                    FieldValue::Bytes(data.bitmap.clone()?),
                    FieldValue::S32(data.area?),
                    FieldValue::U8(data.status?),
                    FieldValue::S32(data.sim_wide_max_prims?),
                    FieldValue::S32(data.sim_wide_total_prims?),
                    FieldValue::S32(data.max_prims?),
                    FieldValue::S32(data.total_prims?),
                    FieldValue::S32(data.owner_prims?),
                    FieldValue::S32(data.group_prims?),
                    FieldValue::S32(data.other_prims?),
                    FieldValue::S32(data.selected_prims?),
                    FieldValue::F32(data.parcel_prim_bonus?),
                    FieldValue::S32(data.other_clean_time?),
                    FieldValue::U32(data.parcel_flags?),
                    FieldValue::S32(data.sale_price?),
                    FieldValue::Bytes(data.name.clone()?),
                    FieldValue::Bytes(data.description.clone()?),
                    FieldValue::Bytes(data.music_url.clone()?),
                    FieldValue::Bytes(data.media_url.clone()?),
                    FieldValue::Uuid(data.media_id?),
                    FieldValue::U8(data.media_auto_scale?),
                    FieldValue::Uuid(data.group_id?),
                    FieldValue::S32(data.pass_price?),
                    FieldValue::F32(data.pass_hours?),
                    FieldValue::U8(data.category?),
                    FieldValue::Uuid(data.auth_buyer_id?),
                    FieldValue::Uuid(data.snapshot_id?),
                    FieldValue::Vector3(data.user_location?),
                    FieldValue::Vector3(data.user_look_at?),
                    FieldValue::U8(data.landing_type?),
                    FieldValue::Bool(data.region_push_override?),
                    FieldValue::Bool(data.region_deny_anonymous?),
                    FieldValue::Bool(data.region_deny_identified?),
                    FieldValue::Bool(data.region_deny_transacted?),
                ]),
                one(vec![FieldValue::Bool(message.age_verification_block.region_deny_age_unverified?)]),
                one(vec![FieldValue::Bool(message.region_allow_access_block.region_allow_access_override?)]),
                one(vec![
                    FieldValue::S32(message.parcel_environment_block.parcel_environment_version?),
                    FieldValue::Bool(message.parcel_environment_block.region_allow_environment_override?),
                ]),
            ])
        }
        Message::GenericMessage(message) => Some(vec![
            one(vec![
                FieldValue::Uuid(message.agent_data.agent_id),
                FieldValue::Uuid(message.agent_data.session_id),
                FieldValue::Uuid(message.agent_data.transaction_id),
            ]),
            one(vec![FieldValue::Bytes(message.method_data.method.clone()), FieldValue::Uuid(message.method_data.invoice?)]),
            many(message.param_list.as_ref().into_iter().flatten().map(|value| vec![FieldValue::Bytes(value.parameter.clone())]).collect()),
        ]),
        Message::LayerData(message) => {
            Some(vec![one(vec![FieldValue::U8(message.layer_type)]), one(vec![FieldValue::Bytes(message.data.clone())])])
        }
        Message::UpdateCreateInventoryItem(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.agent_id), FieldValue::Bool(message.sim_approved), FieldValue::Uuid(message.transaction_id)]),
            many(message.inventory_data.iter().map(|item| item_fields(item, false)).collect()),
        ]),
        Message::RemoveInventoryItem(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.agent_id), FieldValue::Uuid(message.session_id)]),
            many(message.inventory_data.iter().map(|item| vec![FieldValue::Uuid(*item)]).collect()),
        ]),
        Message::RemoveInventoryFolder(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.agent_id), FieldValue::Uuid(message.session_id)]),
            many(message.folder_data.iter().map(|folder| vec![FieldValue::Uuid(*folder)]).collect()),
        ]),
        Message::RemoveInventoryObjects(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.agent_id), FieldValue::Uuid(message.session_id)]),
            many(message.folder_data.iter().map(|folder| vec![FieldValue::Uuid(*folder)]).collect()),
            many(message.item_data.iter().map(|item| vec![FieldValue::Uuid(*item)]).collect()),
        ]),
        Message::SaveAssetIntoInventory(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.agent_id)]),
            one(vec![FieldValue::Uuid(message.item_id), FieldValue::Uuid(message.new_asset_id)]),
        ]),
        Message::BulkUpdateInventory(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.agent_id), FieldValue::Uuid(message.transaction_id)]),
            many(
                message
                    .folder_data
                    .iter()
                    .map(|folder| {
                        vec![
                            FieldValue::Uuid(folder.folder_id),
                            FieldValue::Uuid(folder.parent_id),
                            FieldValue::S8(folder.folder_type),
                            ll_string(&folder.name),
                        ]
                    })
                    .collect(),
            ),
            many(message.item_data.iter().map(|item| item_fields(item, true)).collect()),
        ]),
        Message::MoveInventoryItem(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.agent_id), FieldValue::Uuid(message.session_id), FieldValue::Bool(message.stamp)]),
            many(
                message
                    .inventory_data
                    .iter()
                    .map(|MoveInventoryItemBlock { item_id, folder_id, new_name }| {
                        vec![FieldValue::Uuid(*item_id), FieldValue::Uuid(*folder_id), ll_string(new_name)]
                    })
                    .collect(),
            ),
        ]),
        Message::MoveInventoryFolder(message) => Some(vec![
            one(vec![FieldValue::Uuid(message.agent_id), FieldValue::Uuid(message.session_id), FieldValue::Bool(message.stamp)]),
            many(
                message
                    .inventory_data
                    .iter()
                    .map(|MoveInventoryFolderBlock { folder_id, parent_id }| {
                        vec![FieldValue::Uuid(*folder_id), FieldValue::Uuid(*parent_id)]
                    })
                    .collect(),
            ),
        ]),
        Message::Schema(_) | Message::Unknown { .. } => None,
    }
}

pub(crate) fn agent_update_fields(message: &protocol::messages::AgentUpdate) -> Vec<FieldValue> {
    let data = &message.agent_data;
    vec![
        FieldValue::Uuid(data.agent_id),
        FieldValue::Uuid(data.session_id),
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
