use uuid::Uuid;

macro_rules! ids {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
            pub struct $name(Uuid);

            impl $name {
                pub const fn new(uuid: Uuid) -> Self {
                    Self(uuid)
                }

                pub const fn uuid(self) -> Uuid {
                    self.0
                }
            }
        )+
    };
}

ids!(ObjectId, AssetId, AvatarId, GroupId, RegionId, ParcelId, ItemId, FolderId);
