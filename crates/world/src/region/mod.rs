pub(crate) mod environment;
pub(crate) mod parcel;
pub(crate) mod terrain;

use crate::{RegionInfo, update::FieldStamp};

#[derive(Default)]
pub(crate) struct RegionState {
    pub(crate) info: RegionInfo,
    pub(crate) name_fence: Option<FieldStamp>,
    pub(crate) terrain: terrain::Terrain,
    pub(crate) environment: environment::Environments,
    pub(crate) parcels: parcel::ParcelState,
}
