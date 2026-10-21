pub use uuid::Uuid as UUID;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AssetId(UUID);

impl AssetId {
    pub const fn new(uuid: UUID) -> Self {
        Self(uuid)
    }

    pub const fn uuid(self) -> UUID {
        self.0
    }
}
