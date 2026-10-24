use std::net::{Ipv4Addr, SocketAddr};

use llsd::Value;
use tokio::task::JoinError;
use url::Url;
use uuid::Uuid;

use super::{
    NeighborEventQueueJoin, RuntimeAction, SessionDispatcher, SessionError,
    handoff::{HandoffOrder, PromotionSetup},
    is_current_simulator,
    neighbors::neighbor_event_queue_failure,
};
use crate::{
    capabilities::EventQueueExit,
    event::{Event, EventEnvelope},
};

const MAX_SIMULATORS_PER_ENABLE_EVENT: usize = 32;

impl SessionDispatcher<'_> {
    pub(super) async fn handle_capability_event(&mut self, capability_event: Option<EventEnvelope>) -> Result<RuntimeAction, SessionError> {
        let Some(envelope) = capability_event else {
            self.capability_events_closed = true;
            return Ok(RuntimeAction::Continue);
        };
        let (event, retained_bytes, coverage_watermark) = envelope.into_parts();
        match event {
            Event::CapabilityEvent { simulator_id, source, message, mut body } => {
                if !is_current_simulator(simulator_id, self.state.simulator_id, &self.run_state.neighbor_connections) {
                    return Ok(RuntimeAction::Continue);
                }
                let mut setup = PromotionSetup {
                    state: &mut self.run_state,
                    neighbor_requests: self.neighbor_requests.clone(),
                    application_events: self.events.clone(),
                    neighbor_events: self.neighbor_events.clone(),
                    shutdown: self.disconnect_requested.clone(),
                    capability_events: self.capability_events.clone(),
                };
                self.state.process_capability_event(simulator_id, &message, &body, None, &mut setup, &mut self.handoff_state).await?;
                redact_seed_capability(&mut body);
                self.state
                    .publish_simulator_event(
                        simulator_id,
                        EventEnvelope::from_parts(
                            Event::CapabilityEvent { simulator_id, source, message, body },
                            retained_bytes,
                            coverage_watermark,
                        ),
                        &self.events,
                        &mut self.run_state,
                    )
                    .await?;
            }
            Event::CapabilityEventBatch { simulator_id, source, order, events: mut batch } => {
                if !is_current_simulator(simulator_id, self.state.simulator_id, &self.run_state.neighbor_connections) {
                    return Ok(RuntimeAction::Continue);
                }
                let mut setup = PromotionSetup {
                    state: &mut self.run_state,
                    neighbor_requests: self.neighbor_requests.clone(),
                    application_events: self.events.clone(),
                    neighbor_events: self.neighbor_events.clone(),
                    shutdown: self.disconnect_requested.clone(),
                    capability_events: self.capability_events.clone(),
                };
                for (index, event) in batch.iter().enumerate() {
                    self.state
                        .process_capability_event(
                            simulator_id,
                            &event.message,
                            &event.body,
                            Some(HandoffOrder::EventQueue { incarnation: order.incarnation, batch: order.ordinal, index }),
                            &mut setup,
                            &mut self.handoff_state,
                        )
                        .await?;
                }
                for event in &mut batch {
                    redact_seed_capability(&mut event.body);
                }
                self.state
                    .publish_simulator_event(
                        simulator_id,
                        EventEnvelope::from_parts(
                            Event::CapabilityEventBatch { simulator_id, source, order, events: batch },
                            retained_bytes,
                            coverage_watermark,
                        ),
                        &self.events,
                        &mut self.run_state,
                    )
                    .await?;
            }
            Event::CapabilityError { simulator_id, source, capability, reason }
                if is_current_simulator(simulator_id, self.state.simulator_id, &self.run_state.neighbor_connections) =>
            {
                self.state
                    .publish_simulator_event(
                        simulator_id,
                        EventEnvelope::from_parts(
                            Event::CapabilityError { simulator_id, source, capability, reason },
                            retained_bytes,
                            coverage_watermark,
                        ),
                        &self.events,
                        &mut self.run_state,
                    )
                    .await?;
            }
            Event::CapabilityError { .. } => {}
            event => {
                self.state
                    .publish_event(&self.events, &mut self.run_state, EventEnvelope::from_parts(event, retained_bytes, coverage_watermark))
                    .await?;
            }
        }
        Ok(RuntimeAction::Continue)
    }

    pub(super) async fn handle_neighbor_event_queue_join(&mut self, joined: NeighborEventQueueJoin) -> Result<RuntimeAction, SessionError> {
        match joined {
            Some(Ok((task_id, (simulator_id, source, exit)))) => {
                self.run_state.neighbor_event_queue_owners.remove(&task_id);
                let current_queue = self.run_state.simulator_capabilities.get_mut(&simulator_id).is_some_and(|capability| {
                    if capability.endpoint == source && capability.event_queue_task == Some(task_id) {
                        capability.event_queue_running = false;
                        true
                    } else {
                        false
                    }
                });
                if !current_queue {
                    return Ok(RuntimeAction::Continue);
                }
                if simulator_id == self.state.primary.simulator_id
                    && exit != EventQueueExit::Stopped
                    && !*self.disconnect_requested.borrow()
                    && self.run_state.logout_deadline.is_none()
                {
                    self.state.note_primary_event_queue_exit(exit);
                }
                if simulator_id != self.state.primary.simulator_id
                    && let Some(event) = neighbor_event_queue_failure(
                        simulator_id,
                        source,
                        exit,
                        self.state.simulator_id,
                        &self.run_state.neighbor_connections,
                    )
                {
                    self.state.publish_event(&self.events, &mut self.run_state, event).await?;
                }
            }
            Some(Err(error)) => {
                let task_id = error.id();
                let current_queue =
                    self.run_state.neighbor_event_queue_owners.get(&task_id).copied().is_some_and(|(simulator_id, source)| {
                        self.run_state.simulator_capabilities.get_mut(&simulator_id).is_some_and(|capability| {
                            if capability.endpoint == source && capability.event_queue_task == Some(task_id) {
                                capability.event_queue_running = false;
                                true
                            } else {
                                false
                            }
                        })
                    });
                if !current_queue {
                    self.run_state.neighbor_event_queue_owners.remove(&task_id);
                    return Ok(RuntimeAction::Continue);
                }
                let Some((simulator_id, source)) = self.run_state.neighbor_event_queue_owners.remove(&task_id) else {
                    return Ok(RuntimeAction::Continue);
                };
                let active_primary_queue = simulator_id == self.state.primary.simulator_id;
                if active_primary_queue && !*self.disconnect_requested.borrow() && self.run_state.logout_deadline.is_none() {
                    self.state.note_primary_event_queue_exit(EventQueueExit::TaskFailed);
                }
                if !active_primary_queue
                    && let Some(event) = neighbor_event_queue_failure(
                        simulator_id,
                        source,
                        EventQueueExit::TaskFailed,
                        self.state.simulator_id,
                        &self.run_state.neighbor_connections,
                    )
                {
                    self.state.publish_event(&self.events, &mut self.run_state, event).await?;
                }
            }
            None => {}
        }
        Ok(RuntimeAction::Continue)
    }

    pub(super) fn handle_primary_event_queue_completion(&mut self, exit: EventQueueExit) -> RuntimeAction {
        self.event_queue_finished = true;
        let initial_queue_current = self.run_state.simulator_capabilities.get_mut(&self.state.simulator_id).is_some_and(|capability| {
            if capability.endpoint == self.state.session.simulator_address
                && capability.seed_capability == self.state.session.seed_capability
                && capability.event_queue_task.is_none()
            {
                capability.event_queue_running = false;
                true
            } else {
                false
            }
        });
        if self.state.primary.simulator_id == self.state.simulator_id
            && initial_queue_current
            && !*self.disconnect_requested.borrow()
            && self.run_state.logout_deadline.is_none()
        {
            self.state.note_primary_event_queue_exit(exit);
        } else if initial_queue_current
            && matches!(exit, EventQueueExit::TaskFailed | EventQueueExit::RetryLimitReached | EventQueueExit::Unavailable)
        {
            tracing::warn!(simulator = self.state.simulator_id.get(), "former primary simulator EventQueueGet ended after promotion");
        }
        RuntimeAction::Continue
    }

    pub(super) fn handle_capability_request_join(&mut self, joined: Option<Result<(), JoinError>>) -> RuntimeAction {
        if matches!(joined, Some(Err(_))) {
            tracing::error!("Capability request waiter task failed");
        }
        RuntimeAction::Continue
    }
}

pub(super) fn decode_enable_simulators(body: &Value) -> Result<Vec<(u64, SocketAddr)>, &'static str> {
    let body = super::teleport::event_fields(body)?;
    let Some(Value::Array(simulators)) = body.get("SimulatorInfo") else {
        return Err("SimulatorInfo is not an LLSD array");
    };
    if simulators.is_empty() || simulators.len() > MAX_SIMULATORS_PER_ENABLE_EVENT {
        return Err("SimulatorInfo has an invalid number of entries");
    }

    simulators
        .iter()
        .map(|simulator| {
            let Value::Map(simulator) = simulator else {
                return Err("SimulatorInfo entry is not an LLSD map");
            };
            let Some(Value::Binary(handle)) = simulator.get("Handle") else {
                return Err("Handle is not LLSD binary data");
            };
            let handle: [u8; 8] = handle.as_slice().try_into().map_err(|_| "Handle is not 8 bytes")?;
            let Some(Value::Binary(ip)) = simulator.get("IP") else {
                return Err("IP is not LLSD binary data");
            };
            let ip: [u8; 4] = ip.as_slice().try_into().map_err(|_| "IP is not 4 bytes")?;
            let Some(Value::Integer(port)) = simulator.get("Port") else {
                return Err("Port is not an LLSD integer");
            };
            let port = u16::try_from(*port).ok().filter(|port| *port != 0).ok_or("Port is outside the valid range")?;
            let endpoint = SocketAddr::from((Ipv4Addr::from(ip), port));
            Ok((u64::from_be_bytes(handle), endpoint))
        })
        .collect()
}

pub(super) fn decode_agent_communication(body: &Value, agent_id: Uuid) -> Result<(SocketAddr, Url), &'static str> {
    let body = super::teleport::event_fields(body)?;
    match body.get("agent-id") {
        Some(Value::Uuid(value)) if *value == agent_id => {}
        Some(Value::Uuid(_)) => return Err("agent-id does not match the logged-in agent"),
        _ => return Err("agent-id is not an LLSD UUID"),
    }
    let Some(Value::String(address)) = body.get("sim-ip-and-port") else {
        return Err("sim-ip-and-port is not an LLSD string");
    };
    let endpoint = address.parse().map_err(|_| "sim-ip-and-port is not a socket address")?;
    let Some(Value::String(seed)) = body.get("seed-capability") else {
        return Err("seed-capability is not an LLSD string");
    };
    let seed = Url::parse(seed).map_err(|_| "seed-capability is not a valid URL")?;
    Ok((endpoint, seed))
}

pub(super) fn redact_seed_capability(body: &mut Value) {
    fn redact(value: &mut Value) {
        match value {
            Value::Map(fields) => {
                for (name, value) in fields {
                    if matches!(name.as_str(), "SeedCapability" | "seed-capability") {
                        *value = Value::String("[redacted]".to_owned());
                    } else {
                        redact(value);
                    }
                }
            }
            Value::Array(values) => values.iter_mut().for_each(redact),
            _ => {}
        }
    }

    redact(body);
}

#[cfg(test)]
#[path = "tests/capability_events.rs"]
mod tests;
