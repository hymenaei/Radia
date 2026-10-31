use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use network::{AgentSession, SessionHandle, SimulatorId};
use protocol::lludp::{FieldValue, Message};
use tokio::runtime::Handle;

use crate::session::{
    AgentControlEvent,
    agent_control::{self, Command as AgentControlCommand},
    inbound::decode_sit_animation,
};

use super::SessionDriver;

#[derive(Default)]
pub(super) struct PendingHandshakeReplies {
    pub(super) by_simulator: HashMap<SimulatorId, PendingHandshakeReply>,
}

pub(super) struct PendingHandshakeReply {
    pub(super) flags: u32,
    pub(super) remaining: usize,
}

impl PendingHandshakeReplies {
    pub(super) fn queue(&mut self, simulator_id: SimulatorId, flags: u32) -> bool {
        match self.by_simulator.get_mut(&simulator_id) {
            Some(reply) => {
                reply.flags = flags;
                reply.remaining = reply.remaining.saturating_add(1);
                false
            }
            None => {
                self.by_simulator.insert(simulator_id, PendingHandshakeReply { flags, remaining: 1 });
                true
            }
        }
    }

    pub(super) fn take_next(&mut self, simulator_id: SimulatorId) -> Option<u32> {
        if self.by_simulator.get(&simulator_id).is_some_and(|reply| reply.remaining == 0) {
            self.by_simulator.remove(&simulator_id);
            return None;
        }
        let reply = self.by_simulator.get_mut(&simulator_id)?;
        reply.remaining -= 1;
        Some(reply.flags)
    }
}

impl SessionDriver {
    pub(super) fn handle_agent_control_event(&mut self, event: AgentControlEvent) {
        match event {
            AgentControlEvent::Command(Some(command)) => self.handle_agent_control_command(command),
            AgentControlEvent::Command(None) => {}
            AgentControlEvent::SitTimedOut => self.agent_control.timeout_sit(),
        }
    }

    fn handle_agent_control_command(&mut self, command: AgentControlCommand) {
        agent_control::process_agent_control_command(
            &self.session_handle,
            &mut self.agent_control,
            &mut self.camera,
            self.primary_simulator,
            self.active_session.as_ref().map(|session| (session.agent_id, session.session_id)),
            command,
        );
    }

    pub(super) fn handle_sit_response(&mut self, simulator: SimulatorId, message: &Message) {
        let world = self.session_world.world();
        agent_control::process_sit_response(&mut self.agent_control, &mut self.camera, world, simulator, message);
        if let Some(agent_id) = self.active_session.as_ref().map(|session| session.agent_id) {
            self.agent_control.observe_world(world, simulator, agent_id, self.agent_motion.ground_sitting());
        }
    }

    pub(super) fn update_control_grabs(&mut self, simulator: SimulatorId, message: &Message) {
        if self.primary_simulator != Some(simulator) {
            return;
        }
        if let Some(changes) = decode_script_control_changes(message) {
            let changed = changes
                .into_iter()
                .fold(false, |changed, (take, controls, pass_to_agent)| self.control_grabs.apply(take, controls, pass_to_agent) || changed);
            if changed {
                self.control_grabs_sender.send_replace(self.control_grabs);
            }
        } else if message.name() == "ForceScriptControlRelease"
            && self.active_session.as_ref().is_some_and(|session| script_control_release_matches(message, session))
            && self.control_grabs.clear()
        {
            self.control_grabs_sender.send_replace(self.control_grabs);
        }
    }

    pub(super) fn handle_agent_sit_animation(&mut self, simulator: SimulatorId, received_at: std::time::Instant, message: &Message) {
        let Some(update) = decode_sit_animation(message) else { return };
        if self.primary_simulator != Some(simulator)
            || !self.active_session.as_ref().is_some_and(|session| session.agent_id == update.agent_id)
            || !self.agent_motion.set_ground_sitting(update.ground_sitting)
        {
            return;
        }
        tracing::debug!(
            simulator = simulator.get(),
            agent = %update.agent_id,
            ground_sitting = update.ground_sitting,
            "agent sit animation changed"
        );
        self.publish_world_agent_motion(received_at);
    }
}

pub(super) fn decode_script_control_changes(message: &Message) -> Option<Vec<(bool, u32, bool)>> {
    let Message::Schema(message) = message else { return None };
    let schema = message.schema();
    if schema.name != "ScriptControlChange" || schema.blocks.first()?.name != "Data" {
        return None;
    }
    message
        .blocks()
        .first()?
        .instances
        .iter()
        .map(|instance| {
            let [FieldValue::Bool(take), FieldValue::U32(controls), FieldValue::Bool(pass_to_agent)] = instance.fields.as_slice() else {
                return None;
            };
            Some((*take, *controls, *pass_to_agent))
        })
        .collect()
}

pub(super) fn script_control_release_matches(message: &Message, session: &AgentSession) -> bool {
    let Message::Schema(message) = message else { return false };
    let schema = message.schema();
    if schema.name != "ForceScriptControlRelease" || schema.blocks.first().is_none_or(|block| block.name != "AgentData") {
        return false;
    }
    let Some(instance) = message.blocks().first().and_then(|block| block.instances.first()) else { return false };
    matches!(instance.fields.as_slice(), [FieldValue::Uuid(agent_id), FieldValue::Uuid(session_id)] if *agent_id == session.agent_id && *session_id == session.session_id)
}

pub(super) fn reply_region_handshake(
    runtime: &Handle,
    pending_replies: Arc<Mutex<PendingHandshakeReplies>>,
    session_handle: SessionHandle,
    simulator_id: SimulatorId,
    flags: u32,
) {
    let mut pending = pending_replies.lock().unwrap_or_else(|error| error.into_inner());
    if !pending.by_simulator.contains_key(&simulator_id) {
        match session_handle.try_reply_region_handshake(simulator_id, flags) {
            Ok(response) => {
                drop(pending);
                drop(runtime.spawn(async move {
                    match response.await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            tracing::warn!(simulator = simulator_id.get(), %error, "region handshake reply could not be sent");
                        }
                        Err(_) => {
                            tracing::warn!(simulator = simulator_id.get(), "region handshake reply ended with the network session");
                        }
                    }
                }));
                return;
            }
            Err(network::Error::CommandQueueFull) => {}
            Err(error) => {
                tracing::warn!(simulator = simulator_id.get(), %error, "region handshake reply could not be sent");
                return;
            }
        }
    }
    let should_drain = pending.queue(simulator_id, flags);
    drop(pending);
    if !should_drain {
        return;
    }
    drop(runtime.spawn(async move {
        loop {
            let next_flags = pending_replies.lock().unwrap_or_else(|error| error.into_inner()).take_next(simulator_id);
            let Some(next_flags) = next_flags else { break };
            if let Err(error) = session_handle.reply_region_handshake(simulator_id, next_flags).await {
                tracing::warn!(simulator = simulator_id.get(), %error, "region handshake reply could not be sent");
                pending_replies.lock().unwrap_or_else(|error| error.into_inner()).by_simulator.remove(&simulator_id);
                break;
            }
        }
    }));
}
