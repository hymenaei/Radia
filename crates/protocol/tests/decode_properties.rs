use proptest::{prelude::*, test_runner::Config as ProptestConfig};
use protocol::{UdpError, udp::Packet};
use test_support::{parse_hex_fixture, udp::vectors};

const MAX_DATAGRAM_BYTES: usize = 0x2000;

const OBJECT_FIXTURES: [&str; 6] = [
    vectors::OBJECT_UPDATE,
    vectors::OBJECT_UPDATE_COMPRESSED_TYPED,
    vectors::IMPROVED_TERSE_OBJECT_UPDATE,
    vectors::OBJECT_UPDATE_CACHED,
    vectors::KILL_OBJECT,
    vectors::REQUEST_MULTIPLE_OBJECTS,
];

fn object_packet_prefixes() -> Vec<Vec<u8>> {
    OBJECT_FIXTURES
        .into_iter()
        .map(|fixture| {
            let bytes = parse_hex_fixture(fixture).expect("valid object packet fixture");
            let packet = Packet::decode(&bytes).expect("decode object packet fixture").with_zero_coding(false);
            let mut wire = packet.encode().expect("encode uncompressed object packet fixture");

            let message_id_start = 6 + usize::from(wire[5]);
            let message_id_len = match wire[message_id_start] {
                0xff if wire[message_id_start + 1] == 0xff => 4,
                0xff => 2,
                _ => 1,
            };
            wire.truncate(message_id_start + message_id_len);
            wire
        })
        .collect()
}

fn object_payload() -> impl Strategy<Value = Vec<u8>> {
    let max_prefix_len = object_packet_prefixes().into_iter().map(|prefix| prefix.len()).max().unwrap_or(0);
    prop::collection::vec(any::<u8>(), 0..=(MAX_DATAGRAM_BYTES - max_prefix_len))
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        ..ProptestConfig::default()
    })]

    #[test]
    fn packet_decode_arbitrary(bytes in prop::collection::vec(any::<u8>(), 0..=MAX_DATAGRAM_BYTES)) {
        let _ = Packet::decode(&bytes);
    }

    #[test]
    fn object_decode_arbitrary(payload in object_payload()) {
        for mut packet in object_packet_prefixes() {
            packet.extend_from_slice(&payload);
            prop_assert!(packet.len() <= MAX_DATAGRAM_BYTES);
            let _ = Packet::decode(&packet);
        }
    }
}

#[test]
fn packet_rejects_oversized_input() {
    let bytes = vec![0; MAX_DATAGRAM_BYTES + 1];
    assert_eq!(Packet::decode(&bytes), Err(UdpError::DatagramTooLarge));
}
