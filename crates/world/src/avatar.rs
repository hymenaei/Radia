use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    sync::Arc,
};

use utils::AssetId;

pub const MAX_AVATAR_TEXTURE_SLOTS: usize = 45;
pub const MAX_AVATAR_VISUAL_PARAMS: usize = u8::MAX as usize;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AvatarVisualParamIndex(u8);

impl AvatarVisualParamIndex {
    pub const fn new(index: u8) -> Option<Self> {
        if index < u8::MAX { Some(Self(index)) } else { None }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AvatarTextureSlot(u8);

impl AvatarTextureSlot {
    pub const HEAD_BODY_PAINT: Self = Self(0);
    pub const UPPER_SHIRT: Self = Self(1);
    pub const LOWER_PANTS: Self = Self(2);
    pub const EYES_IRIS: Self = Self(3);
    pub const HAIR: Self = Self(4);
    pub const UPPER_BODY_PAINT: Self = Self(5);
    pub const LOWER_BODY_PAINT: Self = Self(6);
    pub const LOWER_SHOES: Self = Self(7);
    pub const HEAD_BAKED: Self = Self(8);
    pub const UPPER_BAKED: Self = Self(9);
    pub const LOWER_BAKED: Self = Self(10);
    pub const EYES_BAKED: Self = Self(11);
    pub const LOWER_SOCKS: Self = Self(12);
    pub const UPPER_JACKET: Self = Self(13);
    pub const LOWER_JACKET: Self = Self(14);
    pub const UPPER_GLOVES: Self = Self(15);
    pub const UPPER_UNDERSHIRT: Self = Self(16);
    pub const LOWER_UNDERPANTS: Self = Self(17);
    pub const SKIRT: Self = Self(18);
    pub const SKIRT_BAKED: Self = Self(19);
    pub const HAIR_BAKED: Self = Self(20);
    pub const LOWER_ALPHA: Self = Self(21);
    pub const UPPER_ALPHA: Self = Self(22);
    pub const HEAD_ALPHA: Self = Self(23);
    pub const EYES_ALPHA: Self = Self(24);
    pub const HAIR_ALPHA: Self = Self(25);
    pub const HEAD_TATTOO: Self = Self(26);
    pub const UPPER_TATTOO: Self = Self(27);
    pub const LOWER_TATTOO: Self = Self(28);
    pub const HEAD_UNIVERSAL_TATTOO: Self = Self(29);
    pub const UPPER_UNIVERSAL_TATTOO: Self = Self(30);
    pub const LOWER_UNIVERSAL_TATTOO: Self = Self(31);
    pub const SKIRT_TATTOO: Self = Self(32);
    pub const HAIR_TATTOO: Self = Self(33);
    pub const EYES_TATTOO: Self = Self(34);
    pub const LEFT_ARM_TATTOO: Self = Self(35);
    pub const LEFT_LEG_TATTOO: Self = Self(36);
    pub const AUX_TATTOO_1: Self = Self(37);
    pub const AUX_TATTOO_2: Self = Self(38);
    pub const AUX_TATTOO_3: Self = Self(39);
    pub const LEFT_ARM_BAKED: Self = Self(40);
    pub const LEFT_LEG_BAKED: Self = Self(41);
    pub const AUX_BAKED_1: Self = Self(42);
    pub const AUX_BAKED_2: Self = Self(43);
    pub const AUX_BAKED_3: Self = Self(44);

    pub const fn new(index: u8) -> Option<Self> {
        if index < MAX_AVATAR_TEXTURE_SLOTS as u8 { Some(Self(index)) } else { None }
    }

    pub const fn index(self) -> u8 {
        self.0
    }

    pub const fn baked_slot(self) -> Option<AvatarBakeSlot> {
        match self.0 {
            8 => Some(AvatarBakeSlot::Head),
            9 => Some(AvatarBakeSlot::Upper),
            10 => Some(AvatarBakeSlot::Lower),
            11 => Some(AvatarBakeSlot::Eyes),
            19 => Some(AvatarBakeSlot::Skirt),
            20 => Some(AvatarBakeSlot::Hair),
            40 => Some(AvatarBakeSlot::LeftArm),
            41 => Some(AvatarBakeSlot::LeftLeg),
            42 => Some(AvatarBakeSlot::Aux1),
            43 => Some(AvatarBakeSlot::Aux2),
            44 => Some(AvatarBakeSlot::Aux3),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AvatarBakeSlot {
    Head,
    Upper,
    Lower,
    Eyes,
    Skirt,
    Hair,
    LeftArm,
    LeftLeg,
    Aux1,
    Aux2,
    Aux3,
}

impl AvatarBakeSlot {
    pub const fn texture_slot(self) -> AvatarTextureSlot {
        match self {
            Self::Head => AvatarTextureSlot::HEAD_BAKED,
            Self::Upper => AvatarTextureSlot::UPPER_BAKED,
            Self::Lower => AvatarTextureSlot::LOWER_BAKED,
            Self::Eyes => AvatarTextureSlot::EYES_BAKED,
            Self::Skirt => AvatarTextureSlot::SKIRT_BAKED,
            Self::Hair => AvatarTextureSlot::HAIR_BAKED,
            Self::LeftArm => AvatarTextureSlot::LEFT_ARM_BAKED,
            Self::LeftLeg => AvatarTextureSlot::LEFT_LEG_BAKED,
            Self::Aux1 => AvatarTextureSlot::AUX_BAKED_1,
            Self::Aux2 => AvatarTextureSlot::AUX_BAKED_2,
            Self::Aux3 => AvatarTextureSlot::AUX_BAKED_3,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvatarTextureSlots {
    values: [Option<AssetId>; MAX_AVATAR_TEXTURE_SLOTS],
}

impl Default for AvatarTextureSlots {
    fn default() -> Self {
        Self { values: [None; MAX_AVATAR_TEXTURE_SLOTS] }
    }
}

impl AvatarTextureSlots {
    pub fn new(values: [Option<AssetId>; MAX_AVATAR_TEXTURE_SLOTS]) -> Result<Self, AvatarAppearanceError> {
        if values.iter().flatten().any(|asset| asset.uuid().is_nil()) {
            return Err(AvatarAppearanceError::NilTextureId);
        }
        Ok(Self { values })
    }

    pub fn texture(&self, slot: AvatarTextureSlot) -> Option<AssetId> {
        self.values[usize::from(slot.index())]
    }

    pub fn iter(&self) -> impl Iterator<Item = (AvatarTextureSlot, Option<AssetId>)> + '_ {
        self.values
            .iter()
            .copied()
            .enumerate()
            .map(|(index, asset)| (AvatarTextureSlot::new(index as u8).expect("slot array has the supported length"), asset))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AvatarAppearance {
    appearance_version: Option<u8>,
    cof_version: Option<i32>,
    visual_params: Vec<(AvatarVisualParamIndex, u8)>,
    texture_slots: Option<Arc<AvatarTextureSlots>>,
}

impl AvatarAppearance {
    pub const fn appearance_version(&self) -> Option<u8> {
        self.appearance_version
    }

    pub const fn cof_version(&self) -> Option<i32> {
        self.cof_version
    }

    pub fn visual_param(&self, index: AvatarVisualParamIndex) -> Option<u8> {
        self.visual_params.binary_search_by_key(&index, |(id, _)| *id).ok().map(|at| self.visual_params[at].1)
    }

    pub fn visual_params(&self) -> impl Iterator<Item = (AvatarVisualParamIndex, u8)> + '_ {
        self.visual_params.iter().copied()
    }

    pub fn texture_slots(&self) -> Option<&AvatarTextureSlots> {
        self.texture_slots.as_deref()
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.visual_params.capacity() * size_of::<(AvatarVisualParamIndex, u8)>()
            + self.texture_slots.as_ref().map_or(0, |_| size_of::<AvatarTextureSlots>())
    }

    pub(crate) fn apply(&mut self, update: AvatarAppearanceUpdate) {
        if let Some(version) = update.appearance_version {
            self.appearance_version = Some(version);
        }
        if let Some(version) = update.cof_version {
            self.cof_version = Some(version);
        }
        for (index, value) in update.visual_params {
            match self.visual_params.binary_search_by_key(&index, |(id, _)| *id) {
                Ok(at) => self.visual_params[at].1 = value,
                Err(at) => self.visual_params.insert(at, (index, value)),
            }
        }
        if let Some(slots) = update.texture_slots {
            self.texture_slots = Some(slots);
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AvatarAppearanceUpdate {
    appearance_version: Option<u8>,
    cof_version: Option<i32>,
    visual_params: Vec<(AvatarVisualParamIndex, u8)>,
    texture_slots: Option<Arc<AvatarTextureSlots>>,
}

impl AvatarAppearanceUpdate {
    pub fn new(
        appearance_version: Option<u8>,
        cof_version: Option<i32>,
        mut visual_params: Vec<(AvatarVisualParamIndex, u8)>,
        texture_slots: Option<AvatarTextureSlots>,
    ) -> Result<Self, AvatarAppearanceError> {
        if visual_params.len() > MAX_AVATAR_VISUAL_PARAMS {
            return Err(AvatarAppearanceError::TooManyVisualParams);
        }
        visual_params.sort_unstable_by_key(|(index, _)| *index);
        if visual_params.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(AvatarAppearanceError::DuplicateVisualParam);
        }
        Ok(Self { appearance_version, cof_version, visual_params, texture_slots: texture_slots.map(Arc::new) })
    }

    pub const fn appearance_version(&self) -> Option<u8> {
        self.appearance_version
    }

    pub const fn cof_version(&self) -> Option<i32> {
        self.cof_version
    }

    pub fn visual_params(&self) -> impl Iterator<Item = (AvatarVisualParamIndex, u8)> + '_ {
        self.visual_params.iter().copied()
    }

    pub fn texture_slots(&self) -> Option<&AvatarTextureSlots> {
        self.texture_slots.as_deref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AvatarAppearanceError {
    DuplicateVisualParam,
    NilTextureId,
    TooManyVisualParams,
}

impl Display for AvatarAppearanceError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateVisualParam => f.write_str("avatar visual parameter is duplicated"),
            Self::NilTextureId => f.write_str("avatar texture identifier is nil"),
            Self::TooManyVisualParams => f.write_str("too many avatar visual parameters"),
        }
    }
}

impl Error for AvatarAppearanceError {}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{AvatarAppearance, AvatarAppearanceUpdate, AvatarBakeSlot, AvatarTextureSlot, AvatarTextureSlots, AvatarVisualParamIndex};
    use utils::AssetId;

    #[test]
    fn avatar_slot_bounds_hold() {
        assert_eq!(AvatarTextureSlot::new(44).map(AvatarTextureSlot::index), Some(44));
        assert_eq!(AvatarTextureSlot::new(45), None);
        assert_eq!(AvatarVisualParamIndex::new(254).map(AvatarVisualParamIndex::get), Some(254));
        assert_eq!(AvatarVisualParamIndex::new(255), None);
    }

    #[test]
    fn baked_slots_use_avatar_indices() {
        assert_eq!(AvatarTextureSlot::HEAD_BAKED.baked_slot(), Some(AvatarBakeSlot::Head));
        assert_eq!(AvatarTextureSlot::AUX_BAKED_3.baked_slot(), Some(AvatarBakeSlot::Aux3));
        assert_eq!(AvatarTextureSlot::HEAD_BODY_PAINT.baked_slot(), None);
        assert_eq!(AvatarBakeSlot::Skirt.texture_slot(), AvatarTextureSlot::SKIRT_BAKED);
    }

    #[test]
    fn params_merge_by_position() {
        let first = AvatarAppearanceUpdate::new(
            None,
            None,
            vec![(AvatarVisualParamIndex::new(0).unwrap(), 10), (AvatarVisualParamIndex::new(2).unwrap(), 30)],
            None,
        )
        .expect("valid update");
        let second =
            AvatarAppearanceUpdate::new(None, None, vec![(AvatarVisualParamIndex::new(0).unwrap(), 20)], None).expect("valid update");
        let mut appearance = AvatarAppearance::default();

        appearance.apply(first);
        appearance.apply(second);

        assert_eq!(appearance.visual_param(AvatarVisualParamIndex::new(0).unwrap()), Some(20));
        assert_eq!(appearance.visual_param(AvatarVisualParamIndex::new(2).unwrap()), Some(30));
    }

    #[test]
    fn texture_slots_replace_and_clear() {
        let slot = AvatarTextureSlot::HEAD_BAKED;
        let mut values = [None; super::MAX_AVATAR_TEXTURE_SLOTS];
        values[usize::from(slot.index())] = Some(AssetId::new(Uuid::from_u128(7)));
        let set = AvatarTextureSlots::new(values).expect("valid slots");
        let mut appearance = AvatarAppearance::default();
        appearance.apply(AvatarAppearanceUpdate::new(None, None, Vec::new(), Some(set)).expect("valid update"));
        assert_eq!(appearance.texture_slots().and_then(|slots| slots.texture(slot)), Some(AssetId::new(Uuid::from_u128(7))));

        let retained = appearance.clone();
        appearance.apply(AvatarAppearanceUpdate::new(None, None, Vec::new(), Some(AvatarTextureSlots::default())).expect("valid clear"));
        assert_eq!(appearance.texture_slots().and_then(|slots| slots.texture(slot)), None);
        assert_eq!(retained.texture_slots().and_then(|slots| slots.texture(slot)), Some(AssetId::new(Uuid::from_u128(7))));

        appearance.apply(AvatarAppearanceUpdate::new(Some(1), None, Vec::new(), None).expect("partial update"));
        assert!(appearance.texture_slots().is_some());
        assert_eq!(appearance.texture_slots().and_then(|slots| slots.texture(slot)), None);
    }
}
