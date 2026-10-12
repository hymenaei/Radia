use base::UUID;
use protocol::{FieldValue, SchemaMessage, lludp::Message};
use world::SitTarget;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SitTargetUpdate {
    pub(crate) object_id: UUID,
    pub(crate) target: SitTarget,
}

pub(crate) fn decode_sit_target_update(message: &Message) -> Option<SitTargetUpdate> {
    let Message::Schema(message) = message else { return None };
    if message.schema().name != "AvatarSitResponse" {
        return None;
    }

    let object_id = match schema_field(message, "SitObject", "ID")? {
        FieldValue::UUID(id) if !id.is_nil() => *id,
        _ => return None,
    };
    let position = match schema_field(message, "SitTransform", "SitPosition")? {
        FieldValue::Vector3(position) => *position,
        _ => return None,
    };
    let rotation = match schema_field(message, "SitTransform", "SitRotation")? {
        FieldValue::Quaternion(rotation) => *rotation,
        _ => return None,
    };

    Some(SitTargetUpdate { object_id, target: SitTarget { position, rotation } })
}

fn schema_field<'a>(message: &'a SchemaMessage, block_name: &str, field_name: &str) -> Option<&'a FieldValue> {
    let (block_schema, block_values) =
        message.schema().blocks.iter().zip(message.blocks()).find(|(schema, _)| schema.name == block_name)?;
    let instance = block_values.instances.first()?;
    let field_index = block_schema.fields.iter().position(|field| field.name == field_name)?;
    instance.fields.get(field_index)
}

#[cfg(test)]
mod tests {
    use base::UUID;
    use math::{Quat, Vec3};
    use protocol::{BlockValues, FieldValue, SchemaMessage, lludp::Message};

    use super::decode_sit_target_update;

    #[test]
    fn sit_response_maps_object_relative_evidence() {
        let schema = SchemaMessage::schema_by_name("AvatarSitResponse").expect("sit response schema");
        let message = SchemaMessage::new(
            schema,
            vec![
                BlockValues::single(vec![FieldValue::UUID(UUID::from_u128(7))]),
                BlockValues::single(vec![
                    FieldValue::Bool(true),
                    FieldValue::Vector3(Vec3::new(1.0, 2.0, 3.0)),
                    FieldValue::Quaternion(Quat::IDENTITY),
                    FieldValue::Vector3(Vec3::ZERO),
                    FieldValue::Vector3(Vec3::ZERO),
                    FieldValue::Bool(false),
                ]),
            ],
        )
        .expect("valid schema message");
        let update = decode_sit_target_update(&Message::Schema(message)).expect("sit target update");

        assert_eq!(update.object_id, UUID::from_u128(7));
        assert_eq!(update.target.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(update.target.rotation, Quat::IDENTITY);
    }

    #[test]
    fn other_messages_do_not_map_as_sit_targets() {
        assert_eq!(decode_sit_target_update(&Message::StartPingCheck(Default::default())), None);
    }
}
