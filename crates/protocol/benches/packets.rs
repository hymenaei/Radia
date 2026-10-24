use std::hint::black_box;

use criterion::{BenchmarkGroup, BenchmarkId, Criterion, Throughput, measurement::WallTime};
use protocol::lludp::{Message, MessageId, Packet};
use test_support::{lludp::vectors, parse_hex_fixture};

pub fn bench(c: &mut Criterion) {
    bench_packets(c);
}

fn fixture(hex: &str) -> Vec<u8> {
    parse_hex_fixture(hex).expect("valid packet fixture")
}

fn bench_packet_bytes(group: &mut BenchmarkGroup<'_, WallTime>, name: &str, wire: Vec<u8>) {
    let packet = Packet::decode(&wire).expect("decode packet fixture");
    group.throughput(Throughput::Bytes(wire.len() as u64));
    group.bench_with_input(BenchmarkId::new("encode", name), &packet, |b, packet| {
        b.iter(|| black_box(packet.encode().expect("encode packet")));
    });
    group.bench_with_input(BenchmarkId::new("decode", name), &wire, |b, wire| {
        b.iter(|| black_box(Packet::decode(black_box(wire.as_slice())).expect("decode packet")));
    });
}

fn bench_object_packets(group: &mut BenchmarkGroup<'_, WallTime>, name: &str, wire: Vec<u8>, objects: u64) {
    let packet = Packet::decode(&wire).expect("decode object fixture");
    group.throughput(Throughput::Elements(objects));
    group.bench_with_input(BenchmarkId::new("encode", name), &packet, |b, packet| {
        b.iter(|| black_box(packet.encode().expect("encode object packet")));
    });
    group.bench_with_input(BenchmarkId::new("decode", name), &wire, |b, wire| {
        b.iter(|| black_box(Packet::decode(black_box(wire.as_slice())).expect("decode object packet")));
    });
}

fn bench_packets(c: &mut Criterion) {
    let mut objects = c.benchmark_group("protocol/lludp/objects");
    bench_object_packets(&mut objects, "update_full", fixture(vectors::OBJECT_UPDATE), 2);
    bench_object_packets(&mut objects, "update_compressed", fixture(vectors::OBJECT_UPDATE_COMPRESSED_TYPED), 1);
    bench_object_packets(&mut objects, "update_terse", fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE), 1);
    bench_object_packets(&mut objects, "update_terse_avatar", fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR), 1);
    bench_object_packets(&mut objects, "update_cached", fixture(vectors::OBJECT_UPDATE_CACHED), 2);
    bench_object_packets(&mut objects, "kill_object", fixture(vectors::KILL_OBJECT), 2);
    objects.finish();

    let mut packets = c.benchmark_group("protocol/lludp/packets");
    bench_packet_bytes(&mut packets, "packet_ack", fixture(vectors::PACKET_ACK));
    bench_packet_bytes(&mut packets, "appended_acks", fixture(vectors::APPENDED_ACKS));
    bench_packet_bytes(&mut packets, "request_multiple_objects", fixture(vectors::REQUEST_MULTIPLE_OBJECTS));
    bench_packet_bytes(&mut packets, "zerocode_at_limit", fixture(vectors::ZEROCODE_PACKET_AT_LIMIT));
    bench_packet_bytes(&mut packets, "schema_fallback", fixture(vectors::COARSE_LOCATION_UPDATE));

    let unknown = Packet::new(7, Message::Unknown { id: MessageId::high(0x7e), payload: (0..256).map(|byte| byte as u8).collect() })
        .with_zero_coding(false)
        .encode()
        .expect("encode unknown fixture");
    bench_packet_bytes(&mut packets, "unknown_fallback", unknown);
    packets.finish();

    let wire = fixture(vectors::TRANSFER_REQUEST_LARGE_ZEROCODED);
    assert!(Packet::decode(&wire).is_err(), "invalid packet fixture must be rejected");
    let mut rejections = c.benchmark_group("protocol/lludp/rejections");
    rejections.throughput(Throughput::Bytes(wire.len() as u64));
    rejections.bench_function("zerocode_over_limit", |b| {
        b.iter(|| black_box(Packet::decode(black_box(wire.as_slice()))));
    });
    rejections.finish();
}
