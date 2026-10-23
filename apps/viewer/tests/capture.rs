use std::{
    collections::HashSet,
    error::Error,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use network::capture::{
    CaptureEvent, CaptureEventKind, CaptureManifest, CapturePayloadRef, CaptureState, MAX_CAPTURE_ENTRY_BYTES, MAX_CAPTURE_EVENT_LINE_BYTES,
};
use protocol::lludp::{Message, Packet};
use serde_json::{from_slice, from_str, to_writer};
use tempfile::{Builder, NamedTempFile, tempdir};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

const CAPTURE_ENTRIES: [&str; 3] = ["manifest.json", "events.jsonl", "payloads.bin"];
const MAX_PACKET_BYTES: u64 = 64 * 1024 * 1024;

struct Summary {
    events: u64,
    udp: u64,
    decoded_lludp: u64,
    malformed_lludp: u64,
    http_requests: u64,
    http_responses: u64,
}

#[test]
fn valid_capture_decodes_lludp() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let capture_path = directory.path().join("valid.capture");
    let bytes = test_support::parse_hex_fixture(test_support::lludp::vectors::START_PING_CHECK)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let event = CaptureEvent {
        index: 0,
        elapsed_ns: 0,
        kind: CaptureEventKind::LLUdpReceive {
            circuit_id: 1,
            endpoint: "simulator".into(),
            sequence: Some(0x0102_0304),
            reliable: Some(false),
            resent: Some(false),
            zerocoded: Some(false),
            appended_acks: Vec::new(),
            packet_acks: Vec::new(),
            message: None,
        },
        payloads: vec![CapturePayloadRef { role: "datagram".into(), offset: 0, length: bytes.len() as u64 }],
    };
    let mut event_line = Vec::new();
    to_writer(&mut event_line, &event)?;
    event_line.push(b'\n');
    write_capture(&capture_path, 1, &event_line, &bytes)?;
    fs::write(directory.path().join("notes.txt"), b"not a capture")?;

    let captures = capture_files(directory.path())?;
    assert_eq!(captures, [capture_path]);
    let summary = inspect(&captures[0], directory.path())?.ok_or("complete fixture was skipped")?;
    assert_eq!(summary.events, 1);
    assert_eq!(summary.udp, 1);
    assert_eq!(summary.decoded_lludp, 1);
    assert_eq!(summary.malformed_lludp, 0);
    assert_eq!(summary.http_requests, 0);
    assert_eq!(summary.http_responses, 0);
    Ok(())
}

#[test]
fn accepts_missing_packet_metadata() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let bytes = test_support::parse_hex_fixture(test_support::lludp::vectors::START_PING_CHECK)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let payload_path = directory.path().join("payloads.bin");
    File::create(&payload_path)?.write_all(&bytes)?;
    let payload_file = File::open(payload_path)?;
    let event = CaptureEvent {
        index: 0,
        elapsed_ns: 0,
        kind: CaptureEventKind::LLUdpReceive {
            circuit_id: 1,
            endpoint: "simulator".into(),
            sequence: None,
            reliable: None,
            resent: None,
            zerocoded: None,
            appended_acks: Vec::new(),
            packet_acks: Vec::new(),
            message: None,
        },
        payloads: vec![CapturePayloadRef { role: "datagram".into(), offset: 0, length: bytes.len() as u64 }],
    };
    let mut summary = Summary { events: 0, udp: 1, decoded_lludp: 0, malformed_lludp: 0, http_requests: 0, http_responses: 0 };

    inspect_datagram(&event, &payload_file, &mut summary)?;

    assert_eq!(summary.decoded_lludp, 1);
    assert_eq!(summary.malformed_lludp, 0);
    Ok(())
}

#[test]
fn rejects_oversized_event_line() -> Result<(), Box<dyn Error>> {
    let directory = tempdir()?;
    let capture_path = directory.path().join("oversized.capture");
    let oversized_line = vec![b' '; MAX_CAPTURE_EVENT_LINE_BYTES + 1];
    write_capture(&capture_path, 0, &oversized_line, &[])?;

    let error = inspect(&capture_path, directory.path()).err().ok_or("oversized event line was accepted")?;
    assert!(error.to_string().contains("event line exceeds"));
    Ok(())
}

fn capture_files(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.is_file() && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("capture")) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn inspect(path: &Path, temp_dir: &Path) -> Result<Option<Summary>, Box<dyn Error>> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)?;
    if archive.len() != CAPTURE_ENTRIES.len() {
        return Err("Archive must contain manifest.json, events.jsonl, and payloads.bin".into());
    }
    for (index, expected) in CAPTURE_ENTRIES.iter().enumerate() {
        if archive.by_index(index)?.name() != *expected {
            return Err("Archive entries must be manifest.json, events.jsonl, and payloads.bin".into());
        }
    }

    let manifest: CaptureManifest = read_json_entry(&mut archive, "manifest.json")?;
    if manifest.schema_version != 1 || manifest.viewer_version.is_empty() {
        return Err("Manifest has an unsupported schema or missing viewer version".into());
    }
    if manifest.state != CaptureState::Complete {
        return Ok(None);
    }
    if manifest.dropped_events != 0 {
        return Err("Complete capture reports dropped events".into());
    }
    let started = OffsetDateTime::parse(&manifest.started_at, &Rfc3339)?;
    let ended = OffsetDateTime::parse(&manifest.ended_at, &Rfc3339)?;
    if ended < started {
        return Err("Capture end time precedes its start time".into());
    }

    let payload_file = extract_payloads(&mut archive, temp_dir)?;
    if payload_file.as_file().metadata()?.len() != manifest.payload_bytes {
        return Err("Payload_bytes does not match payloads.bin".into());
    }

    let mut event_file = archive.by_name("events.jsonl")?;
    if event_file.encrypted() || event_file.size() > MAX_CAPTURE_ENTRY_BYTES {
        return Err("Events.jsonl is encrypted or exceeds the entry size limit".into());
    }
    let mut events = BufReader::new(&mut event_file);
    let mut summary = Summary { events: 0, udp: 0, decoded_lludp: 0, malformed_lludp: 0, http_requests: 0, http_responses: 0 };
    let mut elapsed_ns = 0;
    let mut request_ids = HashSet::new();
    let mut line = String::new();
    loop {
        line.clear();
        let line_bytes = (&mut events).take((MAX_CAPTURE_EVENT_LINE_BYTES + 1) as u64).read_line(&mut line)?;
        if line_bytes == 0 {
            break;
        }
        if line_bytes > MAX_CAPTURE_EVENT_LINE_BYTES {
            return Err("Events.jsonl event line exceeds the size limit".into());
        }
        if line.trim().is_empty() {
            return Err("Events.jsonl contains a blank line".into());
        }
        let event: CaptureEvent = from_str(&line)?;
        if event.index != summary.events || event.elapsed_ns < elapsed_ns {
            return Err("Event indexes or elapsed times are invalid".into());
        }
        elapsed_ns = event.elapsed_ns;
        check_payload_ranges(&event, manifest.payload_bytes)?;

        match &event.kind {
            CaptureEventKind::LLUdpSend { .. } | CaptureEventKind::LLUdpReceive { .. } => {
                summary.udp += 1;
                let direction = if matches!(&event.kind, CaptureEventKind::LLUdpSend { .. }) { "send" } else { "receive" };
                inspect_datagram(&event, payload_file.as_file(), &mut summary).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("viewer version {} event {} {direction}: {error}", manifest.viewer_version, event.index),
                    )
                })?;
            }
            CaptureEventKind::HttpRequest { request_id, .. } => {
                if !request_ids.insert(*request_id) {
                    return Err("Duplicate HTTP request_id".into());
                }
                summary.http_requests += 1;
                check_body_refs(&event)?;
            }
            CaptureEventKind::HttpResponse { request_id, .. } => {
                if !request_ids.contains(request_id) {
                    return Err("HTTP response has no captured request".into());
                }
                summary.http_responses += 1;
                check_body_refs(&event)?;
            }
            CaptureEventKind::HttpError { request_id, .. } => {
                if !request_ids.contains(request_id) {
                    return Err("HTTP error has no captured request".into());
                }
                if !event.payloads.is_empty() {
                    return Err("HTTP error must not reference a body".into());
                }
            }
            _ if !event.payloads.is_empty() => return Err("lifecycle event has payload data".into()),
            _ => {}
        }
        summary.events += 1;
    }

    if summary.events != manifest.event_count {
        return Err("Event_count does not match events.jsonl".into());
    }
    Ok(Some(summary))
}

fn read_json_entry<T: serde::de::DeserializeOwned>(archive: &mut ZipArchive<File>, name: &str) -> Result<T, Box<dyn Error>> {
    let mut entry = archive.by_name(name)?;
    if entry.encrypted() || entry.size() > MAX_CAPTURE_ENTRY_BYTES {
        return Err(format!("{name} is encrypted or exceeds the entry size limit").into());
    }
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes)?;
    Ok(from_slice(&bytes)?)
}

fn extract_payloads(archive: &mut ZipArchive<File>, temp_dir: &Path) -> Result<NamedTempFile, Box<dyn Error>> {
    let mut entry = archive.by_name("payloads.bin")?;
    if entry.encrypted() || entry.size() > MAX_CAPTURE_ENTRY_BYTES {
        return Err("Payloads.bin is encrypted or exceeds the entry size limit".into());
    }
    let expected_bytes = entry.size();
    let mut file = create_temp_payload(temp_dir)?;
    let copied_bytes = io::copy(&mut entry, file.as_file_mut())?;
    if copied_bytes != expected_bytes {
        return Err("Payloads.bin length does not match its ZIP entry".into());
    }
    file.as_file_mut().seek(SeekFrom::Start(0))?;
    Ok(file)
}

fn create_temp_payload(temp_dir: &Path) -> Result<NamedTempFile, Box<dyn Error>> {
    Ok(Builder::new().prefix("capture-check-").suffix(".bin").tempfile_in(temp_dir)?)
}

fn write_capture(path: &Path, event_count: u64, events: &[u8], payloads: &[u8]) -> Result<(), Box<dyn Error>> {
    let manifest = CaptureManifest {
        schema_version: 1,
        viewer_version: "test".into(),
        started_at: "2025-01-01T00:00:00Z".into(),
        ended_at: "2025-01-01T00:00:00Z".into(),
        event_count,
        dropped_events: 0,
        payload_bytes: payloads.len() as u64,
        state: CaptureState::Complete,
    };
    let mut archive = ZipWriter::new(File::create(path)?);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    archive.start_file(CAPTURE_ENTRIES[0], options)?;
    to_writer(&mut archive, &manifest)?;
    archive.start_file(CAPTURE_ENTRIES[1], options)?;
    archive.write_all(events)?;
    archive.start_file(CAPTURE_ENTRIES[2], options)?;
    archive.write_all(payloads)?;
    drop(archive.finish()?);
    Ok(())
}

fn check_payload_ranges(event: &CaptureEvent, payload_bytes: u64) -> Result<(), Box<dyn Error>> {
    for reference in &event.payloads {
        let end = reference.offset.checked_add(reference.length).ok_or("payload range overflows")?;
        if end > payload_bytes {
            return Err("Payload reference lies outside payloads.bin".into());
        }
    }
    match &event.kind {
        CaptureEventKind::LLUdpSend { .. } | CaptureEventKind::LLUdpReceive { .. } => {
            if event.payloads.len() != 1 || event.payloads[0].role != "datagram" {
                return Err("UDP event must reference one datagram".into());
            }
        }
        CaptureEventKind::HttpRequest { .. } | CaptureEventKind::HttpResponse { .. } => check_body_refs(event)?,
        _ if !event.payloads.is_empty() => return Err("event has an unexpected payload reference".into()),
        _ => {}
    }
    Ok(())
}

fn check_body_refs(event: &CaptureEvent) -> Result<(), Box<dyn Error>> {
    if event.payloads.len() > 1 || event.payloads.iter().any(|reference| reference.role != "body") {
        return Err("HTTP event has invalid body references".into());
    }
    Ok(())
}

fn inspect_datagram(event: &CaptureEvent, payload_file: &File, summary: &mut Summary) -> Result<(), Box<dyn Error>> {
    let reference: &CapturePayloadRef = &event.payloads[0];
    if reference.length > MAX_PACKET_BYTES {
        return Err("UDP datagram exceeds the local test limit".into());
    }
    let length = usize::try_from(reference.length)?;
    let mut bytes = vec![0; length];
    let mut payload_file = payload_file;
    payload_file.seek(SeekFrom::Start(reference.offset))?;
    payload_file.read_exact(&mut bytes)?;

    let (sequence, reliable, resent, zerocoded, appended_acks, packet_acks, message) = match &event.kind {
        CaptureEventKind::LLUdpSend { sequence, reliable, resent, zerocoded, appended_acks, packet_acks, message, .. }
        | CaptureEventKind::LLUdpReceive { sequence, reliable, resent, zerocoded, appended_acks, packet_acks, message, .. } => {
            (sequence, reliable, resent, zerocoded, appended_acks, packet_acks, message)
        }
        _ => return Err("Expected UDP event".into()),
    };
    let has_metadata = sequence.is_some()
        || reliable.is_some()
        || resent.is_some()
        || zerocoded.is_some()
        || !appended_acks.is_empty()
        || !packet_acks.is_empty()
        || message.is_some();

    match Packet::decode(&bytes) {
        Ok(packet) => {
            summary.decoded_lludp += 1;
            if !has_metadata {
                return Ok(());
            }
            let mut mismatches = Vec::new();
            if *sequence != Some(packet.sequence()) {
                mismatches.push(format!("sequence {}", if sequence.is_none() { "missing" } else { "different" }));
            }
            if *reliable != Some(packet.is_reliable()) {
                mismatches.push(format!("reliable {}", if reliable.is_none() { "missing" } else { "different" }));
            }
            if *resent != Some(packet.is_resent()) {
                mismatches.push(format!("resent {}", if resent.is_none() { "missing" } else { "different" }));
            }
            if *zerocoded != Some(packet.is_zerocoded()?) {
                mismatches.push(format!("zerocoded {}", if zerocoded.is_none() { "missing" } else { "different" }));
            }
            if appended_acks.as_slice() != packet.appended_acks() {
                mismatches.push("appended_acks different".into());
            }
            if !mismatches.is_empty() {
                return Err(format!("LLUDP metadata disagrees with its UDP datagram: {}", mismatches.join(", ")).into());
            }
            let decoded_acks = match packet.message() {
                Message::PacketAck(message) => message.packets.iter().map(|ack| ack.id).collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            if packet_acks.as_slice() != decoded_acks.as_slice() {
                return Err("Packet ACK metadata disagrees with its datagram".into());
            }
            if message.is_some() && matches!(packet.message(), Message::Unknown { .. }) {
                return Err("LLUDP message name is present for an unknown message".into());
            }
        }
        Err(_) => {
            summary.malformed_lludp += 1;
            if has_metadata {
                return Err("Undecodable UDP datagram has decoded LLUDP metadata".into());
            }
        }
    }
    Ok(())
}
