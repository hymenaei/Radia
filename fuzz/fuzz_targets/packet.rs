#![no_main]

use libfuzzer_sys::fuzz_target;
use protocol::lludp::Packet;

fuzz_target!(|data: &[u8]| {
    let _ = Packet::decode(data);
});
