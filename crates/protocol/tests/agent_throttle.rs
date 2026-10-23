use protocol::{
    lludp::{LLUdpError, Message, Packet},
    messages::{
        AgentThrottle, ThrottleRates,
        agent_throttle::{AgentData, Throttle},
    },
};
use testing::{lludp::vectors, parse_hex_fixture};
use uuid::Uuid;

const AGENT_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
const SESSION_ID: &str = "00112233-4455-6677-8899-aabbccddeeff";

fn fixture_message() -> AgentThrottle {
    AgentThrottle {
        agent_data: AgentData {
            agent_id: Uuid::parse_str(AGENT_ID).expect("valid fixture agent UUID"),
            session_id: Uuid::parse_str(SESSION_ID).expect("valid fixture session UUID"),
            circuit_code: 0x1122_3344,
        },
        throttle: Throttle {
            gen_counter: 0,
            throttles: ThrottleRates {
                resend_bits_per_second: 1.0,
                land_bits_per_second: 2.0,
                wind_bits_per_second: 3.0,
                cloud_bits_per_second: 4.0,
                task_bits_per_second: 5.0,
                texture_bits_per_second: 6.0,
                asset_bits_per_second: 7.0,
            },
        },
    }
}

#[test]
fn agent_throttle_preserves_bit_rates_in_wire_order() {
    let expected = parse_hex_fixture(vectors::AGENT_THROTTLE).expect("valid AgentThrottle fixture");
    let packet = Packet::new(3, fixture_message().into()).with_reliable().with_zero_coding(false);

    assert_eq!(packet.encode().expect("encode AgentThrottle"), expected);
    let decoded = Packet::decode(&expected).expect("decode AgentThrottle fixture");
    assert_eq!(decoded.message(), &Message::AgentThrottle(fixture_message()));
}

#[test]
fn agent_throttle_encodes_bits_per_second() {
    let mut message = fixture_message();
    message.throttle.throttles.resend_bits_per_second = 8_192.0;
    let encoded = Packet::new(3, message.into()).with_reliable().with_zero_coding(false).encode().expect("encode AgentThrottle");

    assert_eq!(&encoded[51..55], &8_192.0_f32.to_le_bytes());
}

#[test]
fn agent_throttle_rejects_invalid_rate_shape_and_values() {
    let mut short_rates = parse_hex_fixture(vectors::AGENT_THROTTLE).expect("valid AgentThrottle fixture");
    short_rates[50] = 4;
    assert_eq!(Packet::decode(&short_rates), Err(LLUdpError::InvalidMessage));

    let mut nan_rate = parse_hex_fixture(vectors::AGENT_THROTTLE).expect("valid AgentThrottle fixture");
    nan_rate[51..55].copy_from_slice(&f32::NAN.to_le_bytes());
    assert_eq!(Packet::decode(&nan_rate), Err(LLUdpError::InvalidMessage));

    let mut negative_rate = fixture_message();
    negative_rate.throttle.throttles.resend_bits_per_second = -1.0;
    assert_eq!(Packet::new(3, negative_rate.into()).encode(), Err(LLUdpError::InvalidMessage));
}
