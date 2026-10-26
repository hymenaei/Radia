#[cfg(test)]
mod controller_tests;

mod controller;
mod delta;
mod handle;
mod types;

pub(crate) use controller::{InventoryEditController, InventoryEditEvent};
pub use handle::InventoryEditHandle;
pub(crate) use types::MoveExpectation;
pub use types::{InventoryEditOutcome, InventoryEditRejection};
