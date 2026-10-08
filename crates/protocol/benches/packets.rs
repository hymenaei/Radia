use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use protocol::udp::{Message, MessageId, Packet};
use test_support::{parse_hex_fixture, udp::vectors};

fn fixture(hex: &str) -> Vec<u8> {
    parse_hex_fixture(hex).expect("valid packet fixture")
}

fn bench_packet_bytes(c: &mut Criterion, name: &str, wire: Vec<u8>) {
    let packet = Packet::decode(&wire).expect("decode packet fixture");
    let mut group = c.benchmark_group(format!("protocol/{name}"));
    group.throughput(Throughput::Bytes(wire.len() as u64));
    group.bench_function("encode", |b| {
        b.iter(|| black_box(packet.encode().expect("encode packet")));
    });
    group.bench_function("decode", |b| {
        b.iter(|| black_box(Packet::decode(black_box(wire.as_slice())).expect("decode packet")));
    });
    group.finish();
}

fn bench_rejected_packet(c: &mut Criterion, name: &str, wire: Vec<u8>) {
    assert!(Packet::decode(&wire).is_err(), "invalid packet fixture must be rejected");
    let mut group = c.benchmark_group(format!("protocol/{name}"));
    group.throughput(Throughput::Bytes(wire.len() as u64));
    group.bench_function("decode_reject", |b| {
        b.iter(|| black_box(Packet::decode(black_box(wire.as_slice()))));
    });
    group.finish();
}

fn bench_object_packets(c: &mut Criterion, name: &str, wire: Vec<u8>, objects: u64) {
    let packet = Packet::decode(&wire).expect("decode object fixture");
    let mut group = c.benchmark_group(format!("protocol/{name}"));
    group.throughput(Throughput::Elements(objects));
    group.bench_function("encode", |b| {
        b.iter(|| black_box(packet.encode().expect("encode object packet")));
    });
    group.bench_function("decode", |b| {
        b.iter(|| black_box(Packet::decode(black_box(wire.as_slice())).expect("decode object packet")));
    });
    group.finish();
}

fn bench_packets(c: &mut Criterion) {
    bench_object_packets(c, "object_update_full", fixture(vectors::OBJECT_UPDATE), 2);
    bench_object_packets(c, "object_update_compressed", fixture(vectors::OBJECT_UPDATE_COMPRESSED_TYPED), 1);
    bench_object_packets(c, "object_update_terse", fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE), 1);
    bench_object_packets(c, "object_update_terse_avatar", fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR), 1);
    bench_object_packets(c, "object_update_cached", fixture(vectors::OBJECT_UPDATE_CACHED), 2);
    bench_object_packets(c, "kill_object", fixture(vectors::KILL_OBJECT), 2);

    bench_packet_bytes(c, "packet_ack", fixture(vectors::PACKET_ACK));
    bench_packet_bytes(c, "appended_acks", fixture(vectors::APPENDED_ACKS));
    bench_packet_bytes(c, "request_multiple_objects", fixture(vectors::REQUEST_MULTIPLE_OBJECTS));
    bench_packet_bytes(c, "zerocode_at_limit", fixture(vectors::ZEROCODE_PACKET_AT_LIMIT));
    bench_rejected_packet(c, "zerocode_over_limit", fixture(vectors::TRANSFER_REQUEST_LARGE_ZEROCODED));
    bench_packet_bytes(c, "schema_fallback", fixture(vectors::COARSE_LOCATION_UPDATE));

    let unknown = Packet::new(7, Message::Unknown { id: MessageId::high(0x7e), payload: (0..256).map(|byte| byte as u8).collect() })
        .with_zero_coding(false)
        .encode()
        .expect("encode unknown fixture");
    bench_packet_bytes(c, "unknown_fallback", unknown);
}

criterion_group!(benches, bench_packets);
criterion_main!(benches);
