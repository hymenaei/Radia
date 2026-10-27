use std::fmt;

use math::Vec3;
use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::PARCEL_PROPERTIES,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ParcelProperties {
    pub parcel_data: Box<ParcelData>,
    pub age_verification_block: AgeVerificationBlock,
    pub region_allow_access_block: RegionAllowAccessBlock,
    pub parcel_environment_block: ParcelEnvironmentBlock,
}

#[derive(Clone, PartialEq)]
pub struct ParcelData {
    pub request_result: Option<i32>,
    pub sequence_id: Option<i32>,
    pub snap_selection: Option<bool>,
    pub self_count: Option<i32>,
    pub other_count: Option<i32>,
    pub public_count: Option<i32>,
    pub local_id: Option<i32>,
    pub owner_id: Option<Uuid>,
    pub is_group_owned: Option<bool>,
    pub auction_id: Option<u32>,
    pub claim_date: Option<i32>,
    pub claim_price: Option<i32>,
    pub rent_price: Option<i32>,
    pub aabb_min: Option<Vec3>,
    pub aabb_max: Option<Vec3>,
    pub bitmap: Option<Vec<u8>>,
    pub area: Option<i32>,
    pub status: Option<u8>,
    pub sim_wide_max_prims: Option<i32>,
    pub sim_wide_total_prims: Option<i32>,
    pub max_prims: Option<i32>,
    pub total_prims: Option<i32>,
    pub owner_prims: Option<i32>,
    pub group_prims: Option<i32>,
    pub other_prims: Option<i32>,
    pub selected_prims: Option<i32>,
    pub parcel_prim_bonus: Option<f32>,
    pub other_clean_time: Option<i32>,
    pub parcel_flags: Option<u32>,
    pub sale_price: Option<i32>,
    pub name: Option<Vec<u8>>,
    pub description: Option<Vec<u8>>,
    pub music_url: Option<Vec<u8>>,
    pub media_url: Option<Vec<u8>>,
    pub media_id: Option<Uuid>,
    pub media_auto_scale: Option<u8>,
    pub group_id: Option<Uuid>,
    pub pass_price: Option<i32>,
    pub pass_hours: Option<f32>,
    pub category: Option<u8>,
    pub auth_buyer_id: Option<Uuid>,
    pub snapshot_id: Option<Uuid>,
    pub user_location: Option<Vec3>,
    pub user_look_at: Option<Vec3>,
    pub landing_type: Option<u8>,
    pub region_push_override: Option<bool>,
    pub region_deny_anonymous: Option<bool>,
    pub region_deny_identified: Option<bool>,
    pub region_deny_transacted: Option<bool>,
}

impl fmt::Debug for ParcelData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParcelData")
            .field("request_result", &self.request_result)
            .field("sequence_id", &self.sequence_id)
            .field("local_id", &self.local_id)
            .field("owner_id", &self.owner_id)
            .field("area", &self.area)
            .field("parcel_flags", &self.parcel_flags)
            .field("name", &self.name)
            .field("description", &self.description)
            .field("music_url", &self.music_url.as_ref().map(|_| "<redacted>"))
            .field("media_url", &self.media_url.as_ref().map(|_| "<redacted>"))
            .field("group_id", &self.group_id)
            .field("user_location", &self.user_location)
            .field("user_look_at", &self.user_look_at)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgeVerificationBlock {
    pub region_deny_age_unverified: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegionAllowAccessBlock {
    pub region_allow_access_override: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParcelEnvironmentBlock {
    pub parcel_environment_version: Option<i32>,
    pub region_allow_environment_override: Option<bool>,
}

impl TypedMessage for ParcelProperties {
    const ID: MessageId = PARCEL_PROPERTIES;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let parcel_data = Box::new(ParcelData {
            request_result: reader.optional_fixed(4, |reader| reader.i32())?,
            sequence_id: reader.optional_fixed(4, |reader| reader.i32())?,
            snap_selection: reader.optional_fixed(1, |reader| reader.u8().map(|value| value != 0))?,
            self_count: reader.optional_fixed(4, |reader| reader.i32())?,
            other_count: reader.optional_fixed(4, |reader| reader.i32())?,
            public_count: reader.optional_fixed(4, |reader| reader.i32())?,
            local_id: reader.optional_fixed(4, |reader| reader.i32())?,
            owner_id: reader.optional_fixed(16, |reader| reader.uuid())?,
            is_group_owned: reader.optional_fixed(1, |reader| reader.u8().map(|value| value != 0))?,
            auction_id: reader.optional_fixed(4, |reader| reader.u32())?,
            claim_date: reader.optional_fixed(4, |reader| reader.i32())?,
            claim_price: reader.optional_fixed(4, |reader| reader.i32())?,
            rent_price: reader.optional_fixed(4, |reader| reader.i32())?,
            aabb_min: reader.optional_fixed(12, |reader| reader.vector3())?,
            aabb_max: reader.optional_fixed(12, |reader| reader.vector3())?,
            bitmap: Some(reader.variable2()?.to_vec()),
            area: reader.optional_fixed(4, |reader| reader.i32())?,
            status: reader.optional_fixed(1, |reader| reader.u8())?,
            sim_wide_max_prims: reader.optional_fixed(4, |reader| reader.i32())?,
            sim_wide_total_prims: reader.optional_fixed(4, |reader| reader.i32())?,
            max_prims: reader.optional_fixed(4, |reader| reader.i32())?,
            total_prims: reader.optional_fixed(4, |reader| reader.i32())?,
            owner_prims: reader.optional_fixed(4, |reader| reader.i32())?,
            group_prims: reader.optional_fixed(4, |reader| reader.i32())?,
            other_prims: reader.optional_fixed(4, |reader| reader.i32())?,
            selected_prims: reader.optional_fixed(4, |reader| reader.i32())?,
            parcel_prim_bonus: reader.optional_fixed(4, |reader| reader.f32())?,
            other_clean_time: reader.optional_fixed(4, |reader| reader.i32())?,
            parcel_flags: reader.optional_fixed(4, |reader| reader.u32())?,
            sale_price: reader.optional_fixed(4, |reader| reader.i32())?,
            name: Some(reader.variable1()?.to_vec()),
            description: Some(reader.variable1()?.to_vec()),
            music_url: Some(reader.variable1()?.to_vec()),
            media_url: Some(reader.variable1()?.to_vec()),
            media_id: reader.optional_fixed(16, |reader| reader.uuid())?,
            media_auto_scale: reader.optional_fixed(1, |reader| reader.u8())?,
            group_id: reader.optional_fixed(16, |reader| reader.uuid())?,
            pass_price: reader.optional_fixed(4, |reader| reader.i32())?,
            pass_hours: reader.optional_fixed(4, |reader| reader.f32())?,
            category: reader.optional_fixed(1, |reader| reader.u8())?,
            auth_buyer_id: reader.optional_fixed(16, |reader| reader.uuid())?,
            snapshot_id: reader.optional_fixed(16, |reader| reader.uuid())?,
            user_location: reader.optional_fixed(12, |reader| reader.vector3())?,
            user_look_at: reader.optional_fixed(12, |reader| reader.vector3())?,
            landing_type: reader.optional_fixed(1, |reader| reader.u8())?,
            region_push_override: reader.optional_fixed(1, |reader| reader.u8().map(|value| value != 0))?,
            region_deny_anonymous: reader.optional_fixed(1, |reader| reader.u8().map(|value| value != 0))?,
            region_deny_identified: reader.optional_fixed(1, |reader| reader.u8().map(|value| value != 0))?,
            region_deny_transacted: reader.optional_fixed(1, |reader| reader.u8().map(|value| value != 0))?,
        });
        let age_verification_block =
            AgeVerificationBlock { region_deny_age_unverified: reader.optional_fixed(1, |reader| reader.u8().map(|value| value != 0))? };
        let region_allow_access_block = RegionAllowAccessBlock {
            region_allow_access_override: reader.optional_fixed(1, |reader| reader.u8().map(|value| value != 0))?,
        };
        let parcel_environment_block = ParcelEnvironmentBlock {
            parcel_environment_version: reader.optional_fixed(4, |reader| reader.i32())?,
            region_allow_environment_override: reader.optional_fixed(1, |reader| reader.u8().map(|value| value != 0))?,
        };
        Ok(Self { parcel_data, age_verification_block, region_allow_access_block, parcel_environment_block })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        if !encode_parcel_data(&self.parcel_data, writer)? {
            return Ok(());
        }
        let Some(age) = self.age_verification_block.region_deny_age_unverified else { return Ok(()) };
        writer.u8(u8::from(age));
        let Some(access) = self.region_allow_access_block.region_allow_access_override else { return Ok(()) };
        writer.u8(u8::from(access));
        let Some(version) = self.parcel_environment_block.parcel_environment_version else { return Ok(()) };
        writer.i32(version);
        let Some(overrides) = self.parcel_environment_block.region_allow_environment_override else { return Ok(()) };
        writer.u8(u8::from(overrides));
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = crate::lludp::message::allocation::capacity_bytes(1, std::mem::size_of::<ParcelData>());
        for value in [
            &self.parcel_data.bitmap,
            &self.parcel_data.name,
            &self.parcel_data.description,
            &self.parcel_data.music_url,
            &self.parcel_data.media_url,
        ]
        .into_iter()
        .flatten()
        {
            crate::lludp::message::allocation::add_capacity(&mut bytes, value.capacity(), std::mem::size_of::<u8>());
        }
        bytes
    }
}

fn encode_parcel_data(data: &ParcelData, writer: &mut Writer<'_>) -> Result<bool, LLUdpError> {
    macro_rules! fixed {
        ($field:expr, $write:ident) => {
            let Some(value) = $field else { return Ok(false) };
            writer.$write(value);
        };
    }

    fixed!(data.request_result, i32);
    fixed!(data.sequence_id, i32);
    let Some(snap_selection) = data.snap_selection else { return Ok(false) };
    writer.u8(u8::from(snap_selection));
    fixed!(data.self_count, i32);
    fixed!(data.other_count, i32);
    fixed!(data.public_count, i32);
    fixed!(data.local_id, i32);
    fixed!(data.owner_id, uuid);
    let Some(is_group_owned) = data.is_group_owned else { return Ok(false) };
    writer.u8(u8::from(is_group_owned));
    fixed!(data.auction_id, u32);
    fixed!(data.claim_date, i32);
    fixed!(data.claim_price, i32);
    fixed!(data.rent_price, i32);
    fixed!(data.aabb_min, vector3);
    fixed!(data.aabb_max, vector3);
    let Some(bitmap) = &data.bitmap else { return Ok(false) };
    writer.variable2(bitmap)?;
    fixed!(data.area, i32);
    fixed!(data.status, u8);
    fixed!(data.sim_wide_max_prims, i32);
    fixed!(data.sim_wide_total_prims, i32);
    fixed!(data.max_prims, i32);
    fixed!(data.total_prims, i32);
    fixed!(data.owner_prims, i32);
    fixed!(data.group_prims, i32);
    fixed!(data.other_prims, i32);
    fixed!(data.selected_prims, i32);
    fixed!(data.parcel_prim_bonus, f32);
    fixed!(data.other_clean_time, i32);
    fixed!(data.parcel_flags, u32);
    fixed!(data.sale_price, i32);
    let Some(name) = &data.name else { return Ok(false) };
    writer.variable1(name)?;
    let Some(description) = &data.description else { return Ok(false) };
    writer.variable1(description)?;
    let Some(music_url) = &data.music_url else { return Ok(false) };
    writer.variable1(music_url)?;
    let Some(media_url) = &data.media_url else { return Ok(false) };
    writer.variable1(media_url)?;
    fixed!(data.media_id, uuid);
    fixed!(data.media_auto_scale, u8);
    fixed!(data.group_id, uuid);
    fixed!(data.pass_price, i32);
    fixed!(data.pass_hours, f32);
    fixed!(data.category, u8);
    fixed!(data.auth_buyer_id, uuid);
    fixed!(data.snapshot_id, uuid);
    fixed!(data.user_location, vector3);
    fixed!(data.user_look_at, vector3);
    fixed!(data.landing_type, u8);
    let Some(region_push_override) = data.region_push_override else { return Ok(false) };
    writer.u8(u8::from(region_push_override));
    let Some(region_deny_anonymous) = data.region_deny_anonymous else { return Ok(false) };
    writer.u8(u8::from(region_deny_anonymous));
    let Some(region_deny_identified) = data.region_deny_identified else { return Ok(false) };
    writer.u8(u8::from(region_deny_identified));
    let Some(region_deny_transacted) = data.region_deny_transacted else { return Ok(false) };
    writer.u8(u8::from(region_deny_transacted));
    Ok(true)
}
