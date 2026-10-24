use std::{net::SocketAddr, str};

use llsd::{Value, xml};
use reqwest::Url;
use tokio::sync::{OwnedSemaphorePermit, mpsc, watch};

use crate::{
    capabilities::{CapabilityBudget, Client, EVENT_QUEUE_NAME, EventQueueTask},
    coverage::{CoverageLossLedger, CoverageWatermark},
    event::{Event, EventEnvelope, EventRetention, SimulatorId},
    fixtures::http::{EventQueueTestServer, EventServerResponse, request_line, seed_grant},
};

pub(crate) struct EventReceiver(mpsc::Receiver<EventEnvelope>);

impl EventReceiver {
    pub(crate) fn new(receiver: mpsc::Receiver<EventEnvelope>) -> Self {
        Self(receiver)
    }

    pub(crate) async fn recv(&mut self) -> Option<Event> {
        self.0.recv().await.map(EventEnvelope::into_event)
    }

    pub(crate) fn try_recv(&mut self) -> Result<Event, mpsc::error::TryRecvError> {
        self.0.try_recv().map(EventEnvelope::into_event)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub(crate) struct EventQueueHarness {
    pub(crate) server: EventQueueTestServer,
    pub(crate) client: Client,
    pub(crate) event_sender: mpsc::Sender<EventEnvelope>,
    pub(crate) received_events: EventReceiver,
    pub(crate) shutdown: watch::Sender<bool>,
    pub(crate) coverage_losses: CoverageLossLedger,
}

pub(crate) struct CapabilityHarness {
    pub(crate) server: EventQueueTestServer,
    pub(crate) client: Client,
}

impl CapabilityHarness {
    pub(crate) fn new(responses: impl FnOnce(&Url) -> Vec<EventServerResponse>) -> Self {
        let server = EventQueueTestServer::start(responses);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        Self { server, client }
    }
}

pub(crate) struct EventQueueBudgetHarness {
    pub(crate) budget: CapabilityBudget,
    pub(crate) events: mpsc::Sender<EventEnvelope>,
    pub(crate) received_events: mpsc::Receiver<EventEnvelope>,
    pub(crate) shutdown: watch::Sender<bool>,
    pub(crate) shutdown_requested: watch::Receiver<bool>,
}

impl EventQueueBudgetHarness {
    pub(crate) fn new(event_capacity: usize) -> Self {
        let (events, received_events) = mpsc::channel(event_capacity);
        let (shutdown, shutdown_requested) = watch::channel(false);
        Self { budget: CapabilityBudget::new(), events, received_events, shutdown, shutdown_requested }
    }

    pub(crate) fn client(&self, seed: Url) -> Client {
        Client::new_with_budget(seed, self.budget.clone()).expect("active Tokio runtime")
    }

    pub(crate) fn start(
        &self,
        client: &Client,
        simulator_id: SimulatorId,
        source: SocketAddr,
        ledger: CoverageLossLedger,
    ) -> EventQueueTask {
        client
            .start_event_queue(simulator_id, source, self.events.clone(), self.shutdown_requested.clone(), ledger)
            .expect("start event queue")
    }

    pub(crate) fn start_with_role(
        &self,
        client: &Client,
        simulator_id: SimulatorId,
        source: SocketAddr,
        ledger: CoverageLossLedger,
        is_primary: bool,
    ) -> EventQueueTask {
        if is_primary {
            client
                .start_primary_event_queue(simulator_id, source, self.events.clone(), self.shutdown_requested.clone(), ledger)
                .expect("start primary event queue")
        } else {
            self.start(client, simulator_id, source, ledger)
        }
    }
}

impl EventQueueHarness {
    pub(crate) fn new(responses: impl FnOnce(&Url) -> Vec<EventServerResponse>, event_capacity: usize) -> Self {
        let server = EventQueueTestServer::start(responses);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(event_capacity);
        let (shutdown, _) = watch::channel(false);
        Self {
            server,
            client,
            event_sender,
            received_events: EventReceiver::new(received_events),
            shutdown,
            coverage_losses: CoverageLossLedger::new(),
        }
    }

    pub(crate) fn seeded(path: &str, event_capacity: usize, follow_up: impl IntoIterator<Item = EventServerResponse>) -> Self {
        Self::new(
            |base| {
                let mut event_url = base.clone();
                event_url.set_path(path);
                std::iter::once(EventServerResponse::Response(seed_grant(&event_url))).chain(follow_up).collect()
            },
            event_capacity,
        )
    }

    pub(crate) fn expect_request(&self, path: &str) -> Vec<u8> {
        let request = self.server.next_request();
        assert!(request_line(&request).starts_with(&format!("POST {path} HTTP/1.1")));
        request
    }

    pub(crate) fn start_queue(&self) -> EventQueueTask {
        self.start_queue_for(crate::fixtures::simulator::test_simulator_id(1), crate::fixtures::simulator::test_source())
    }

    pub(crate) fn start_queue_for(&self, simulator_id: SimulatorId, source: SocketAddr) -> EventQueueTask {
        self.client
            .start_event_queue(simulator_id, source, self.event_sender.clone(), self.shutdown.subscribe(), self.coverage_losses.clone())
            .expect("start event queue")
    }
}

pub(crate) fn event_with_retained_bytes(event: Event, permit: OwnedSemaphorePermit) -> EventEnvelope {
    EventEnvelope::from_parts(event, Some(EventRetention::from_permits(vec![permit])), None)
}

pub(crate) fn event_with_retention(event: Event, retention: EventRetention) -> EventEnvelope {
    EventEnvelope::from_parts(event, Some(retention), None)
}

pub(crate) fn event_with_coverage_watermark(event: Event, coverage_watermark: CoverageWatermark) -> EventEnvelope {
    EventEnvelope::from_parts(event, None, Some(coverage_watermark))
}

pub(crate) fn request_body(request: &[u8]) -> &str {
    str::from_utf8(request).expect("request should be UTF-8").split_once("\r\n\r\n").expect("request should include body").1
}

pub(crate) fn decode_request_body(request: &[u8]) -> Value {
    xml::from_str(request_body(request)).expect("request body should contain LLSD XML")
}

pub(crate) fn map_field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_map()?.get(key)
}

pub(crate) async fn receive_poll_error(events: &mut EventReceiver) -> String {
    let loss = events.recv().await.expect("coverage loss should be reported before the poll error");
    std::assert_matches!(loss, Event::EventQueueCoverageLost { .. });

    let event = events.recv().await.expect("poll error should be reported after coverage loss");
    let Event::CapabilityError { capability, reason, .. } = event else {
        panic!("expected an EventQueueGet capability error");
    };
    assert_eq!(capability, EVENT_QUEUE_NAME);
    reason
}

pub(crate) fn test_fixture(source: &str) -> Vec<u8> {
    test_support::parse_hex_fixture(source).expect("valid LLUDP fixture")
}

pub(crate) fn set_sequence(bytes: &mut [u8], sequence: u32) {
    bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
}
