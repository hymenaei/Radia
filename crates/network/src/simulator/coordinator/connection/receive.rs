use std::{io, time::Duration};

use protocol::{
    LLUdpError,
    lludp::{Message, Packet},
    messages::{
        CompleteAgentMovement, CompletePingCheck, PingId, RegionHandshakeReply, agent::complete_agent_movement,
        region::region_handshake_reply,
    },
    schema::ids::PACKET_ACK,
};
use tokio::{sync::mpsc, time::Instant};

use super::{
    ConnectionRole, ConnectionTask, EVENT_BACKLOG_DISCONNECT_REASON, MAX_PRECONNECT_EVENTS, MAX_QUEUED_SIMULATOR_EVENTS,
    NEIGHBOR_PRECONNECT_OVERFLOW_REASON, SimulatorRequest, active_queue_class, lifecycle::RunControl, outbound::MovementSend,
    preempt_deferred_send, preflight_loss_reason, queue_internal,
};
use crate::simulator::coordinator::event_routing::NeighborEventQueueOutcome;
use crate::{
    coverage::{CoverageLossReason, CoverageSource},
    event::{Event, EventEnvelope},
    transport::{
        lludp::{Delivery, ReceiveAdmission},
        pacing::TrafficClass,
    },
};

const PACKET_REJECTION_SUMMARY_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
enum RejectionKind {
    FrameInspection,
    AcknowledgementAdmission,
    DecodeAdmission,
    Preflight,
    RetainedMemoryAdmission,
    Decode,
    AllocationEstimateMismatch,
    EventAdmission,
}

#[derive(Default)]
pub(super) struct PacketRejections {
    counts: [u64; 8],
    last_reported_at: Option<Instant>,
}

impl PacketRejections {
    pub(super) fn flush(&mut self) {
        if self.counts.iter().any(|count| *count > 0) {
            self.report(Instant::now());
        }
    }

    fn record(&mut self, kind: RejectionKind) {
        let index = match kind {
            RejectionKind::FrameInspection => 0,
            RejectionKind::AcknowledgementAdmission => 1,
            RejectionKind::DecodeAdmission => 2,
            RejectionKind::Preflight => 3,
            RejectionKind::RetainedMemoryAdmission => 4,
            RejectionKind::Decode => 5,
            RejectionKind::AllocationEstimateMismatch => 6,
            RejectionKind::EventAdmission => 7,
        };
        self.counts[index] = self.counts[index].saturating_add(1);
        let now = Instant::now();
        if self.last_reported_at.is_none_or(|last| now.saturating_duration_since(last) >= PACKET_REJECTION_SUMMARY_INTERVAL) {
            self.report(now);
        }
    }

    fn report(&mut self, now: Instant) {
        let [
            frame_inspection,
            acknowledgement_admission,
            decode_admission,
            preflight,
            retained_memory_admission,
            decode,
            allocation_estimate_mismatch,
            event_admission,
        ] = std::mem::take(&mut self.counts);
        self.last_reported_at = Some(now);
        if allocation_estimate_mismatch > 0 {
            tracing::error!(
                frame_inspection,
                acknowledgement_admission,
                decode_admission,
                preflight,
                retained_memory_admission,
                decode,
                allocation_estimate_mismatch,
                event_admission,
                "simulator packets rejected; allocation estimate mismatch"
            );
        } else {
            tracing::warn!(
                frame_inspection,
                acknowledgement_admission,
                decode_admission,
                preflight,
                retained_memory_admission,
                decode,
                allocation_estimate_mismatch,
                event_admission,
                "simulator packets rejected"
            );
        }
    }
}

impl ConnectionTask {
    pub(super) fn receive_datagram(&mut self, received: io::Result<usize>, buffer: &[u8]) -> Result<RunControl, String> {
        self.event_delivery_turn = !self.pending_notifications.is_empty() || !self.pending_events.is_empty();
        let size = received.map_err(|error| format!("Neighbor simulator connection failed: {error}"))?;
        if *self.shutdown.borrow() || self.events.is_closed() {
            return Ok(RunControl::Finish(None));
        }
        self.receive_packet(&buffer[..size])
    }

    fn receive_packet(&mut self, bytes: &[u8]) -> Result<RunControl, String> {
        let frame = match Packet::inspect(bytes) {
            Ok(frame) => frame,
            Err(_) => {
                self.reject(RejectionKind::FrameInspection, CoverageLossReason::PreflightRejected);
                return Ok(RunControl::Continue);
            }
        };
        let admission =
            match self.connection.circuit.prepare_receive(frame.sequence(), frame.is_reliable(), frame.appended_acks(), Instant::now()) {
                Ok(admission) => admission,
                Err(_) => {
                    self.reject(RejectionKind::AcknowledgementAdmission, CoverageLossReason::AcknowledgementAdmission);
                    return Ok(RunControl::Continue);
                }
            };
        if matches!(admission, ReceiveAdmission::Duplicate(_)) {
            preempt_deferred_send(
                &mut self.pending_send,
                &mut self.outbound,
                &self.connection.circuit,
                self.ack_timer_due || self.phase.is_draining(),
            )?;
            self.last_udp_received = Instant::now();
            return Ok(RunControl::Continue);
        }
        let message_id = match frame.message_id() {
            Ok(id) => id,
            Err(_) => {
                self.reject(RejectionKind::FrameInspection, CoverageLossReason::PreflightRejected);
                return Ok(RunControl::Continue);
            }
        };
        let is_packet_ack = message_id == PACKET_ACK;
        let decode_permit = if is_packet_ack {
            None
        } else {
            let Some(permit) = self.packet_memory.try_acquire_decode_slot() else {
                self.reject(RejectionKind::DecodeAdmission, CoverageLossReason::DecodeAdmission);
                return Ok(RunControl::Continue);
            };
            Some(permit)
        };
        let preflighted = match frame.preflighted() {
            Ok(preflighted) => preflighted,
            Err(error) => {
                self.reject(RejectionKind::Preflight, preflight_loss_reason(error, message_id));
                drop(decode_permit);
                return Ok(RunControl::Continue);
            }
        };
        let preflight = preflighted.preflight();
        let _ack_decode_permit = if is_packet_ack {
            let Some(permit) = self.packet_memory.try_acquire_ack_decode_slot(preflight) else {
                self.reject(RejectionKind::DecodeAdmission, CoverageLossReason::DecodeAdmission);
                drop(decode_permit);
                return Ok(RunControl::Continue);
            };
            Some(permit)
        } else {
            None
        };
        let mut memory_reservation = if is_packet_ack {
            None
        } else {
            match self.packet_memory.reserve_packet(preflight) {
                Ok(reservation) => Some(reservation),
                Err(_) => {
                    self.reject(RejectionKind::RetainedMemoryAdmission, CoverageLossReason::RetainedMemoryAdmission);
                    drop(decode_permit);
                    return Ok(RunControl::Continue);
                }
            }
        };
        let packet = match preflighted.decode() {
            Ok(packet) => packet,
            Err(error) => {
                let reason = if matches!(error, LLUdpError::DecodedPacketTooLarge) {
                    CoverageLossReason::AllocationEstimateMismatch
                } else {
                    CoverageLossReason::DecodeRejected
                };
                self.reject(
                    if matches!(error, LLUdpError::DecodedPacketTooLarge) {
                        RejectionKind::AllocationEstimateMismatch
                    } else {
                        RejectionKind::Decode
                    },
                    reason,
                );
                drop(memory_reservation);
                drop(decode_permit);
                return Ok(RunControl::Continue);
            }
        };
        if let Some(reservation) = &mut memory_reservation
            && self.packet_memory.trim_packet_reservation(reservation, &packet).is_err()
        {
            self.reject(RejectionKind::AllocationEstimateMismatch, CoverageLossReason::AllocationEstimateMismatch);
            drop(memory_reservation);
            drop(decode_permit);
            return Ok(RunControl::Continue);
        }
        let message = packet.message();
        let primary_state = self.connection.current_primary_state();
        self.connection.invalidate_stale_handoff(primary_state.role_generation(), &mut self.outbound, &mut self.pending_send);
        let connection_role = primary_state.connection_role();
        let dispatch_message = super::connection_message_has_application_event(
            message,
            connection_role,
            self.connection.handoff.as_ref(),
            self.connection.movement_requested,
        );
        let event_queue_full = if self.phase.is_established() {
            self.pending_events.len() >= MAX_QUEUED_SIMULATOR_EVENTS
        } else {
            self.preconnect_messages.len() >= MAX_PRECONNECT_EVENTS
        };
        if dispatch_message && (self.phase.is_closing() || self.events.is_closed() || event_queue_full) {
            if event_queue_full && !self.phase.is_closing() {
                let reason =
                    if self.phase.is_established() { EVENT_BACKLOG_DISCONNECT_REASON } else { NEIGHBOR_PRECONNECT_OVERFLOW_REASON };
                self.phase.close(Some(reason), false);
            }
            self.reject(RejectionKind::EventAdmission, CoverageLossReason::EventAdmission);
            self.last_udp_received = Instant::now();
            drop(memory_reservation);
            drop(decode_permit);
            return Ok(RunControl::Continue);
        }
        self.last_udp_received = Instant::now();
        self.connection.circuit.acknowledge_message(packet.message());
        let receive_outcome = self.connection.circuit.commit_receive(admission);
        preempt_deferred_send(
            &mut self.pending_send,
            &mut self.outbound,
            &self.connection.circuit,
            self.ack_timer_due || self.phase.is_draining(),
        )?;
        drop(decode_permit);
        let is_new_packet = receive_outcome.is_new_packet;
        let dispatch_allowed = !self.phase.is_closing();
        if !self.phase.is_established() && !self.connection.circuit.has_pending(self.connection.use_circuit_sequence) {
            self.phase.mark_established();
            tracing::info!(role = ?self.connection.current_primary_state().connection_role(), "simulator connected");
            match self.queue_event(Event::SimulatorConnected {
                simulator_id: self.connection.simulator_id,
                region_handle: self.connection.region_handle,
                endpoint: self.connection.session.simulator_address,
            }) {
                NeighborEventQueueOutcome::ReceiverClosed => return Ok(RunControl::Finish(None)),
                NeighborEventQueueOutcome::Queued | NeighborEventQueueOutcome::Closing => {}
            }
            while let Some(event) = self.preconnect_messages.pop_front() {
                let admission_rejected = self.events.is_closed() || self.pending_events.len() >= MAX_QUEUED_SIMULATOR_EVENTS;
                let watermark = event.coverage_watermark();
                match self.queue_event(event) {
                    NeighborEventQueueOutcome::ReceiverClosed => {
                        if let Some(watermark) = watermark {
                            super::record_coverage_loss(
                                &self.connection.coverage_losses,
                                watermark.simulator_id,
                                watermark.source,
                                CoverageLossReason::EventAdmission,
                            );
                        }
                        for queued in self.pending_events.drain(..) {
                            super::record_event_envelope_loss(&self.connection.coverage_losses, queued, CoverageLossReason::EventAdmission);
                        }
                        return Ok(RunControl::Finish(None));
                    }
                    NeighborEventQueueOutcome::Queued | NeighborEventQueueOutcome::Closing => {
                        if admission_rejected && let Some(watermark) = watermark {
                            super::record_coverage_loss(
                                &self.connection.coverage_losses,
                                watermark.simulator_id,
                                watermark.source,
                                CoverageLossReason::EventAdmission,
                            );
                        }
                    }
                }
                if self.phase.close_reason().is_some() {
                    for queued in self.preconnect_messages.drain(..) {
                        if let Some(watermark) = queued.coverage_watermark() {
                            super::record_coverage_loss(
                                &self.connection.coverage_losses,
                                watermark.simulator_id,
                                watermark.source,
                                CoverageLossReason::EventAdmission,
                            );
                        }
                    }
                    break;
                }
            }
            if self.connection.initial_role == ConnectionRole::Primary {
                queue_internal(
                    &mut self.outbound,
                    Message::CompleteAgentMovement(CompleteAgentMovement {
                        agent_data: complete_agent_movement::AgentData {
                            agent_id: self.connection.session.agent_id,
                            session_id: self.connection.session.session_id,
                            circuit_code: self.connection.session.circuit_code,
                        },
                    }),
                    Delivery::Reliable,
                    TrafficClass::Deadline,
                    MovementSend::Initial,
                    active_queue_class(self.pending_send.as_ref()),
                )?;
            }
            if self.phase.has_region_handshake() && self.connection.handoff.is_some() {
                self.connection.queue_handoff_movement(&mut self.outbound, self.pending_send.as_ref())?;
            }
        }
        if !is_new_packet {
            return Ok(RunControl::Continue);
        }
        let dispatch_message = self.handle_simulator_message(message, connection_role, dispatch_allowed)?;
        preempt_deferred_send(
            &mut self.pending_send,
            &mut self.outbound,
            &self.connection.circuit,
            self.ack_timer_due || self.phase.is_draining(),
        )?;
        if dispatch_message && dispatch_allowed {
            let event = EventEnvelope::with_retention_and_coverage(
                Event::SimulatorMessage {
                    simulator_id: self.connection.simulator_id,
                    source: self.connection.session.simulator_address,
                    sequence: packet.sequence(),
                    packet_order: receive_outcome.packet_order,
                    message: packet.into_message(),
                },
                memory_reservation.take().expect("application packet was admitted").into_retention(),
                self.connection.coverage_losses.watermark(self.connection.simulator_id, CoverageSource::Lludp),
            );
            if self.phase.is_established() {
                let admission_rejected = self.events.is_closed() || self.pending_events.len() >= MAX_QUEUED_SIMULATOR_EVENTS;
                let watermark = event.coverage_watermark();
                match self.queue_event(event) {
                    NeighborEventQueueOutcome::ReceiverClosed => {
                        if let Some(watermark) = watermark {
                            super::record_coverage_loss(
                                &self.connection.coverage_losses,
                                watermark.simulator_id,
                                watermark.source,
                                CoverageLossReason::EventAdmission,
                            );
                        }
                        for queued in self.pending_events.drain(..) {
                            super::record_event_envelope_loss(&self.connection.coverage_losses, queued, CoverageLossReason::EventAdmission);
                        }
                        return Ok(RunControl::Finish(None));
                    }
                    NeighborEventQueueOutcome::Queued | NeighborEventQueueOutcome::Closing => {
                        if admission_rejected && let Some(watermark) = watermark {
                            super::record_coverage_loss(
                                &self.connection.coverage_losses,
                                watermark.simulator_id,
                                watermark.source,
                                CoverageLossReason::EventAdmission,
                            );
                        }
                    }
                }
            } else if self.preconnect_messages.len() < MAX_PRECONNECT_EVENTS {
                self.preconnect_messages.push_back(event);
            } else {
                self.phase.close(Some(NEIGHBOR_PRECONNECT_OVERFLOW_REASON), false);
                self.packet_rejections.record(RejectionKind::EventAdmission);
                if let Some(watermark) = event.coverage_watermark() {
                    super::record_coverage_loss(
                        &self.connection.coverage_losses,
                        watermark.simulator_id,
                        watermark.source,
                        CoverageLossReason::EventAdmission,
                    );
                }
                for queued in self.preconnect_messages.drain(..) {
                    if let Some(watermark) = queued.coverage_watermark() {
                        super::record_coverage_loss(
                            &self.connection.coverage_losses,
                            watermark.simulator_id,
                            watermark.source,
                            CoverageLossReason::EventAdmission,
                        );
                    }
                }
            }
        } else if dispatch_message && is_new_packet {
            self.reject(RejectionKind::EventAdmission, CoverageLossReason::EventAdmission);
        }
        Ok(RunControl::Continue)
    }

    fn handle_simulator_message(
        &mut self,
        message: &Message,
        connection_role: ConnectionRole,
        dispatch_allowed: bool,
    ) -> Result<bool, String> {
        let mut dispatch_message = true;
        match message {
            Message::EnableSimulator(enabled) => {
                let info = &enabled.simulator_info;
                let endpoint = std::net::SocketAddr::from((info.ip, info.port));
                if let Err(error) = self.neighbor_requests.try_send(SimulatorRequest::Enable {
                    simulator_id: self.connection.simulator_id,
                    region_handle: info.handle,
                    endpoint,
                }) {
                    let reason = match error {
                        mpsc::error::TrySendError::Full(_) => "The neighbor request queue is full.",
                        mpsc::error::TrySendError::Closed(_) => "The simulator manager has closed.",
                    };
                    return Err(reason.into());
                }
            }
            Message::DisableSimulator(_) => {
                self.phase.close(None, false);
                dispatch_message = false;
            }
            Message::RegionHandshake(_) => {
                self.phase.mark_region_handshake();
                queue_internal(
                    &mut self.outbound,
                    Message::RegionHandshakeReply(RegionHandshakeReply {
                        agent_data: region_handshake_reply::AgentData {
                            agent_id: self.connection.session.agent_id,
                            session_id: self.connection.session.session_id,
                        },
                        region_info: region_handshake_reply::RegionInfo { flags: 0 },
                    }),
                    Delivery::Reliable,
                    TrafficClass::Control,
                    MovementSend::None,
                    active_queue_class(self.pending_send.as_ref()),
                )?;
                if self.phase.is_established() && self.connection.handoff.is_some() {
                    self.connection.queue_handoff_movement(&mut self.outbound, self.pending_send.as_ref())?;
                }
            }
            Message::AgentMovementComplete(movement) => {
                let matches_current_movement = match connection_role {
                    ConnectionRole::Primary => {
                        movement.agent_data.agent_id == self.connection.session.agent_id
                            && movement.agent_data.session_id == self.connection.session.session_id
                    }
                    ConnectionRole::Neighbor => self.connection.handoff.as_ref().is_some_and(|handoff| {
                        self.connection.movement_requested
                            && movement.agent_data.agent_id == handoff.agent_id
                            && movement.agent_data.session_id == handoff.session_id
                            && movement.data.region_handle == handoff.region_handle
                    }),
                };
                if !matches_current_movement {
                    dispatch_message = false;
                } else {
                    if connection_role == ConnectionRole::Primary {
                        self.connection.region_handle = Some(movement.data.region_handle);
                    }
                    if dispatch_allowed && !self.connection.agent_throttle_sent {
                        queue_internal(
                            &mut self.outbound,
                            super::agent_throttle_message(&self.connection.session),
                            Delivery::Reliable,
                            TrafficClass::Control,
                            MovementSend::None,
                            active_queue_class(self.pending_send.as_ref()),
                        )?;
                        self.connection.agent_throttle_sent = true;
                    }
                }
            }
            Message::StartPingCheck(ping) => {
                dispatch_message = false;
                self.connection.circuit.clear_duplicate_list(ping.oldest_unacked, Instant::now());
                queue_internal(
                    &mut self.outbound,
                    Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: ping.ping_id.ping_id } }),
                    Delivery::Unreliable,
                    TrafficClass::Control,
                    MovementSend::None,
                    active_queue_class(self.pending_send.as_ref()),
                )?;
            }
            Message::CompletePingCheck(ping) => {
                dispatch_message = false;
                self.connection.circuit.record_ping_response(ping.ping_id.ping_id, Instant::now());
            }
            Message::LogoutReply(reply)
                if self.phase.is_closing()
                    && reply.agent_data.agent_id == self.connection.session.agent_id
                    && reply.agent_data.session_id == self.connection.session.session_id =>
            {
                dispatch_message = false;
                self.phase.begin_draining();
            }
            Message::PacketAck(_) => dispatch_message = false,
            _ => {}
        }
        Ok(dispatch_message)
    }

    fn queue_event(&mut self, event: impl Into<EventEnvelope>) -> NeighborEventQueueOutcome {
        let outcome = super::queue_neighbor_event(&self.events, &mut self.pending_events, event);
        if matches!(outcome, NeighborEventQueueOutcome::Closing) {
            self.phase.close(Some(EVENT_BACKLOG_DISCONNECT_REASON), true);
        }
        outcome
    }

    fn reject(&mut self, kind: RejectionKind, reason: CoverageLossReason) {
        self.packet_rejections.record(kind);
        super::record_coverage_loss(&self.connection.coverage_losses, self.connection.simulator_id, CoverageSource::Lludp, reason);
    }
}
