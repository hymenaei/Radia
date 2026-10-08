use std::{collections::BTreeMap, hint::black_box};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use llsd::{Binary, Map, URI, Value};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Deserialize, Serialize)]
#[serde(transparent)]
struct CapabilityMap(BTreeMap<String, URI>);

#[derive(Deserialize, Serialize)]
struct EventBatch {
    id: i32,
    events: Vec<EventMessage>,
}

#[derive(Deserialize, Serialize)]
struct EventMessage {
    message: String,
    body: Value,
}

fn capabilities() -> CapabilityMap {
    const NAMES: &[&str] = &[
        "ChatSessionRequest",
        "CopyInventoryFromNotecard",
        "CreateInventoryCategory",
        "EventQueueGet",
        "FetchInventory2",
        "GetAgentDisplayName",
        "GetAvatarPickerSearch",
        "GetDisplayNames",
        "GetExperienceInfo",
        "GetExperiences",
        "GetMesh",
        "GetObjectCost",
        "GetObjectPhysicsData",
        "GetTexture",
        "GroupMemberData",
        "MapLayer",
        "MapLayerGod",
        "NewFileAgentInventory",
        "RemoveInventoryObjects",
        "RequestTextureDownload",
        "Seed",
        "UpdateGestureAgentInventory",
        "UpdateInventoryItem",
        "UploadAgentInventory",
        "UploadAgentProfileImage",
        "UploadAgentPreferences",
        "UploadBakedTexture",
        "ViewerAsset",
        "ViewerStats",
        "UntrustedSimulatorMessage",
    ];

    CapabilityMap(
        NAMES
            .iter()
            .enumerate()
            .map(|(index, name)| ((*name).to_owned(), URI::from(format!("https://cap.example.invalid/region/{index:04}"))))
            .collect(),
    )
}

fn capability_value(capabilities: &CapabilityMap) -> Value {
    Value::Map(capabilities.0.iter().map(|(name, uri)| (name.clone(), Value::URI(uri.clone()))).collect())
}

fn event_batch() -> EventBatch {
    const TYPES: &[&str] = &["ObjectUpdate", "ChatFromSimulator", "ParcelProperties", "CoarseLocationUpdate"];

    let events = (0..48)
        .map(|index| {
            let mut body = Map::new();
            body.insert("local_id".into(), Value::Integer(index));
            body.insert("name".into(), Value::String(format!("Object {index:03}")));
            body.insert(
                "position".into(),
                Value::Array(vec![
                    Value::Real(index as f64 * 1.25),
                    Value::Real(index as f64 * -0.5),
                    Value::Real(20.0 + f64::from(index)),
                ]),
            );
            body.insert("flags".into(), Value::Integer(index % 16));
            body.insert("visible".into(), Value::Boolean(index % 3 != 0));
            body.insert("payload".into(), Value::Binary(Binary::new(vec![(index % 251) as u8; 24])));

            EventMessage { message: TYPES[index as usize % TYPES.len()].to_owned(), body: Value::Map(body) }
        })
        .collect();

    EventBatch { id: 8192, events }
}

fn event_batch_value(batch: &EventBatch) -> Value {
    let events = batch
        .events
        .iter()
        .map(|event| {
            Value::Map(Map::from([("body".to_owned(), event.body.clone()), ("message".to_owned(), Value::String(event.message.clone()))]))
        })
        .collect();

    Value::Map(Map::from([("events".to_owned(), Value::Array(events)), ("id".to_owned(), Value::Integer(batch.id))]))
}

fn bench_format<T>(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name: &str,
    value: &T,
    input: &[u8],
    encode: impl Fn(&T) -> llsd::Result<Vec<u8>>,
    decode: impl Fn(&[u8]) -> llsd::Result<T>,
) where
    T: Serialize + DeserializeOwned,
{
    group.throughput(Throughput::Bytes(input.len() as u64));
    group.bench_with_input(BenchmarkId::new(name, "encode"), value, |b, value| {
        b.iter(|| black_box(encode(black_box(value)).expect("encode LLSD")));
    });
    group.bench_with_input(BenchmarkId::new(name, "decode"), input, |b, input| {
        b.iter(|| black_box(decode(black_box(input)).expect("decode LLSD")));
    });
}

fn bench_xml<T>(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>, name: &str, value: &T, input: &[u8])
where
    T: Serialize + DeserializeOwned,
{
    bench_format(group, name, value, input, llsd::xml::to_vec, llsd::xml::from_slice::<T>);

    group.throughput(Throughput::Bytes(input.len() as u64));
    group.bench_with_input(BenchmarkId::new(name, "bounded_encode"), value, |b, value| {
        b.iter(|| black_box(llsd::xml::to_vec_with_max_bytes(black_box(value), input.len()).expect("bounded LLSD XML encode")));
    });
}

fn bench_codec(c: &mut Criterion) {
    let capabilities = capabilities();
    let capability_value = capability_value(&capabilities);
    let typed_capability_xml = llsd::xml::to_vec(&capabilities).expect("encode capability fixture");
    let dynamic_capability_xml = llsd::xml::to_vec(&capability_value).expect("encode dynamic capability fixture");
    let typed_capability_binary = llsd::binary::to_vec(&capabilities).expect("encode binary capability fixture");
    let dynamic_capability_binary = llsd::binary::to_vec(&capability_value).expect("encode dynamic binary capability fixture");
    let typed_capability_notation = llsd::notation::to_vec(&capabilities).expect("encode notation capability fixture");
    let dynamic_capability_notation = llsd::notation::to_vec(&capability_value).expect("encode dynamic notation capability fixture");

    let mut group = c.benchmark_group("llsd/capabilities");
    bench_xml(&mut group, "typed", &capabilities, &typed_capability_xml);
    bench_xml(&mut group, "value", &capability_value, &dynamic_capability_xml);
    bench_format(
        &mut group,
        "binary_typed",
        &capabilities,
        &typed_capability_binary,
        llsd::binary::to_vec,
        llsd::binary::from_slice::<CapabilityMap>,
    );
    bench_format(
        &mut group,
        "binary_value",
        &capability_value,
        &dynamic_capability_binary,
        llsd::binary::to_vec,
        llsd::binary::from_slice::<Value>,
    );
    bench_format(
        &mut group,
        "notation_typed",
        &capabilities,
        &typed_capability_notation,
        llsd::notation::to_vec,
        llsd::notation::from_slice::<CapabilityMap>,
    );
    bench_format(
        &mut group,
        "notation_value",
        &capability_value,
        &dynamic_capability_notation,
        llsd::notation::to_vec,
        llsd::notation::from_slice::<Value>,
    );
    group.finish();

    let batch = event_batch();
    let batch_value = event_batch_value(&batch);
    let typed_batch_xml = llsd::xml::to_vec(&batch).expect("encode event batch fixture");
    let dynamic_batch_xml = llsd::xml::to_vec(&batch_value).expect("encode dynamic event batch fixture");
    let typed_batch_binary = llsd::binary::to_vec(&batch).expect("encode binary event batch fixture");
    let dynamic_batch_binary = llsd::binary::to_vec(&batch_value).expect("encode dynamic binary event batch fixture");
    let typed_batch_notation = llsd::notation::to_vec(&batch).expect("encode notation event batch fixture");
    let dynamic_batch_notation = llsd::notation::to_vec(&batch_value).expect("encode dynamic notation event batch fixture");

    let mut group = c.benchmark_group("llsd/event_batch");
    bench_xml(&mut group, "typed", &batch, &typed_batch_xml);
    bench_xml(&mut group, "value", &batch_value, &dynamic_batch_xml);
    bench_format(&mut group, "binary_typed", &batch, &typed_batch_binary, llsd::binary::to_vec, llsd::binary::from_slice::<EventBatch>);
    bench_format(&mut group, "binary_value", &batch_value, &dynamic_batch_binary, llsd::binary::to_vec, llsd::binary::from_slice::<Value>);
    bench_format(
        &mut group,
        "notation_typed",
        &batch,
        &typed_batch_notation,
        llsd::notation::to_vec,
        llsd::notation::from_slice::<EventBatch>,
    );
    bench_format(
        &mut group,
        "notation_value",
        &batch_value,
        &dynamic_batch_notation,
        llsd::notation::to_vec,
        llsd::notation::from_slice::<Value>,
    );
    group.finish();

    let inventory = inventory_batch();
    let inventory_xml = llsd::xml::to_vec(&inventory).expect("encode inventory-like fixture");
    let inventory_binary = llsd::binary::to_vec(&inventory).expect("encode binary inventory-like fixture");
    let inventory_notation = llsd::notation::to_vec(&inventory).expect("encode notation inventory-like fixture");
    let mut group = c.benchmark_group("llsd/inventory_batch");
    bench_xml(&mut group, "value", &inventory, &inventory_xml);
    bench_format(&mut group, "binary_value", &inventory, &inventory_binary, llsd::binary::to_vec, llsd::binary::from_slice::<Value>);
    bench_format(
        &mut group,
        "notation_value",
        &inventory,
        &inventory_notation,
        llsd::notation::to_vec,
        llsd::notation::from_slice::<Value>,
    );
    group.finish();

    let nested = nested_array_value(60);
    let nested_xml = llsd::xml::to_vec(&nested).expect("encode nested fixture");
    let nested_binary = llsd::binary::to_vec(&nested).expect("encode nested binary fixture");
    let nested_notation = llsd::notation::to_vec(&nested).expect("encode nested notation fixture");
    let mut group = c.benchmark_group("llsd/nested_near_default_limit");
    bench_xml(&mut group, "value", &nested, &nested_xml);
    bench_format(&mut group, "binary_value", &nested, &nested_binary, llsd::binary::to_vec, llsd::binary::from_slice::<Value>);
    bench_format(&mut group, "notation_value", &nested, &nested_notation, llsd::notation::to_vec, llsd::notation::from_slice::<Value>);
    group.finish();
}

fn inventory_batch() -> Value {
    let items = (0..96)
        .map(|index| {
            Value::Map(Map::from([
                ("name".to_owned(), Value::String(format!("Inventory item {index:03}"))),
                ("description".to_owned(), Value::String("viewer-created inventory entry ".repeat(8))),
                ("asset_data".to_owned(), Value::Binary(Binary::new(vec![(index % 251) as u8; 512]))),
                ("permissions".to_owned(), Value::Integer(index % 16)),
            ]))
        })
        .collect();
    Value::Map(Map::from([("items".to_owned(), Value::Array(items)), ("version".to_owned(), Value::Integer(1))]))
}

fn nested_array_value(depth: usize) -> Value {
    (0..depth).fold(Value::Integer(1), |value, _| Value::Array(vec![value]))
}

criterion_group!(benches, bench_codec);
criterion_main!(benches);
