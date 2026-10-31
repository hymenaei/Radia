use std::{
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    time::{Duration, Instant},
};

use egui::{Align2, Area, Button, Context, Frame, Id, Order, Pos2, Ui, Vec2};
use math::{DVec3, Vec3};
use render::{PickHit, PickObject, PickObjectKind, PickTarget, scene::Camera};
use uuid::Uuid;
use winit::dpi::{PhysicalPosition, PhysicalSize};

use crate::{
    scene_feed::SceneFeed,
    session::{AgentControlError, SitOutcome},
};

const MIN_OBJECT_EXTENT: f32 = 0.001;
const OFFSET_PLANE_EPSILON: f64 = 0.0001;
const OFFSET_COMPONENT_EPSILON: f32 = 0.0001;
const OFFSET_BIAS_START: f32 = 0.1;
const OFFSET_BIAS_END: f32 = 0.7;
const NOTICE_DURATION: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToggleAction {
    SitOnGround,
    StandUp,
}

#[derive(Default)]
pub(crate) struct SitToggle {
    pending: bool,
}

impl SitToggle {
    pub(crate) fn label(&self, seated: bool) -> &'static str {
        if seated { "Stand Up" } else { "Sit Down" }
    }

    pub(crate) fn action(&self, seated: bool) -> Option<ToggleAction> {
        if self.pending {
            return None;
        }
        Some(if seated { ToggleAction::StandUp } else { ToggleAction::SitOnGround })
    }

    fn is_pending(&self) -> bool {
        self.pending
    }

    fn begin(&mut self) -> bool {
        if self.pending {
            return false;
        }
        self.pending = true;
        true
    }

    fn complete(&mut self) {
        self.pending = false;
    }
}

pub(super) struct SitControls {
    toggle: SitToggle,
    menu: Option<SitMenu>,
    sender: SyncSender<SitCompletion>,
    receiver: Receiver<SitCompletion>,
    notice: Option<(String, Instant)>,
}

impl SitControls {
    pub(super) fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        Self { toggle: SitToggle::default(), menu: None, sender, receiver, notice: None }
    }

    pub(super) fn open_menu(&mut self, hit: PickHit, camera: Camera, ray: DVec3, position: Pos2) {
        self.menu = sit_here_target(hit, camera, ray).map(|(id, offset)| SitMenu { id, offset, position });
    }

    pub(super) fn close_menu(&mut self) -> bool {
        let was_open = self.menu.is_some();
        self.menu = None;
        was_open
    }

    pub(super) fn handle_right_click(
        &mut self,
        capture: super::InputCapture,
        renderer: Option<&render::Renderer>,
        camera: Option<Camera>,
        cursor: PhysicalPosition<f64>,
        size: PhysicalSize<u32>,
        dpi: f64,
    ) {
        if capture.consumed || capture.pointer {
            return;
        }
        self.close_menu();
        let (Some(camera), Some(renderer)) = (camera, renderer) else { return };
        let ray = camera.ray([cursor.x, cursor.y], [size.width, size.height]);
        let hit = match renderer.pick(camera, ray) {
            Ok(hit) => hit,
            Err(error) => {
                tracing::debug!(%error, "scene context pick exceeded its work limit");
                None
            }
        };
        if let Some(hit) = hit {
            let position = egui::pos2((cursor.x / dpi) as f32, (cursor.y / dpi) as f32);
            self.open_menu(hit, camera, ray, position);
        }
    }

    pub(super) fn button(&mut self, ui: &mut Ui, seated: bool, available: bool) -> Option<SitRequest> {
        let action = self.toggle.action(seated);
        let response = ui.add_enabled(available && !self.toggle.is_pending(), Button::new(self.toggle.label(seated)));
        if response.clicked() {
            return match action? {
                ToggleAction::SitOnGround => Some(SitRequest::Ground),
                ToggleAction::StandUp => Some(SitRequest::Stand),
            };
        }
        None
    }

    pub(super) fn controls_ui(&mut self, root: &mut Ui, seated: bool, available: bool, now: Instant) -> (bool, Option<SitRequest>) {
        let mut request_chat = false;
        let mut sit_request = None;
        root.horizontal(|ui| {
            request_chat = ui.button("Nearby chat (F6)").clicked();
            sit_request = self.button(ui, seated, available);
        });
        let menu_request = self.overlays(root.ctx(), now);
        if sit_request.is_none() {
            sit_request = menu_request;
        }
        (request_chat, sit_request)
    }

    pub(super) fn overlays(&mut self, context: &Context, now: Instant) -> Option<SitRequest> {
        let mut request = None;
        if let Some(menu) = &self.menu {
            let response =
                Area::new(Id::unique("radia-viewport-sit-menu")).order(Order::Foreground).fixed_pos(menu.position).show(context, |ui| {
                    Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_min_width(96.0);
                        if ui.add_enabled(!self.toggle.is_pending(), Button::new("Sit Here")).clicked() {
                            request = Some(SitRequest::Object { id: menu.id, offset: menu.offset });
                        }
                    });
                });
            if context.input(|input| input.pointer.any_click()) && !response.response.contains_pointer() {
                self.menu = None;
            }
        }
        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.menu = None;
        }
        if self.notice.as_ref().is_some_and(|(_, expires)| now >= *expires) {
            self.notice = None;
        }
        if let Some((message, _)) = &self.notice {
            Area::new(Id::unique("radia-viewport-sit-notice"))
                .order(Order::Foreground)
                .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -18.0))
                .show(context, |ui| {
                    Frame::popup(ui.style()).show(ui, |ui| ui.label(message));
                });
        }
        request
    }

    pub(super) fn next_deadline(&self, now: Instant, repaint_delay: Option<Duration>) -> Option<Instant> {
        let repaint_at = repaint_delay.and_then(|delay| now.checked_add(delay));
        let notice_at = self.notice.as_ref().map(|(_, expires)| *expires);
        match (repaint_at, notice_at) {
            (Some(repaint), Some(notice)) => Some(repaint.min(notice)),
            (Some(repaint), None) => Some(repaint),
            (None, Some(notice)) => Some(notice),
            (None, None) => None,
        }
    }

    fn start(&mut self, request: SitRequest, feed: &SceneFeed, wake: std::sync::Arc<super::SharedWindowState>) -> bool {
        if !self.toggle.begin() {
            return false;
        }
        self.menu = None;
        let sender = self.sender.clone();
        let agent_control = feed.agent_control().clone();
        let runtime = feed.runtime.clone();
        drop(runtime.spawn(async move {
            let completion = match request {
                SitRequest::Object { id, offset } => SitCompletion::Object(agent_control.sit_on_object(id, offset).await),
                SitRequest::Ground => SitCompletion::Control(agent_control.sit_on_ground().await),
                SitRequest::Stand => SitCompletion::Control(agent_control.stand_up().await),
            };
            match sender.try_send(completion) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => tracing::warn!("sit result queue is full"),
                Err(TrySendError::Disconnected(_)) => tracing::trace!("viewport closed before sit result"),
            }
            wake.wake();
        }));
        true
    }

    pub(super) fn start_if_requested(
        &mut self,
        request: Option<SitRequest>,
        feed: Option<&SceneFeed>,
        wake: std::sync::Arc<super::SharedWindowState>,
    ) -> bool {
        let Some((request, feed)) = request.zip(feed) else { return false };
        self.start(request, feed, wake)
    }

    pub(super) fn poll(&mut self, now: Instant) {
        match self.receiver.try_recv() {
            Ok(completion) => {
                self.toggle.complete();
                self.notice = completion.notice().map(|message| (message, now + NOTICE_DURATION));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.toggle.complete();
                self.notice = Some(("Sit request ended unexpectedly.".to_owned(), now + NOTICE_DURATION));
            }
        }
    }
}

pub(super) enum SitRequest {
    Object { id: Uuid, offset: Vec3 },
    Ground,
    Stand,
}

struct SitMenu {
    id: Uuid,
    offset: Vec3,
    position: Pos2,
}

enum SitCompletion {
    Object(Result<SitOutcome, AgentControlError>),
    Control(Result<(), AgentControlError>),
}

impl SitCompletion {
    fn notice(self) -> Option<String> {
        match self {
            Self::Object(Ok(SitOutcome::Accepted { .. })) | Self::Control(Ok(())) => None,
            Self::Object(Ok(SitOutcome::Refused)) => Some("Sit request was refused.".to_owned()),
            Self::Object(Ok(SitOutcome::TimedOut)) => Some("Sit request timed out.".to_owned()),
            Self::Object(Ok(SitOutcome::Cancelled)) => Some("Sit request was cancelled.".to_owned()),
            Self::Object(Ok(SitOutcome::SimulatorChanged)) => Some("Sit request ended after a simulator change.".to_owned()),
            Self::Object(Err(error)) | Self::Control(Err(error)) => Some(format!("Sit action failed: {error}.")),
        }
    }
}

fn sit_here_target(hit: PickHit, camera: Camera, ray: DVec3) -> Option<(Uuid, Vec3)> {
    let PickTarget::Object(object) = hit.target else { return None };
    if object.kind != PickObjectKind::Prim {
        return None;
    }
    let offset = agent_request_sit_offset(hit.position, object, camera, ray)?;
    Some((object.id, offset))
}

pub(crate) fn agent_request_sit_offset(hit: DVec3, object: PickObject, camera: Camera, ray: DVec3) -> Option<Vec3> {
    if !hit.is_finite() || !object.position.is_finite() || !object.scale.is_finite() || !object.rotation.is_finite() || !ray.is_finite() {
        return None;
    }
    let ray = ray.try_normalize()?;
    let extents = object.scale.abs().max(Vec3::splat(MIN_OBJECT_EXTENT));
    let inverse_rotation = object.rotation.conjugate();
    let camera_to_object = inverse_rotation * (object.position - camera.position()).as_vec3();
    let proportions = (camera_to_object / extents).abs().to_array();
    let local_axis = if proportions[0] > proportions[1] && proportions[0] > proportions[2] {
        Vec3::X
    } else if proportions[1] > proportions[2] {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let plane_normal = (object.rotation * local_axis).as_dvec3();
    let denominator = plane_normal.dot(ray);
    let focus = if denominator.abs() < OFFSET_PLANE_EPSILON {
        hit
    } else {
        let distance = plane_normal.dot(object.position - camera.position()) / denominator;
        if !distance.is_finite() || distance < 0.0 {
            return None;
        }
        camera.position() + ray * distance
    };
    let camera_to_focus = inverse_rotation * (focus - camera.position()).as_vec3();
    let mut focus_offset_local = inverse_rotation * (focus - object.position).as_vec3();
    let focus_values = focus_offset_local.to_array();
    let camera_values = camera_to_focus.to_array();
    let extent_values = extents.to_array();
    let clip_fraction: [f32; 3] = std::array::from_fn(|axis| {
        let offset = focus_values[axis];
        let half_extent = extent_values[axis] * 0.5;
        let distance_outside = if offset > 0.0 { (offset - half_extent).max(0.0) } else { (offset + half_extent).min(0.0) };
        if camera_values[axis].abs() < OFFSET_COMPONENT_EPSILON { 0.0 } else { distance_outside / camera_values[axis] }
    });
    let absolute_fraction = clip_fraction.map(f32::abs);
    let clip_axis = if absolute_fraction[0] > absolute_fraction[1] && absolute_fraction[0] > absolute_fraction[2] {
        0
    } else if absolute_fraction[1] > absolute_fraction[2] {
        1
    } else {
        2
    };
    focus_offset_local -= camera_to_focus * clip_fraction[clip_axis];
    let hit_offset_world = (hit - object.position).as_vec3();
    let hit_offset_local = inverse_rotation * hit_offset_world;
    let camera_from_object = inverse_rotation * (camera.position() - object.position).as_vec3();
    let camera_inside = (0..3).all(|axis| camera_from_object[axis].abs() <= extents[axis] * 0.5);
    if camera_inside {
        return hit_offset_local.is_finite().then_some(hit_offset_local);
    }
    let view_distance = camera.position().distance(hit) as f32;
    if !view_distance.is_finite() || view_distance <= f32::EPSILON {
        return Some(focus_offset_local);
    }
    let relative_distance = (hit_offset_world.dot(camera.direction())).abs();
    let depth_fraction = relative_distance / view_distance;
    let bias = ((depth_fraction - OFFSET_BIAS_START) / (OFFSET_BIAS_END - OFFSET_BIAS_START)).clamp(0.0, 1.0);
    let offset = focus_offset_local.lerp(hit_offset_local, bias);
    offset.is_finite().then_some(offset)
}

#[cfg(test)]
mod tests {
    use math::{DVec3, Quat, Vec3};
    use render::{PickHit, PickObject, PickObjectKind, PickTarget, scene::Camera};
    use uuid::Uuid;

    use super::{SitToggle, ToggleAction, agent_request_sit_offset, sit_here_target};

    #[test]
    fn toggle_tracks_world_state() {
        let mut toggle = SitToggle::default();
        assert_eq!(toggle.label(false), "Sit Down");
        assert_eq!(toggle.action(false), Some(ToggleAction::SitOnGround));
        assert_eq!(toggle.label(true), "Stand Up");
        assert_eq!(toggle.action(true), Some(ToggleAction::StandUp));
        assert!(toggle.begin());
        assert_eq!(toggle.action(false), None);
        assert_eq!(toggle.action(true), None);
        toggle.complete();
        assert_eq!(toggle.action(false), Some(ToggleAction::SitOnGround));
    }

    #[test]
    fn sit_offset_uses_focus_projection() {
        let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [128, 128]).unwrap();
        let object = PickObject {
            id: Uuid::from_u128(7),
            kind: PickObjectKind::Prim,
            position: DVec3::new(10.0, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(2.0),
        };
        let offset = agent_request_sit_offset(DVec3::new(9.0, 0.0, 0.0), object, camera, DVec3::X).unwrap();
        assert!((offset.x + 1.0 / 54.0).abs() < 1e-5);
        assert_eq!(offset.y, 0.0);
        assert_eq!(offset.z, 0.0);
    }

    #[test]
    fn rotated_prim_offset_is_local() {
        let camera = Camera::new(DVec3::new(0.0, -10.0, 0.0), Vec3::Y, 1.0, [128, 128]).unwrap();
        let object = PickObject {
            id: Uuid::from_u128(8),
            kind: PickObjectKind::Prim,
            position: DVec3::ZERO,
            rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            scale: Vec3::splat(2.0),
        };
        let offset = agent_request_sit_offset(DVec3::new(0.0, -1.0, 0.0), object, camera, DVec3::Y).unwrap();
        assert!((offset.x + 1.0 / 54.0).abs() < 1e-5);
        assert!(offset.y.abs() < 1e-5);
        assert!(offset.z.abs() < 1e-5);
    }

    #[test]
    fn sit_here_preserves_prim_uuid() {
        let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [128, 128]).unwrap();
        let id = Uuid::from_u128(0x123456789abcdef00123456789abcdef);
        let object = PickObject {
            id,
            kind: PickObjectKind::Prim,
            position: DVec3::new(10.0, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(2.0),
        };
        let (target, _) =
            sit_here_target(PickHit { target: PickTarget::Object(object), position: DVec3::new(9.0, 0.0, 0.0) }, camera, DVec3::X).unwrap();
        assert_eq!(target, id);
    }

    #[test]
    fn non_prims_cannot_sit_here() {
        let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [128, 128]).unwrap();
        let object = PickObject {
            id: Uuid::from_u128(9),
            kind: PickObjectKind::Avatar,
            position: DVec3::new(10.0, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        };
        assert!(
            sit_here_target(PickHit { target: PickTarget::Object(object), position: DVec3::new(9.0, 0.0, 0.0) }, camera, DVec3::X,)
                .is_none()
        );
        assert!(sit_here_target(PickHit { target: PickTarget::Terrain, position: DVec3::new(9.0, 0.0, 0.0) }, camera, DVec3::X).is_none());
    }
}
