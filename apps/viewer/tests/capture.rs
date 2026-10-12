use std::{
    collections::HashSet,
    env,
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
use tempfile::{Builder, NamedTempFile};
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
#[ignore]
fn saved_captures_are_valid() -> Result<(), Box<dyn Error>> {
    let root = capture_root()?;
    let captures = capture_files(&root)?;
    if captures.is_empty() {
        return Err("No captures found".into());
    }

    let mut completed = 0;
    for path in captures {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("capture");
        let Some(summary) = inspect(&path)? else {
            println!("{name}: skipped (capture is not complete)");
            continue;
        };
        completed += 1;
        println!(
            "{name}: {} events, {} UDP datagrams ({} decoded LLUDP, {} malformed), {} HTTP requests, {} responses",
            summary.events, summary.udp, summary.decoded_lludp, summary.malformed_lludp, summary.http_requests, summary.http_responses
        );
    }
    if completed == 0 {
        return Err("No complete captures found".into());
    }
    Ok(())
}

#[test]
fn rejects_oversized_event_line() -> Result<(), Box<dyn Error>> {
    let capture = Builder::new().prefix("capture-test-").suffix(".capture").tempfile()?;
    let mut archive = ZipWriter::new(capture.reopen()?);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let manifest = CaptureManifest {
        schema_version: 1,
        viewer_version: "test".into(),
        started_at: "2025-01-01T00:00:00Z".into(),
        ended_at: "2025-01-01T00:00:00Z".into(),
        event_count: 0,
        dropped_events: 0,
        payload_bytes: 0,
        state: CaptureState::Complete,
    };

    archive.start_file(CAPTURE_ENTRIES[0], options)?;
    to_writer(&mut archive, &manifest)?;
    archive.start_file(CAPTURE_ENTRIES[1], options)?;
    let oversized_line = vec![b' '; MAX_CAPTURE_EVENT_LINE_BYTES + 1];
    archive.write_all(&oversized_line)?;
    archive.start_file(CAPTURE_ENTRIES[2], options)?;
    archive.finish()?;

    let error = inspect(capture.path()).err().ok_or("oversized event line was accepted")?;
    assert!(error.to_string().contains("event line exceeds"));
    Ok(())
}

fn capture_root() -> Result<PathBuf, Box<dyn Error>> {
    #[cfg(windows)]
    let data_dir = env::var_os("LOCALAPPDATA").map(PathBuf::from).ok_or("LOCALAPPDATA is not set")?;

    #[cfg(not(windows))]
    let data_dir = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or("No local data directory is available")?;

    Ok(data_dir.join("Radia").join("captures"))
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

fn inspect(path: &Path) -> Result<Option<Summary>, Box<dyn Error>> {
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

    let payload_file = extract_payloads(&mut archive)?;
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
                inspect_datagram(&event, payload_file.as_file(), &mut summary)?;
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

fn extract_payloads(archive: &mut ZipArchive<File>) -> Result<NamedTempFile, Box<dyn Error>> {
    let mut entry = archive.by_name("payloads.bin")?;
    if entry.encrypted() || entry.size() > MAX_CAPTURE_ENTRY_BYTES {
        return Err("Payloads.bin is encrypted or exceeds the entry size limit".into());
    }
    let expected_bytes = entry.size();
    let mut file = create_temp_payload()?;
    let copied_bytes = io::copy(&mut entry, file.as_file_mut())?;
    if copied_bytes != expected_bytes {
        return Err("Payloads.bin length does not match its ZIP entry".into());
    }
    file.as_file_mut().seek(SeekFrom::Start(0))?;
    Ok(file)
}

fn create_temp_payload() -> Result<NamedTempFile, Box<dyn Error>> {
    Ok(Builder::new().prefix("capture-check-").suffix(".bin").tempfile()?)
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

    match Packet::decode(&bytes) {
        Ok(packet) => {
            summary.decoded_lludp += 1;
            if *sequence != Some(packet.sequence())
                || *reliable != Some(packet.is_reliable())
                || *resent != Some(packet.is_resent())
                || *zerocoded != Some(packet.is_zerocoded()?)
                || appended_acks.as_slice() != packet.appended_acks()
            {
                return Err("LLUDP metadata disagrees with its UDP datagram".into());
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
            if sequence.is_some()
                || reliable.is_some()
                || resent.is_some()
                || zerocoded.is_some()
                || !appended_acks.is_empty()
                || !packet_acks.is_empty()
                || message.is_some()
            {
                return Err("Undecodable UDP datagram has decoded LLUDP metadata".into());
            }
        }
    }
    Ok(())
}
