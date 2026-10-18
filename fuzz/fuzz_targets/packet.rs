#![no_main]

use libfuzzer_sys::fuzz_target;
use protocol::lludp::Packet;

fuzz_target!(|data: &[u8]| {
    if let Ok(packet) = Packet::decode(data) {
        let bound = Packet::preflight(data).expect("accepted packet has allocation preflight");
        assert!(packet.retained_capacity_bytes() <= bound.retained_capacity_upper_bound());
        assert!(bound.retained_capacity_upper_bound() <= bound.peak_decode_allocation_bytes());
    }
});
