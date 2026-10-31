use std::time::{Duration, Instant};

use math::{DVec3, Quat, Vec3};
use protocol::messages::AgentMovementComplete;
use tokio::sync::watch;
use uuid::Uuid;
use world::{Parent, Region, World, WorldPose, WorldTransformState};

const MAX_ARRIVAL_XY: f32 = 256.0;
const MAX_ARRIVAL_Z: f32 = 4096.0;
const MAX_MOTION_COMPONENT: f32 = 256.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AgentMotionSample {
    pub(crate) region: Region,
    pub(crate) position: DVec3,
    pub(crate) scale: Vec3,
    pub(crate) velocity: Vec3,
    pub(crate) acceleration: Vec3,
    pub(crate) time_dilation: f32,
    pub(crate) ping_delay: Option<Duration>,
    pub(crate) parented: bool,
    pub(crate) ground_sitting: bool,
    pub(crate) simulator_rotation: Quat,
    pub(crate) received_at: Instant,
    pub(crate) seed: u64,
}

impl AgentMotionSample {
    pub(crate) fn is_finite(self) -> bool {
        self.position.is_finite()
            && self.velocity.is_finite()
            && self.acceleration.is_finite()
            && self.scale.is_finite()
            && self.scale.min_element() > 0.0
            && self.time_dilation.is_finite()
            && (0.0..=1.0).contains(&self.time_dilation)
            && self.simulator_rotation.is_finite()
            && self.simulator_rotation.length_squared().is_finite()
            && self.simulator_rotation.length_squared() > f32::EPSILON
    }

    pub(crate) fn pose(self, position: DVec3) -> WorldPose {
        WorldPose { position, rotation: self.simulator_rotation }
    }
}

pub(crate) struct AgentMotionPublisher {
    sender: watch::Sender<Option<AgentMotionSample>>,
    seed: u64,
    parent: Option<Parent>,
    ground_sitting: bool,
}

impl AgentMotionPublisher {
    pub(crate) fn new() -> Self {
        let (sender, _) = watch::channel(None);
        Self { sender, seed: 0, parent: None, ground_sitting: false }
    }

    pub(crate) fn receiver(&self) -> watch::Receiver<Option<AgentMotionSample>> {
        self.sender.subscribe()
    }

    pub(crate) fn set_ground_sitting(&mut self, ground_sitting: bool) -> bool {
        if self.ground_sitting == ground_sitting {
            return false;
        }
        self.ground_sitting = ground_sitting;
        self.seed = self.seed.wrapping_add(1);
        true
    }

    pub(crate) fn ground_sitting(&self) -> bool {
        self.ground_sitting
    }

    pub(crate) fn publish_arrival(
        &mut self,
        agent_id: Uuid,
        session_id: Uuid,
        movement: &AgentMovementComplete,
        region: Region,
        region_origin: DVec3,
        received_at: Instant,
        ping_delay: Option<Duration>,
    ) -> bool {
        if self.sender.borrow().is_some()
            || movement.agent_data.agent_id != agent_id
            || movement.agent_data.session_id != session_id
            || !region_origin.is_finite()
        {
            return false;
        }
        let local_position = movement.data.position;
        if !(0.0..=MAX_ARRIVAL_XY).contains(&local_position.x)
            || !(0.0..=MAX_ARRIVAL_XY).contains(&local_position.y)
            || !(0.0..=MAX_ARRIVAL_Z).contains(&local_position.z)
        {
            return false;
        }
        let Some(look_at) = Vec3::new(movement.data.look_at.x, movement.data.look_at.y, 0.0).try_normalize() else {
            return false;
        };
        let position = region_origin + local_position.as_dvec3();
        let simulator_rotation = Quat::from_rotation_z(look_at.y.atan2(look_at.x));
        let sample = AgentMotionSample {
            region,
            position,
            scale: Vec3::new(0.6, 0.45, 1.9),
            velocity: Vec3::ZERO,
            acceleration: Vec3::ZERO,
            time_dilation: 1.0,
            ping_delay,
            parented: false,
            ground_sitting: self.ground_sitting,
            simulator_rotation,
            received_at,
            seed: self.seed.wrapping_add(1),
        };
        if !sample.is_finite() {
            return false;
        }
        self.seed = sample.seed;
        self.sender.send_replace(Some(sample));
        true
    }

    pub(crate) fn publish_world(&mut self, world: &World, agent_id: Uuid, received_at: Instant) -> bool {
        let Some(entity) = world.entity(agent_id) else { return false };
        let Some(agent) = world.object(entity) else { return false };
        let parent = agent.parent();
        if self.parent != Some(parent) {
            let previous_parent = self.parent;
            self.seed = self.seed.wrapping_add(1);
            let parent_id = match parent {
                Parent::Resolved(entity) => world.object(entity).map(|object| object.id()),
                Parent::Unknown | Parent::Root | Parent::Pending { .. } => None,
            };
            tracing::debug!(agent = %agent_id, previous = ?previous_parent, parent = ?parent_id, state = ?parent, "agent parent changed");
        }
        self.parent = Some(parent);
        let Ok(transform) = agent.world_transform() else { return false };
        let pose = match transform {
            WorldTransformState::Resolved(transform) => transform.pose,
            WorldTransformState::MissingScale(pose) => pose,
            WorldTransformState::Provisional { .. } | WorldTransformState::Unresolved(_) => return false,
        };
        let motion = agent.linear_motion();
        let source = agent.motion_source();
        let received_at = source.and_then(world::UpdateSource::motion_received_at).unwrap_or(received_at);
        let parented = matches!(parent, Parent::Resolved(_) | Parent::Pending { .. });
        let (velocity, acceleration) = if parented {
            (Vec3::ZERO, Vec3::ZERO)
        } else {
            (motion.velocity.unwrap_or_default(), motion.acceleration.unwrap_or_default())
        };
        let (velocity, acceleration) =
            if velocity.abs().max_element() <= MAX_MOTION_COMPONENT && acceleration.abs().max_element() <= MAX_MOTION_COMPONENT {
                (velocity, acceleration)
            } else {
                (Vec3::ZERO, Vec3::ZERO)
            };
        self.publish(AgentMotionSample {
            region: agent.address().region(),
            position: pose.position,
            scale: agent.scale().unwrap_or(Vec3::new(0.6, 0.45, 1.9)),
            velocity,
            acceleration,
            time_dilation: agent.time_dilation(),
            ping_delay: source.and_then(world::UpdateSource::motion_ping_delay),
            parented,
            ground_sitting: self.ground_sitting,
            simulator_rotation: pose.rotation,
            received_at,
            seed: self.seed,
        })
    }

    fn publish(&self, sample: AgentMotionSample) -> bool {
        if !sample.is_finite() {
            return false;
        }
        self.sender.send_replace(Some(sample));
        true
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU32, time::Duration};

    use math::{DVec3, Quat, Vec3};
    use protocol::{lludp::Message, messages::AgentMovementComplete};
    use uuid::Uuid;
    use world::{
        Kind, LinearMotion, ObjectObservation, OrderEvidence::Ordered, ParentUpdate, Pose, Region, RegionInfo, UpdateSource, World,
    };

    use crate::session::world::viewer_world_limits;

    use super::{AgentMotionPublisher, AgentMotionSample};

    fn arrival() -> AgentMovementComplete {
        let bytes = test_support::parse_hex_fixture(test_support::lludp::vectors::AGENT_MOVEMENT_COMPLETE).expect("movement fixture");
        let packet = protocol::lludp::Packet::decode(&bytes).expect("valid movement packet");
        let Message::AgentMovementComplete(movement) = packet.message() else { panic!("movement-complete fixture") };
        movement.clone()
    }

    #[test]
    fn arrival_flattens_look_and_rejects_out_of_region_position() {
        let mut publisher = AgentMotionPublisher::new();
        let mut movement = arrival();
        movement.data.position = Vec3::new(10.0, 20.0, 30.0);
        movement.data.look_at = Vec3::new(0.0, 1.0, 10.0);
        let now = std::time::Instant::now();
        assert!(publisher.publish_arrival(
            movement.agent_data.agent_id,
            movement.agent_data.session_id,
            &movement,
            Region::new(1),
            DVec3::new(256.0, 512.0, 0.0),
            now,
            None,
        ));
        let sample = publisher.receiver().borrow().expect("arrival sample");
        assert_eq!(sample.position, DVec3::new(266.0, 532.0, 30.0));
        assert_eq!(sample.received_at, now);
        assert_eq!(sample.velocity, Vec3::ZERO);
        assert_eq!(sample.acceleration, Vec3::ZERO);
        assert!((sample.simulator_rotation * Vec3::X - Vec3::Y).length() < 1e-6);

        movement.data.position.x = 1.0e20;
        assert!(!publisher.publish_arrival(
            movement.agent_data.agent_id,
            movement.agent_data.session_id,
            &movement,
            Region::new(1),
            DVec3::ZERO,
            now + Duration::from_secs(1),
            None,
        ));
        assert_eq!(publisher.receiver().borrow().expect("last valid sample").position, DVec3::new(266.0, 532.0, 30.0));
    }

    #[test]
    fn arrival_does_not_replace_existing_motion() {
        let mut publisher = AgentMotionPublisher::new();
        let movement = arrival();
        let (agent_id, session_id) = (movement.agent_data.agent_id, movement.agent_data.session_id);
        let now = std::time::Instant::now();
        assert!(publisher.publish_arrival(agent_id, session_id, &movement, Region::new(1), DVec3::ZERO, now, None));
        let first = publisher.receiver().borrow().expect("arrival sample");
        assert!(!publisher.publish_arrival(
            agent_id,
            session_id,
            &movement,
            Region::new(1),
            DVec3::new(256.0, 0.0, 0.0),
            now + Duration::from_secs(1),
            None
        ));
        assert_eq!(publisher.receiver().borrow().expect("first arrival kept"), first);
    }

    #[test]
    fn seated_agent_publishes_zero_motion() {
        let mut world = World::new(viewer_world_limits());
        let region = Region::new(1);
        let _ = world.adopt_region(region).expect("region");
        let _ = world
            .patch_region(region, RegionInfo { origin: Some(DVec3::new(256.0, 0.0, 0.0)), ..RegionInfo::default() })
            .expect("region origin");
        let agent_id = Uuid::from_u128(1);
        world.set_self_agent(agent_id).expect("self agent");
        let source = UpdateSource::new(region, 1, Ordered(1), 0);
        let seat_id = Uuid::from_u128(2);
        let mut seat =
            ObjectObservation::new(source, seat_id, NonZeroU32::new(1).expect("seat local ID"), Kind::Object, ParentUpdate::Root);
        seat.pose = Some(Pose { position: Some(Vec3::new(10.0, 20.0, 0.0)), rotation: Some(Quat::IDENTITY) });
        let _ = world.observe_object(seat).expect("seat observation");
        let mut agent = ObjectObservation::new(
            source,
            agent_id,
            NonZeroU32::new(2).expect("agent local ID"),
            Kind::Avatar,
            ParentUpdate::Local(NonZeroU32::new(1).expect("seat local ID")),
        );
        agent.pose = Some(Pose { position: Some(Vec3::new(0.0, 0.0, 1.0)), rotation: Some(Quat::IDENTITY) });
        agent.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) });
        let _ = world.observe_object(agent).expect("agent observation");

        let mut publisher = AgentMotionPublisher::new();
        let now = std::time::Instant::now();
        assert!(publisher.publish_world(&world, agent_id, now));
        let sample = publisher.receiver().borrow().expect("world sample");
        assert_eq!(sample.position, DVec3::new(266.0, 20.0, 1.0));
        assert_eq!(sample.velocity, Vec3::ZERO);
        assert_eq!(sample.acceleration, Vec3::ZERO);
        assert_eq!(sample.received_at, now);
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[test]
    fn standing_parent_update_reseeds_body_rotation_from_world_pose() {
        let mut world = World::new(viewer_world_limits());
        let region = Region::new(1);
        let _ = world.adopt_region(region).expect("region");
        let _ = world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).expect("region origin");
        let agent_id = Uuid::from_u128(1);
        world.set_self_agent(agent_id).expect("self agent");
        let seat_rotation = Quat::from_rotation_z(0.7);
        let sitting_rotation = Quat::from_rotation_z(-0.2);
        let source = UpdateSource::new(region, 1, Ordered(1), 0);
        let mut seat = ObjectObservation::new(
            source,
            Uuid::from_u128(2),
            NonZeroU32::new(1).expect("seat local ID"),
            Kind::Object,
            ParentUpdate::Root,
        );
        seat.pose = Some(Pose { position: Some(Vec3::new(10.0, 20.0, 0.0)), rotation: Some(seat_rotation) });
        let _ = world.observe_object(seat).expect("seat observation");
        let mut agent = ObjectObservation::new(
            source,
            agent_id,
            NonZeroU32::new(2).expect("agent local ID"),
            Kind::Avatar,
            ParentUpdate::Local(NonZeroU32::new(1).expect("seat local ID")),
        );
        agent.pose = Some(Pose { position: Some(Vec3::new(0.0, 0.0, 1.0)), rotation: Some(sitting_rotation) });
        let _ = world.observe_object(agent).expect("agent observation");

        let mut publisher = AgentMotionPublisher::new();
        let now = std::time::Instant::now();
        assert!(publisher.publish_world(&world, agent_id, now));
        let seated = publisher.receiver().borrow().expect("seated sample");
        assert!(seated.parented);
        assert!((seated.simulator_rotation * Vec3::X).distance((seat_rotation * sitting_rotation) * Vec3::X) < 1e-6);

        let standing_rotation = Quat::from_rotation_z(1.1);
        let mut agent = ObjectObservation::new(
            UpdateSource::new(region, 1, Ordered(2), 0),
            agent_id,
            NonZeroU32::new(2).expect("agent local ID"),
            Kind::Avatar,
            ParentUpdate::Root,
        );
        agent.pose = Some(Pose { position: Some(Vec3::new(11.0, 20.0, 1.0)), rotation: Some(standing_rotation) });
        let _ = world.observe_object(agent).expect("standing agent update");
        assert!(publisher.publish_world(&world, agent_id, now + Duration::from_millis(20)));
        let standing = publisher.receiver().borrow().expect("standing sample");
        assert!(!standing.parented);
        assert!(standing.seed > seated.seed);
        assert!((standing.simulator_rotation * Vec3::X).distance(standing_rotation * Vec3::X) < 1e-6);

        let mut body_rotation = crate::agent_movement::BodyRotation::default();
        body_rotation.update(Some((seated.seed, seated.simulator_rotation)), 0.0, now);
        body_rotation.update(Some((standing.seed, standing.simulator_rotation)), 0.0, now + Duration::from_millis(20));
        assert!((body_rotation.current() * Vec3::X).distance(standing_rotation * Vec3::X) < 1e-6);
    }

    #[test]
    fn root_agent_publishes_world_motion() {
        let mut world = World::new(viewer_world_limits());
        let region = Region::new(1);
        let _ = world.adopt_region(region).expect("region");
        let _ = world
            .patch_region(region, RegionInfo { origin: Some(DVec3::new(256.0, 0.0, 0.0)), ..RegionInfo::default() })
            .expect("region origin");
        let agent_id = Uuid::from_u128(1);
        world.set_self_agent(agent_id).expect("self agent");
        let rotation = Quat::from_rotation_z(0.4);
        let received_at = std::time::Instant::now();
        let ping_delay = Some(Duration::from_millis(120));
        let mut agent = ObjectObservation::new(
            UpdateSource::new(region, 1, Ordered(1), 0).with_motion_timing(received_at, ping_delay),
            agent_id,
            NonZeroU32::new(1).expect("agent local ID"),
            Kind::Avatar,
            ParentUpdate::Root,
        );
        agent.pose = Some(Pose { position: Some(Vec3::new(10.0, 20.0, 3.0)), rotation: Some(rotation) });
        agent.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::Y) });
        let _ = world.observe_object(agent).expect("agent observation");

        let mut publisher = AgentMotionPublisher::new();
        let now = received_at + Duration::from_millis(100);
        assert!(publisher.publish_world(&world, agent_id, now));
        let sample = publisher.receiver().borrow().expect("world sample");
        assert_eq!(sample.position, DVec3::new(266.0, 20.0, 3.0));
        assert_eq!(sample.velocity, Vec3::X);
        assert_eq!(sample.acceleration, Vec3::Y);
        assert_eq!(sample.simulator_rotation, rotation);
        assert_eq!(sample.received_at, received_at);
        assert_eq!(sample.ping_delay, ping_delay);
    }

    #[test]
    fn motion_beyond_protocol_range_is_discarded() {
        let mut world = World::new(viewer_world_limits());
        let region = Region::new(1);
        let _ = world.adopt_region(region).expect("region");
        let _ = world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).expect("region origin");
        let agent_id = Uuid::from_u128(1);
        world.set_self_agent(agent_id).expect("self agent");
        let mut agent = ObjectObservation::new(
            UpdateSource::new(region, 1, Ordered(1), 0),
            agent_id,
            NonZeroU32::new(1).expect("agent local ID"),
            Kind::Avatar,
            ParentUpdate::Root,
        );
        agent.pose = Some(Pose { position: Some(Vec3::new(10.0, 20.0, 3.0)), rotation: Some(Quat::IDENTITY) });
        agent.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X * 1.0e30), acceleration: Some(Vec3::Y) });
        let _ = world.observe_object(agent).expect("agent observation");

        let mut publisher = AgentMotionPublisher::new();
        assert!(publisher.publish_world(&world, agent_id, std::time::Instant::now()));
        let sample = publisher.receiver().borrow().expect("world sample");
        assert_eq!(sample.position, DVec3::new(10.0, 20.0, 3.0));
        assert_eq!(sample.velocity, Vec3::ZERO);
        assert_eq!(sample.acceleration, Vec3::ZERO);
    }

    #[test]
    fn ground_sit_animation_reseeds_motion() {
        let mut world = World::new(viewer_world_limits());
        let region = Region::new(1);
        let _ = world.adopt_region(region).expect("region");
        let _ = world.patch_region(region, RegionInfo { origin: Some(DVec3::ZERO), ..RegionInfo::default() }).expect("region origin");
        let agent_id = Uuid::from_u128(3);
        let mut agent = ObjectObservation::new(
            UpdateSource::new(region, 1, Ordered(1), 0),
            agent_id,
            NonZeroU32::new(1).expect("agent local ID"),
            Kind::Avatar,
            ParentUpdate::Root,
        );
        agent.pose = Some(Pose { position: Some(Vec3::new(10.0, 20.0, 30.0)), rotation: Some(Quat::IDENTITY) });
        agent.linear_motion = Some(LinearMotion { velocity: Some(Vec3::X), acceleration: Some(Vec3::ZERO) });
        let _ = world.observe_object(agent).expect("agent observation");

        let mut publisher = AgentMotionPublisher::new();
        let now = std::time::Instant::now();
        assert!(publisher.publish_world(&world, agent_id, now));
        let standing = publisher.receiver().borrow().expect("standing motion");
        assert!(publisher.set_ground_sitting(true));
        assert!(publisher.publish_world(&world, agent_id, now + Duration::from_secs(1)));
        let sitting = publisher.receiver().borrow().expect("sitting motion");
        assert!(sitting.ground_sitting);
        assert!(sitting.seed > standing.seed);
        assert!(publisher.set_ground_sitting(false));
        assert!(publisher.publish_world(&world, agent_id, now + Duration::from_secs(2)));
        let standing_again = publisher.receiver().borrow().expect("standing motion after ground sit");
        assert!(!standing_again.ground_sitting);
        assert!(standing_again.seed > sitting.seed);
    }

    #[test]
    fn non_finite_sample_is_rejected() {
        let sample = AgentMotionSample {
            region: Region::new(1),
            position: DVec3::ZERO,
            scale: Vec3::ONE,
            velocity: Vec3::new(f32::INFINITY, 0.0, 0.0),
            acceleration: Vec3::ZERO,
            time_dilation: 1.0,
            ping_delay: None,
            parented: false,
            ground_sitting: false,
            simulator_rotation: Quat::IDENTITY,
            received_at: std::time::Instant::now(),
            seed: 0,
        };
        assert!(!sample.is_finite());
    }
}
