use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::REGION_HANDSHAKE,
};

#[derive(Clone, Debug, PartialEq)]
pub struct RegionHandshake {
    pub region_info1: RegionInfo1,
    pub region_info2: Option<RegionInfo2>,
    pub region_info3: Option<RegionInfo3>,
    pub region_info4: Vec<RegionInfo4>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionInfo1 {
    pub region_flags: u32,
    pub sim_access: u8,
    pub sim_name: String,
    pub sim_owner: Uuid,
    pub is_estate_manager: bool,
    pub water_height: f32,
    pub billable_factor: f32,
    pub cache_id: Uuid,
    pub terrain_base: [Uuid; 4],
    pub terrain_detail: [Uuid; 4],
    pub terrain_start_heights: [f32; 4],
    pub terrain_height_ranges: [f32; 4],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionInfo2 {
    pub region_id: Uuid,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionInfo3 {
    pub cpu_class_id: i32,
    pub cpu_ratio: i32,
    pub colo_name: String,
    pub product_sku: String,
    pub product_name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionInfo4 {
    pub region_flags_extended: u64,
    pub region_protocols: u64,
}

impl TypedMessage for RegionHandshake {
    const ID: MessageId = REGION_HANDSHAKE;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::Specialized;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let region_info1 = RegionInfo1 {
            region_flags: reader.u32()?,
            sim_access: reader.u8()?,
            sim_name: reader.variable1_string()?,
            sim_owner: reader.uuid()?,
            is_estate_manager: reader.u8()? != 0,
            water_height: reader.f32()?,
            billable_factor: reader.f32()?,
            cache_id: reader.uuid()?,
            terrain_base: [reader.uuid()?, reader.uuid()?, reader.uuid()?, reader.uuid()?],
            terrain_detail: [reader.uuid()?, reader.uuid()?, reader.uuid()?, reader.uuid()?],
            terrain_start_heights: [reader.f32()?, reader.f32()?, reader.f32()?, reader.f32()?],
            terrain_height_ranges: [reader.f32()?, reader.f32()?, reader.f32()?, reader.f32()?],
        };
        let (region_info2, region_info3, region_info4) = if reader.is_empty() {
            (None, None, Vec::new())
        } else {
            let region_info2 = RegionInfo2 { region_id: reader.uuid()? };
            let region_info3 = RegionInfo3 {
                cpu_class_id: reader.i32()?,
                cpu_ratio: reader.i32()?,
                colo_name: reader.variable1_string()?,
                product_sku: reader.variable1_string()?,
                product_name: reader.variable1_string()?,
            };
            let count = reader.terminal_variable_count()?;
            let mut region_info4 = Vec::with_capacity(count);
            for _ in 0..count {
                region_info4.push(RegionInfo4 { region_flags_extended: reader.u64()?, region_protocols: reader.u64()? });
            }
            (Some(region_info2), Some(region_info3), region_info4)
        };
        Ok(Self { region_info1, region_info2, region_info3, region_info4 })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        let info = &self.region_info1;
        writer.u32(info.region_flags);
        writer.u8(info.sim_access);
        writer.variable1_string(&info.sim_name)?;
        writer.uuid(info.sim_owner);
        writer.u8(u8::from(info.is_estate_manager));
        writer.f32(info.water_height);
        writer.f32(info.billable_factor);
        writer.uuid(info.cache_id);
        for id in info.terrain_base {
            writer.uuid(id);
        }
        for id in info.terrain_detail {
            writer.uuid(id);
        }
        for height in info.terrain_start_heights {
            writer.f32(height);
        }
        for range in info.terrain_height_ranges {
            writer.f32(range);
        }
        match (&self.region_info2, &self.region_info3) {
            (Some(region_info2), Some(region_info3)) => {
                writer.uuid(region_info2.region_id);
                writer.i32(region_info3.cpu_class_id);
                writer.i32(region_info3.cpu_ratio);
                writer.variable1_string(&region_info3.colo_name)?;
                writer.variable1_string(&region_info3.product_sku)?;
                writer.variable1_string(&region_info3.product_name)?;
                let count = u8::try_from(self.region_info4.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
                writer.u8(count);
                for info in &self.region_info4 {
                    writer.u64(info.region_flags_extended);
                    writer.u64(info.region_protocols);
                }
            }
            (None, None) if self.region_info4.is_empty() => {}
            _ => return Err(LLUdpError::InvalidMessage),
        }
        Ok(())
    }

    fn preflight_allocation_bound(payload: &[u8]) -> Result<usize, LLUdpError> {
        let mut reader = Reader::new(payload);
        reader.take(4)?;
        reader.take(1)?;
        skip_variable1(&mut reader)?;
        reader.take(16)?;
        reader.take(1)?;
        reader.take(8)?;
        reader.take(16)?;
        reader.take(128)?;
        reader.take(32)?;
        let mut allocation_bytes = payload.len();
        if !reader.is_empty() {
            reader.take(16)?;
            reader.take(8)?;
            skip_variable1(&mut reader)?;
            skip_variable1(&mut reader)?;
            skip_variable1(&mut reader)?;
            let count = reader.terminal_variable_count()?;
            allocation_bytes = allocation_bytes
                .checked_add(count.checked_mul(std::mem::size_of::<RegionInfo4>()).ok_or(LLUdpError::AllocationBoundOverflow)?)
                .ok_or(LLUdpError::AllocationBoundOverflow)?;
        }
        Ok(allocation_bytes)
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = crate::lludp::message::allocation::capacity_bytes(self.region_info4.capacity(), std::mem::size_of::<RegionInfo4>());
        crate::lludp::message::allocation::add_string_capacity(&mut bytes, &self.region_info1.sim_name);
        if let Some(info) = &self.region_info3 {
            crate::lludp::message::allocation::add_string_capacity(&mut bytes, &info.colo_name);
            crate::lludp::message::allocation::add_string_capacity(&mut bytes, &info.product_sku);
            crate::lludp::message::allocation::add_string_capacity(&mut bytes, &info.product_name);
        }
        bytes
    }
}

fn skip_variable1(reader: &mut Reader<'_>) -> Result<(), LLUdpError> {
    let length = usize::from(reader.u8()?);
    reader.take(length)?;
    Ok(())
}
