mod capabilities;
mod capture_format;
mod circuit;
mod http;
mod login;
mod redaction;

use protocol::{
    messages::{CompletePingCheck, StartPingCheck, complete_ping_check::PingID as CompletePingID, start_ping_check::PingID as StartPingID},
    udp::Message,
};

fn complete_ping(ping_id: u8) -> Message {
    Message::CompletePingCheck(CompletePingCheck { ping_id: CompletePingID { ping_id } })
}

fn start_ping(ping_id: u8, oldest_unacked: u32) -> Message {
    Message::StartPingCheck(StartPingCheck { ping_id: StartPingID { ping_id, oldest_unacked } })
}
