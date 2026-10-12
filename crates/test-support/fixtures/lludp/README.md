These LLUDP vectors come from published protocol and viewer sources. They are not captured traffic. Their bytes were assembled independently of Radia's codecs.

## Sources

Message definitions come from Linden Lab's [master message template](https://github.com/secondlife/master-message-template/blob/master/message_template.msg). The Second Life [Packet Layout](https://wiki.secondlife.com/wiki/Packet_Layout) describes packet headers, message IDs, byte order, zero coding, and appended ACKs.

The checked-in template at `crates/protocol/build/message_template.msg` was retrieved on 2026-10-04. Its SHA-256 is `1D133208D8EFD2B9761759EAC9188E30C9853E381A6493E585F9E306343AE6F4`. `build.rs` parses it offline to generate message IDs and schema metadata. Typed message codecs remain hand-written.

The object update layouts and quantization follow official viewer revision [`7dd6de6120ce7a80b0a290d34a701e8717d03962`](https://github.com/secondlife/viewer/tree/7dd6de6120ce7a80b0a290d34a701e8717d03962). The references are [`llviewerobjectlist.cpp`](https://github.com/secondlife/viewer/blob/7dd6de6120ce7a80b0a290d34a701e8717d03962/indra/newview/llviewerobjectlist.cpp#L441-L484), [`llviewerobject.cpp`](https://github.com/secondlife/viewer/blob/7dd6de6120ce7a80b0a290d34a701e8717d03962/indra/newview/llviewerobject.cpp#L1301-L1337), its [terse and compressed paths](https://github.com/secondlife/viewer/blob/7dd6de6120ce7a80b0a290d34a701e8717d03962/indra/newview/llviewerobject.cpp#L1502-L1706), and [`llquaternion.cpp`](https://github.com/secondlife/viewer/blob/7dd6de6120ce7a80b0a290d34a701e8717d03962/indra/llmath/llquaternion.cpp#L862-L901).

## Wire behavior

- Fixtures set packet flags explicitly. The sender chooses reliability. Viewers do not zero-code `ChatFromViewer`, although the template marks it as zerocoded; the explicit override fixture tests that codec path.
- UUID fields use `LLUUID` byte order. Multi-byte fields follow the protocol's field-specific byte order.
- Typed string fields include a trailing NUL in their length. Decoding stops at the first NUL and rejects invalid UTF-8 before it. Generic schema `Variable(n)` fields remain bytes and preserve embedded zeroes.
- The official viewer reads `Variable 2` lengths as little-endian. The [`ChatFromSimulator`](https://wiki.secondlife.com/wiki/ChatFromSimulator) reference agrees; the general [Message Layout](https://wiki.secondlife.com/wiki/Message_Layout) page says big-endian. These vectors follow the viewer.
- The viewer accepts `00 00 N` as `255 + N` zero bytes. Radia accepts this form and emits canonical `00 N` runs. See the [viewer decoder](https://github.com/secondlife/viewer/blob/bd9990e3db5c640e2c7fa26651167fc43e1ee37/indra/llmessage/llzerocode.h).
- `RegionHandshake` fixtures cover the current `RegionInfo2`, `RegionInfo3`, and optional `RegionInfo4` blocks, plus the older layout. Zero-filled runs use wire zero coding.

These string fields use Rust `String` because the template marks them as strings and the viewer reads them with string accessors. Other variable fields remain raw bytes.

| Wire field | Template width | Rust field | Official viewer use |
| --- | --- | --- | --- |
| `ChatFromSimulator.ChatData.FromName` | Variable 1 | `chat_data.from_name: String` | [`llviewermessage.cpp`](https://github.com/secondlife/viewer/blob/develop/indra/newview/llviewermessage.cpp), `getString` |
| `ChatFromSimulator.ChatData.Message` | Variable 2 | `chat_data.message: String` | [`llviewermessage.cpp`](https://github.com/secondlife/viewer/blob/develop/indra/newview/llviewermessage.cpp), `getString` |
| `ChatFromViewer.ChatData.Message` | Variable 2 | `chat_data.message: String` | [`llnearbychatbar.cpp`](https://github.com/secondlife/viewer/blob/develop/indra/newview/llnearbychatbar.cpp), `addStringFast` |
| `AgentMovementComplete.SimData.ChannelVersion` | Variable 2 | `sim_data.channel_version: String` | [`llviewermessage.cpp`](https://github.com/secondlife/viewer/blob/develop/indra/newview/llviewermessage.cpp), `getString` |
| `RegionHandshake.RegionInfo.SimName` | Variable 1 | `region_info1.sim_name: String` | [`llviewerregion.cpp`](https://github.com/secondlife/viewer/blob/develop/indra/newview/llviewerregion.cpp), `getString` |
| `RegionHandshake.RegionInfo3.ColoName`, `ProductSKU`, `ProductName` | Variable 1 | `region_info3` string fields | [`llviewerregion.cpp`](https://github.com/secondlife/viewer/blob/develop/indra/newview/llviewerregion.cpp), `getString` |
| `MapBlockReply.Data.Name` | Variable 1 | `data[].name: String` | [`llworldmapmessage.cpp`](https://github.com/secondlife/viewer/blob/develop/indra/newview/llworldmapmessage.cpp), `getStringFast` |

## Edge cases

Fixture names identify their baseline message. These variants cover additional behavior:

- `region_handshake_info4`, `region_handshake_strings`, `region_handshake_invalid_utf8`, and `region_handshake_legacy` cover variable blocks, typed strings, malformed text, and the older layout.
- `logout_reply_empty`, `logout_reply_zero_count`, and `logout_reply_nil_item` distinguish an omitted final block count, an explicit zero count, and the viewer's nil item sentinel.
- `chat_from_viewer_nil_ids` and `chat_from_viewer_zerocoded` cover nil IDs and the codec-only zero-coding override. `chat_from_simulator_*` covers UTF-8, embedded NULs, invalid text, and an inaudible empty message.
- `object_update_compressed_typed` covers optional motion fields and opaque trailing bytes. `improved_terse_object_update_avatar` includes its collision plane. `object_update_compressed` has a truncated packed identity, so typed decoding fails while schema metadata remains available.
- `transfer_request_large_zerocoded` expands beyond the viewer's 8 KiB decode buffer. `zerocode_packet_at_limit` decodes to exactly 8 KiB. `zerocode_extended_run` covers the viewer's extended zero-run form.
- `uuid_name_request_empty` omits the final variable-block count. The [Packet Layout compatibility rule](https://wiki.secondlife.com/wiki/Packet_Layout#Extensions_for_backwards_compatibility) treats it as zero repeats.
