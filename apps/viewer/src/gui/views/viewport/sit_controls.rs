use std::{
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    time::{Duration, Instant},
};

use egui::{Align2, Area, Button, Context, Frame, Id, LayerId, Order, Popup, PopupCloseBehavior, PopupKind, Pos2, Ui, Vec2};
use math::{DVec3, Vec3};
use render::{PickHit, PickObject, PickObjectKind, PickTarget, Renderer, scene::Camera};
use uuid::Uuid;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use world::{CLICK_ACTION_SIT, Kind};

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
pub(super) enum SitMenuEvent {
    OutsideClick,
    Escape,
    Chosen,
    RightClickElsewhere,
    FocusLost,
    MovementKey,
    CameraDragStarted,
    TargetDisappeared,
    SessionEnded,
}

pub(super) fn sit_request_for_left_click(
    renderer: Option<&Renderer>,
    camera: Option<Camera>,
    feed: Option<&mut SceneFeed>,
    cursor: PhysicalPosition<f64>,
    size: PhysicalSize<u32>,
) -> Option<SitRequest> {
    let (Some(renderer), Some(camera)) = (renderer, camera) else { return None };
    let ray = camera.ray([cursor.x, cursor.y], [size.width, size.height]);
    let hit = match renderer.pick(camera, ray) {
        Ok(Some(hit)) => hit,
        Ok(None) => return None,
        Err(error) => {
            tracing::debug!(%error, "scene sit-action pick exceeded its work limit");
            return None;
        }
    };
    let PickTarget::Object(object) = hit.target else { return None };
    let feed = feed?;
    let root_click_action = feed.pick_root_click_action(object);
    sit_click_request(hit, camera, ray, root_click_action, feed.agent_available(), feed.agent_is_seated())
}

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

    pub(crate) fn action(&self, seated: bool, available: bool) -> Option<ToggleAction> {
        if self.pending || !available {
            return None;
        }
        Some(if seated { ToggleAction::StandUp } else { ToggleAction::SitOnGround })
    }

    pub(crate) fn visible(&self, agent_available: bool) -> bool {
        agent_available
    }

    fn is_pending(&self) -> bool {
        self.pending
    }

    fn enabled(&self, agent_available: bool, controls_available: bool) -> bool {
        self.visible(agent_available) && controls_available && !self.pending
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
    next_menu_id: u64,
    sender: SyncSender<SitCompletion>,
    receiver: Receiver<SitCompletion>,
    notice: Option<(String, Instant)>,
}

impl SitControls {
    pub(super) fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        Self { toggle: SitToggle::default(), menu: None, next_menu_id: 0, sender, receiver, notice: None }
    }

    pub(super) fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    pub(super) fn handle_menu_event(&mut self, event: SitMenuEvent) -> bool {
        let closes = matches!(
            event,
            SitMenuEvent::OutsideClick
                | SitMenuEvent::Escape
                | SitMenuEvent::Chosen
                | SitMenuEvent::RightClickElsewhere
                | SitMenuEvent::FocusLost
                | SitMenuEvent::MovementKey
                | SitMenuEvent::CameraDragStarted
                | SitMenuEvent::TargetDisappeared
                | SitMenuEvent::SessionEnded
        );
        closes && self.menu.take().is_some()
    }

    pub(super) fn handle_left_click_while_menu_open(&mut self, captured_by_egui: bool) -> bool {
        if self.menu.is_none() {
            return false;
        }
        if !captured_by_egui {
            self.handle_menu_event(SitMenuEvent::OutsideClick);
        }
        true
    }

    pub(super) fn handle_right_click_while_menu_open(&mut self, captured_by_egui: bool) -> bool {
        if self.menu.is_none() {
            return !captured_by_egui;
        }
        self.handle_menu_event(if captured_by_egui { SitMenuEvent::OutsideClick } else { SitMenuEvent::RightClickElsewhere });
        !captured_by_egui
    }

    fn open_menu(&mut self, mut menu: SitMenu) {
        self.next_menu_id = self.next_menu_id.wrapping_add(1);
        menu.popup_id = Id::unique(("radia-viewport-sit-menu", self.next_menu_id));
        self.menu = Some(menu);
    }

    pub(super) fn reconcile_menu(&mut self, feed: Option<&SceneFeed>) -> bool {
        let Some(menu) = &self.menu else { return false };
        match feed {
            None => self.handle_menu_event(SitMenuEvent::SessionEnded),
            Some(feed) if !feed.agent_available() => self.handle_menu_event(SitMenuEvent::SessionEnded),
            Some(feed) if !feed.knows_pick(menu.object) => self.handle_menu_event(SitMenuEvent::TargetDisappeared),
            Some(_) => false,
        }
    }

    pub(super) fn handle_right_click(
        &mut self,
        capture: super::InputCapture,
        renderer: Option<&render::Renderer>,
        feed: Option<&SceneFeed>,
        camera: Option<Camera>,
        cursor: PhysicalPosition<f64>,
        size: PhysicalSize<u32>,
        dpi: f64,
    ) {
        let may_pick = self.handle_right_click_while_menu_open(capture.consumed || capture.pointer);
        if !may_pick {
            return;
        }
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
            if let PickTarget::Object(object) = hit.target {
                let world_knows_object = feed.is_some_and(|feed| feed.knows_pick(object));
                tracing::debug!(
                    object_local_id = object.local_id.get(),
                    object_id = %object.id,
                    world_knows_object,
                    "viewport object picked"
                );
                if let Some(feed) = feed.filter(|_| world_knows_object)
                    && let Some((_, offset)) = sit_here_target(hit, camera, ray)
                    && let Some(root_id) = feed.root_object_id(object.id)
                {
                    self.open_menu(SitMenu { object, root_id, offset, position, popup_id: Id::NULL });
                }
            }
        }
    }

    pub(super) fn button(&mut self, ui: &mut Ui, seated: bool, agent_available: bool, controls_available: bool) -> Option<SitRequest> {
        if !self.toggle.visible(agent_available) {
            return None;
        }
        let action = self.toggle.action(seated, controls_available);
        let response = ui.add_enabled(self.toggle.enabled(agent_available, controls_available), Button::new(self.toggle.label(seated)));
        if response.clicked() {
            return match action? {
                ToggleAction::SitOnGround => Some(SitRequest::Ground),
                ToggleAction::StandUp => Some(SitRequest::Stand),
            };
        }
        None
    }

    pub(super) fn controls_ui(
        &mut self,
        controls: &mut Ui,
        seated: bool,
        agent_available: bool,
        controls_available: bool,
    ) -> (bool, Option<SitRequest>) {
        let mut request_chat = false;
        let mut sit_request = None;
        controls.horizontal(|ui| {
            request_chat = ui.button("Nearby chat (F6)").clicked();
            sit_request = self.button(ui, seated, agent_available, controls_available);
        });
        (request_chat, sit_request)
    }

    pub(super) fn overlays(&mut self, context: &Context, now: Instant, feed: Option<&SceneFeed>) -> Option<SitRequest> {
        self.reconcile_menu(feed);
        let mut request = None;
        if let Some(menu) = &self.menu {
            let menu = *menu;
            let world_knows_object = feed.is_some_and(|feed| feed.knows_pick(menu.object));
            let sitting_on_target = feed.is_some_and(|feed| feed.agent_is_sitting_on(menu.root_id));
            let mut open = true;
            let layer_id = LayerId::new(PopupKind::Menu.order(), menu.popup_id);
            Popup::new(menu.popup_id, context.clone(), menu.position, layer_id)
                .kind(PopupKind::Menu)
                .layout(egui::Layout::top_down_justified(egui::Align::Min))
                .style(egui::containers::menu::menu_style)
                .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                .open_bool(&mut open)
                .show(|ui| {
                    ui.set_min_width(96.0);
                    if ui
                        .add_enabled(menu.enabled(world_knows_object, self.toggle.is_pending()), Button::new(menu.label(sitting_on_target)))
                        .clicked()
                    {
                        request = Some(menu.request(sitting_on_target));
                    }
                });
            if request.is_some() {
                self.handle_menu_event(SitMenuEvent::Chosen);
            } else if !open {
                let event = if context.input(|input| input.key_pressed(egui::Key::Escape)) {
                    SitMenuEvent::Escape
                } else {
                    SitMenuEvent::OutsideClick
                };
                self.handle_menu_event(event);
            }
        }
        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.handle_menu_event(SitMenuEvent::Escape);
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
        self.handle_menu_event(SitMenuEvent::Chosen);
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

#[derive(Debug)]
pub(super) enum SitRequest {
    Object { id: Uuid, offset: Vec3 },
    Ground,
    Stand,
}

#[derive(Clone, Copy)]
struct SitMenu {
    object: PickObject,
    root_id: Uuid,
    offset: Vec3,
    position: Pos2,
    popup_id: Id,
}

impl SitMenu {
    fn label(&self, sitting_on_target: bool) -> &'static str {
        if sitting_on_target { "Stand Up" } else { "Sit Here" }
    }

    fn request(&self, sitting_on_target: bool) -> SitRequest {
        if sitting_on_target { SitRequest::Stand } else { SitRequest::Object { id: self.object.id, offset: self.offset } }
    }

    fn enabled(&self, world_knows_object: bool, pending: bool) -> bool {
        world_knows_object && !pending
    }
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

pub(super) fn sit_click_request(
    hit: PickHit,
    camera: Camera,
    ray: DVec3,
    root_click_action: Option<(Kind, u8)>,
    agent_available: bool,
    seated: bool,
) -> Option<SitRequest> {
    let PickTarget::Object(object) = hit.target else { return None };
    let (root_kind, root_action) = root_click_action?;
    if object.kind != PickObjectKind::Prim || root_kind != Kind::Object || !agent_available || seated {
        return None;
    }
    let click_action = if object.click_action == 0 { root_action } else { object.click_action };
    if click_action != CLICK_ACTION_SIT {
        return None;
    }
    let offset = agent_request_sit_offset(hit.position, object, camera, ray)?;
    Some(SitRequest::Object { id: object.id, offset })
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
    use std::num::NonZeroU32;

    use egui::Id;
    use math::{DVec3, Quat, Vec3};
    use render::{PickHit, PickObject, PickObjectKind, PickTarget, scene::Camera};
    use uuid::Uuid;

    use super::{
        SitControls, SitMenu, SitMenuEvent, SitRequest, SitToggle, ToggleAction, agent_request_sit_offset, sit_click_request,
        sit_here_target,
    };
    use world::{CLICK_ACTION_DISABLED, CLICK_ACTION_SIT, Kind};

    #[test]
    fn toggle_tracks_world_state() {
        let mut toggle = SitToggle::default();
        assert_eq!(toggle.label(false), "Sit Down");
        assert_eq!(toggle.action(false, true), Some(ToggleAction::SitOnGround));
        assert_eq!(toggle.label(true), "Stand Up");
        assert_eq!(toggle.action(true, true), Some(ToggleAction::StandUp));
        assert!(toggle.begin());
        assert_eq!(toggle.action(false, true), None);
        assert_eq!(toggle.action(true, true), None);
        toggle.complete();
        assert_eq!(toggle.action(false, true), Some(ToggleAction::SitOnGround));
    }

    #[test]
    fn toggle_requires_agent_and_controls() {
        let toggle = SitToggle::default();
        assert!(!toggle.visible(false));
        assert!(!toggle.enabled(false, true));
        assert!(!toggle.enabled(true, false));
        assert!(toggle.enabled(true, true));
        assert_eq!(toggle.action(false, false), None);
    }

    #[test]
    fn sit_menu_stands_on_current_target() {
        let menu = SitMenu {
            object: PickObject {
                id: Uuid::from_u128(9),
                local_id: NonZeroU32::new(10).unwrap(),
                kind: PickObjectKind::Prim,
                click_action: 0,
                position: DVec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            root_id: Uuid::from_u128(9),
            offset: Vec3::X,
            position: egui::Pos2::ZERO,
            popup_id: Id::NULL,
        };
        assert_eq!(menu.label(false), "Sit Here");
        std::assert_matches!(menu.request(false), SitRequest::Object { id, offset } if id == Uuid::from_u128(9) && offset == Vec3::X);
        assert_eq!(menu.label(true), "Stand Up");
        std::assert_matches!(menu.request(true), SitRequest::Stand);
        assert!(menu.enabled(true, false));
        assert!(!menu.enabled(false, false));
        assert!(!menu.enabled(true, true));
    }

    fn test_sit_menu() -> SitMenu {
        SitMenu {
            object: PickObject {
                id: Uuid::from_u128(9),
                local_id: NonZeroU32::new(10).unwrap(),
                kind: PickObjectKind::Prim,
                click_action: 0,
                position: DVec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            root_id: Uuid::from_u128(9),
            offset: Vec3::X,
            position: egui::Pos2::ZERO,
            popup_id: Id::NULL,
        }
    }

    #[test]
    fn menu_dismissal_events_close_the_open_menu() {
        let events = [
            SitMenuEvent::OutsideClick,
            SitMenuEvent::Escape,
            SitMenuEvent::Chosen,
            SitMenuEvent::RightClickElsewhere,
            SitMenuEvent::FocusLost,
            SitMenuEvent::MovementKey,
            SitMenuEvent::CameraDragStarted,
            SitMenuEvent::TargetDisappeared,
            SitMenuEvent::SessionEnded,
        ];
        for event in events {
            let mut controls = SitControls::new();
            controls.open_menu(test_sit_menu());

            assert!(controls.handle_menu_event(event), "{event:?} should close the open menu");
            assert!(!controls.menu_open());
            assert!(!controls.handle_menu_event(event));
        }

        let mut controls = SitControls::new();
        controls.open_menu(test_sit_menu());
        assert!(controls.handle_left_click_while_menu_open(true));
        assert!(controls.menu_open());
    }

    #[test]
    fn menu_left_click_is_consumed_and_right_click_reopens_only_outside_ui() {
        let mut controls = SitControls::new();
        controls.open_menu(test_sit_menu());
        assert!(controls.handle_left_click_while_menu_open(true));
        assert!(controls.menu_open());
        assert!(controls.handle_left_click_while_menu_open(false));
        assert!(!controls.menu_open());

        controls.open_menu(test_sit_menu());
        let first_popup_id = controls.menu.expect("opened sit menu").popup_id;
        assert!(controls.handle_right_click_while_menu_open(false));
        assert!(!controls.menu_open());
        let position = egui::pos2(32.0, 64.0);
        controls.open_menu(SitMenu { position, ..test_sit_menu() });
        let new_menu = controls.menu.expect("right-click opens a menu at the new target");
        assert_eq!(new_menu.position, position);
        assert_ne!(new_menu.popup_id, first_popup_id);

        controls.open_menu(test_sit_menu());
        assert!(!controls.handle_right_click_while_menu_open(true));
        assert!(!controls.menu_open());
    }

    #[test]
    fn sit_offset_uses_focus_projection() {
        let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [128, 128]).unwrap();
        let object = PickObject {
            id: Uuid::from_u128(7),
            local_id: NonZeroU32::new(1).unwrap(),
            kind: PickObjectKind::Prim,
            click_action: 0,
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
            local_id: NonZeroU32::new(2).unwrap(),
            kind: PickObjectKind::Prim,
            click_action: 0,
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
            local_id: NonZeroU32::new(3).unwrap(),
            kind: PickObjectKind::Prim,
            click_action: 0,
            position: DVec3::new(10.0, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(2.0),
        };
        let (target, _) =
            sit_here_target(PickHit { target: PickTarget::Object(object), position: DVec3::new(9.0, 0.0, 0.0) }, camera, DVec3::X).unwrap();
        assert_eq!(target, id);
    }

    #[test]
    fn sit_click_uses_clicked_or_root_action() {
        let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [128, 128]).unwrap();
        let id = Uuid::from_u128(0x123456789abcdef00123456789abcdef);
        let object = PickObject {
            id,
            local_id: NonZeroU32::new(5).unwrap(),
            kind: PickObjectKind::Prim,
            click_action: 0,
            position: DVec3::new(10.0, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(2.0),
        };
        let hit = PickHit { target: PickTarget::Object(object), position: DVec3::new(9.0, 0.0, 0.0) };
        let request = sit_click_request(hit, camera, DVec3::X, Some((Kind::Object, CLICK_ACTION_SIT)), true, false).unwrap();
        std::assert_matches!(request, SitRequest::Object { id: target, .. } if target == id);

        let clicked = PickObject { click_action: CLICK_ACTION_SIT, ..object };
        let hit = PickHit { target: PickTarget::Object(clicked), ..hit };
        let request = sit_click_request(hit, camera, DVec3::X, Some((Kind::Object, 0)), true, false).unwrap();
        std::assert_matches!(request, SitRequest::Object { id: target, .. } if target == id);
        assert!(sit_click_request(hit, camera, DVec3::X, Some((Kind::Avatar, CLICK_ACTION_SIT)), true, false).is_none());
        let disabled = PickObject { click_action: CLICK_ACTION_DISABLED, ..object };
        let hit = PickHit { target: PickTarget::Object(disabled), ..hit };
        assert!(sit_click_request(hit, camera, DVec3::X, Some((Kind::Object, CLICK_ACTION_SIT)), true, false).is_none());
    }

    #[test]
    fn sit_click_while_seated_does_not_stand() {
        let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [128, 128]).unwrap();
        let object = PickObject {
            id: Uuid::from_u128(10),
            local_id: NonZeroU32::new(6).unwrap(),
            kind: PickObjectKind::Prim,
            click_action: CLICK_ACTION_SIT,
            position: DVec3::new(10.0, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(2.0),
        };
        let hit = PickHit { target: PickTarget::Object(object), position: DVec3::new(9.0, 0.0, 0.0) };
        assert!(sit_click_request(hit, camera, DVec3::X, Some((Kind::Object, 0)), true, true).is_none());
    }

    #[test]
    fn non_prims_cannot_sit_here() {
        let camera = Camera::new(DVec3::ZERO, Vec3::X, 1.0, [128, 128]).unwrap();
        let object = PickObject {
            id: Uuid::from_u128(9),
            local_id: NonZeroU32::new(4).unwrap(),
            kind: PickObjectKind::Avatar,
            click_action: 0,
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
