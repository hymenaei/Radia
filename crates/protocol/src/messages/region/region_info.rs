use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::REGION_INFO,
};

#[derive(Clone, Debug, PartialEq)]
pub struct RegionInfo {
    pub agent_data: AgentData,
    pub region_info: RegionInfoBlock,
    pub region_info2: RegionInfo2Block,
    pub region_info3: Vec<RegionInfo3>,
    pub region_info5: Vec<RegionInfo5>,
    pub combat_settings: Vec<CombatSettings>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegionInfoBlock {
    pub sim_name: Vec<u8>,
    pub estate_id: Option<u32>,
    pub parent_estate_id: Option<u32>,
    pub region_flags: Option<u32>,
    pub sim_access: Option<u8>,
    pub max_agents: Option<u8>,
    pub billable_factor: Option<f32>,
    pub object_bonus_factor: Option<f32>,
    pub water_height: Option<f32>,
    pub terrain_raise_limit: Option<f32>,
    pub terrain_lower_limit: Option<f32>,
    pub price_per_meter: Option<i32>,
    pub redirect_grid_x: Option<i32>,
    pub redirect_grid_y: Option<i32>,
    pub use_estate_sun: Option<bool>,
    pub sun_hour: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegionInfo2Block {
    pub product_sku: Vec<u8>,
    pub product_name: Vec<u8>,
    pub max_agents32: Option<u32>,
    pub hard_max_agents: Option<u32>,
    pub hard_max_objects: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegionInfo3 {
    pub region_flags_extended: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionInfo5 {
    pub chat_whisper_range: Option<f32>,
    pub chat_normal_range: Option<f32>,
    pub chat_shout_range: Option<f32>,
    pub chat_whisper_offset: Option<f32>,
    pub chat_normal_offset: Option<f32>,
    pub chat_shout_offset: Option<f32>,
    pub chat_flags: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CombatSettings {
    pub combat_flags: Option<u32>,
    pub on_death: Option<u8>,
    pub damage_throttle: Option<f32>,
    pub regeneration_rate: Option<f32>,
    pub invulnerability_time: Option<f32>,
    pub damage_limit: Option<f32>,
}

impl TypedMessage for RegionInfo {
    const ID: MessageId = REGION_INFO;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let agent_data = AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? };
        let region_info = RegionInfoBlock {
            sim_name: reader.variable1()?.to_vec(),
            estate_id: reader.optional_fixed(4, |reader| reader.u32())?,
            parent_estate_id: reader.optional_fixed(4, |reader| reader.u32())?,
            region_flags: reader.optional_fixed(4, |reader| reader.u32())?,
            sim_access: reader.optional_fixed(1, |reader| reader.u8())?,
            max_agents: reader.optional_fixed(1, |reader| reader.u8())?,
            billable_factor: reader.optional_fixed(4, |reader| reader.f32())?,
            object_bonus_factor: reader.optional_fixed(4, |reader| reader.f32())?,
            water_height: reader.optional_fixed(4, |reader| reader.f32())?,
            terrain_raise_limit: reader.optional_fixed(4, |reader| reader.f32())?,
            terrain_lower_limit: reader.optional_fixed(4, |reader| reader.f32())?,
            price_per_meter: reader.optional_fixed(4, |reader| reader.i32())?,
            redirect_grid_x: reader.optional_fixed(4, |reader| reader.i32())?,
            redirect_grid_y: reader.optional_fixed(4, |reader| reader.i32())?,
            use_estate_sun: reader.optional_fixed(1, |reader| reader.u8().map(|value| value != 0))?,
            sun_hour: reader.optional_fixed(4, |reader| reader.f32())?,
        };
        let region_info2 = RegionInfo2Block {
            product_sku: reader.variable1()?.to_vec(),
            product_name: reader.variable1()?.to_vec(),
            max_agents32: reader.optional_fixed(4, |reader| reader.u32())?,
            hard_max_agents: reader.optional_fixed(4, |reader| reader.u32())?,
            hard_max_objects: reader.optional_fixed(4, |reader| reader.u32())?,
        };
        let region_info3_count = usize::from(reader.u8()?);
        let mut region_info3 = Vec::with_capacity(region_info3_count);
        for _ in 0..region_info3_count {
            region_info3.push(RegionInfo3 { region_flags_extended: reader.optional_fixed(8, |reader| reader.u64())? });
        }
        let region_info5_count = usize::from(reader.u8()?);
        let mut region_info5 = Vec::with_capacity(region_info5_count);
        for _ in 0..region_info5_count {
            region_info5.push(RegionInfo5 {
                chat_whisper_range: reader.optional_fixed(4, |reader| reader.f32())?,
                chat_normal_range: reader.optional_fixed(4, |reader| reader.f32())?,
                chat_shout_range: reader.optional_fixed(4, |reader| reader.f32())?,
                chat_whisper_offset: reader.optional_fixed(4, |reader| reader.f32())?,
                chat_normal_offset: reader.optional_fixed(4, |reader| reader.f32())?,
                chat_shout_offset: reader.optional_fixed(4, |reader| reader.f32())?,
                chat_flags: reader.optional_fixed(4, |reader| reader.u32())?,
            });
        }
        let combat_settings_count = reader.terminal_variable_count()?;
        let mut combat_settings = Vec::with_capacity(combat_settings_count);
        for _ in 0..combat_settings_count {
            combat_settings.push(CombatSettings {
                combat_flags: reader.optional_fixed(4, |reader| reader.u32())?,
                on_death: reader.optional_fixed(1, |reader| reader.u8())?,
                damage_throttle: reader.optional_fixed(4, |reader| reader.f32())?,
                regeneration_rate: reader.optional_fixed(4, |reader| reader.f32())?,
                invulnerability_time: reader.optional_fixed(4, |reader| reader.f32())?,
                damage_limit: reader.optional_fixed(4, |reader| reader.f32())?,
            });
        }
        Ok(Self { agent_data, region_info, region_info2, region_info3, region_info5, combat_settings })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        macro_rules! write_required {
            ($value:expr, $method:ident) => {
                writer.$method($value.ok_or(LLUdpError::InvalidMessage)?);
            };
        }
        macro_rules! write_optional_suffix {
            ($value:expr, $method:ident) => {
                let Some(value) = $value else { return Ok(()) };
                writer.$method(value);
            };
        }

        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        writer.variable1(&self.region_info.sim_name)?;
        write_required!(self.region_info.estate_id, u32);
        write_required!(self.region_info.parent_estate_id, u32);
        write_required!(self.region_info.region_flags, u32);
        write_required!(self.region_info.sim_access, u8);
        write_required!(self.region_info.max_agents, u8);
        write_required!(self.region_info.billable_factor, f32);
        write_required!(self.region_info.object_bonus_factor, f32);
        write_required!(self.region_info.water_height, f32);
        write_required!(self.region_info.terrain_raise_limit, f32);
        write_required!(self.region_info.terrain_lower_limit, f32);
        write_required!(self.region_info.price_per_meter, i32);
        write_required!(self.region_info.redirect_grid_x, i32);
        write_required!(self.region_info.redirect_grid_y, i32);
        let use_estate_sun = self.region_info.use_estate_sun.ok_or(LLUdpError::InvalidMessage)?;
        writer.u8(u8::from(use_estate_sun));
        write_required!(self.region_info.sun_hour, f32);
        writer.variable1(&self.region_info2.product_sku)?;
        writer.variable1(&self.region_info2.product_name)?;
        write_required!(self.region_info2.max_agents32, u32);
        write_required!(self.region_info2.hard_max_agents, u32);
        write_required!(self.region_info2.hard_max_objects, u32);
        writer.u8(u8::try_from(self.region_info3.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        for info in &self.region_info3 {
            write_required!(info.region_flags_extended, u64);
        }
        writer.u8(u8::try_from(self.region_info5.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        for info in &self.region_info5 {
            write_optional_suffix!(info.chat_whisper_range, f32);
            write_optional_suffix!(info.chat_normal_range, f32);
            write_optional_suffix!(info.chat_shout_range, f32);
            write_optional_suffix!(info.chat_whisper_offset, f32);
            write_optional_suffix!(info.chat_normal_offset, f32);
            write_optional_suffix!(info.chat_shout_offset, f32);
            write_optional_suffix!(info.chat_flags, u32);
        }
        writer.u8(u8::try_from(self.combat_settings.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        for settings in &self.combat_settings {
            write_optional_suffix!(settings.combat_flags, u32);
            write_optional_suffix!(settings.on_death, u8);
            write_optional_suffix!(settings.damage_throttle, f32);
            write_optional_suffix!(settings.regeneration_rate, f32);
            write_optional_suffix!(settings.invulnerability_time, f32);
            write_optional_suffix!(settings.damage_limit, f32);
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = crate::lludp::message::allocation::capacity_bytes(self.region_info.sim_name.capacity(), std::mem::size_of::<u8>());
        crate::lludp::message::allocation::add_capacity(&mut bytes, self.region_info3.capacity(), std::mem::size_of::<RegionInfo3>());
        crate::lludp::message::allocation::add_capacity(&mut bytes, self.region_info5.capacity(), std::mem::size_of::<RegionInfo5>());
        crate::lludp::message::allocation::add_capacity(&mut bytes, self.combat_settings.capacity(), std::mem::size_of::<CombatSettings>());
        crate::lludp::message::allocation::add_capacity(&mut bytes, self.region_info2.product_sku.capacity(), std::mem::size_of::<u8>());
        crate::lludp::message::allocation::add_capacity(&mut bytes, self.region_info2.product_name.capacity(), std::mem::size_of::<u8>());
        bytes
    }
}
