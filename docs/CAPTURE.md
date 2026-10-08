# Capture files

Radia writes v1 `.capture` ZIP archives containing raw network captures. Each archive has exactly three entries:

```text
manifest.json
events.jsonl
payloads.bin
```

## Recording

The `.capture` archive is raw. It may contain credentials, MFA values, account and session IDs, capability URLs, cookies, addresses, and message bodies. Keep it local. Never commit or share it.

On Windows, captures are stored under `%LOCALAPPDATA%\Radia\captures\`. Filenames use an eight-character random artifact ID such as `K7M4Q2PQ.capture`; collisions cause Radia to generate another ID.

Pass `--capture` at launch, as in `cargo run -- --capture`, to start recording with the next login.

## V1 bundle

`manifest.json` describes the finished export. Its compact top-level fields are:

| Field                    | Meaning                                                                                                  |
| ------------------------ | -------------------------------------------------------------------------------------------------------- |
| `schema_version`         | Public format version, currently `1`. Update readers, tests, and docs together when this format changes. |
| `viewer_version`         | Radia version that produced the capture.                                                                 |
| `started_at`, `ended_at` | RFC 3339 timestamps for the capture interval.                                                            |
| `event_count`            | Number of lines in `events.jsonl`.                                                                       |
| `dropped_events`         | Event records dropped when the bounded writer queue was full.                                            |
| `payload_bytes`          | Size of `payloads.bin` in bytes.                                                                         |

The manifest includes a `state` field that describes recording completion or interruption. Counts and byte lengths describe the files in the archive.

Each line of `events.jsonl` is one flat tagged event. Common fields are `index`, `elapsed_ns`, and `kind`. `index` starts at zero and increases by one. `elapsed_ns` is monotonic nanoseconds since recording began. `kind` identifies the event. Optional fields are omitted when empty. `sequence` is reserved for an actual Second Life UDP packet sequence number. An encoded line, including its newline, is limited to 1 MiB. Readers reject larger lines. The recorder marks a capture incomplete and reports an error if an event exceeds this limit. HTTP bodies are stored in `payloads.bin` and do not count toward the line limit.

Supported event kinds describe HTTP requests, responses, and errors; UDP sends and receives; and meaningful session transitions such as login completion, circuit establishment, session readiness, and logout. The event stream records protocol activity and lifecycle changes, not general viewer logs.

HTTP events use `request_id` to correlate a request with any recorded response or error. A request may have no terminal event if none was recorded before capture ended. This means Radia observed the request but captured no completion; it does not prove the operation was still in flight at the cutoff. Header fields are ordered `[name, value]` pairs. Names use a consistent normalized form, while order and duplicate names are preserved. Request and response bodies use payload references with role `body`.

UDP events use `udp_send` or `udp_receive`. They include available circuit, reliability, zerocoding, and decoded-message metadata. A decoded message name is optional. The datagram bytes are preserved exactly, including unknown or malformed packets. UDP payload references use role `datagram`.

`payloads.bin` is a plain append-only byte arena. Events refer to a range with a compact object such as `{"role":"body","offset":0,"length":1388}`. The format adds no framing or payload IDs. Every referenced range must fit inside the file. A single payload reference is limited to 64 MiB, and each archive entry is limited to 16 GiB.

Readers should ignore unknown JSON fields where possible. Blank lines are invalid. A valid bundle has contiguous event indexes, nondecreasing elapsed times, in-range payload references, and manifest counts and byte sizes that match the files.

## Offline capture checks

The default workspace tests remain offline and use the handwritten protocol fixtures. They never read local captures or contact a grid. The local archive check is marked `#[ignore]`; to run it against every `.capture` file under the user's local captures folder, use:

```powershell
cargo xtest -p viewer --run-ignored only capture
```

It checks completed local captures and skips interrupted or incomplete archives. If the folder has no `.capture` files, it fails with `No captures found`. If it contains no completed captures, it fails with `No complete captures found`. The command only reads and validates archives. It does not contact Second Life, modify captures, or add them to the repository. Plain `cargo test` does not scan the local captures folder.
