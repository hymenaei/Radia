use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

#[derive(Clone, Copy)]
#[repr(u8)]
pub(in crate::scene) enum DeliveryState {
    Pending = 0,
    Applied = 1,
    Abandoned = 2,
}

pub(in crate::scene) struct Receipt(Arc<AtomicU8>);

impl Receipt {
    pub fn new() -> (Self, Delivery) {
        let state = Arc::new(AtomicU8::new(DeliveryState::Pending as u8));
        (Self(state.clone()), Delivery(Some(state)))
    }

    pub fn state(&self) -> DeliveryState {
        match self.0.load(Ordering::Acquire) {
            0 => DeliveryState::Pending,
            1 => DeliveryState::Applied,
            2 => DeliveryState::Abandoned,
            _ => unreachable!("delivery state is written only by its token"),
        }
    }
}

pub(in crate::scene) struct Delivery(Option<Arc<AtomicU8>>);

impl Delivery {
    pub fn acknowledge(mut self) {
        self.0.take().expect("delivery is consumed at most once").store(DeliveryState::Applied as u8, Ordering::Release);
    }
}

impl Drop for Delivery {
    fn drop(&mut self) {
        if let Some(state) = &self.0 {
            state.store(DeliveryState::Abandoned as u8, Ordering::Release);
        }
    }
}
