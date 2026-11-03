use std::{
    sync::{Arc, Weak},
    time::Instant,
};

use ::inventory::{InventoryOwner, ItemKey};
use render::scene;
use utils::ItemId;

use super::FRAME_INTERVAL;
use crate::{inventory::environments::EnvironmentMenu, scene_feed::SceneFeed};

#[derive(Default)]
pub(super) struct EnvironmentControls {
    label: Option<(scene::EnvironmentChoice, Weak<EnvironmentMenu>, String)>,
}

impl EnvironmentControls {
    pub(super) fn draw(&mut self, ui: &mut egui::Ui, feed: Option<&SceneFeed>) {
        let current_environment = feed.map_or_else(scene::EnvironmentChoice::default, SceneFeed::environment_choice);
        let mut selected_environment = current_environment.clone();
        let selected_text = match &selected_environment {
            scene::EnvironmentChoice::Shared => "Shared environment".to_owned(),
            scene::EnvironmentChoice::Local(scene::LocalChoice::Preset(preset)) => preset.label().to_owned(),
            scene::EnvironmentChoice::Local(scene::LocalChoice::Inventory { owner_id, item_id, .. }) => feed
                .and_then(|feed| {
                    let crate::inventory::environments::EnvironmentItems::Ready(items) = feed.environment_items() else {
                        return None;
                    };
                    if let Some((choice, previous, label)) = &self.label
                        && choice == &selected_environment
                        && previous.upgrade().is_some_and(|previous| Arc::ptr_eq(&previous, &items))
                    {
                        return Some(label.clone());
                    }
                    let key = ItemKey::new(InventoryOwner::new(*owner_id), ItemId::new(*item_id));
                    let label = items.label(key)?;
                    self.label = Some((selected_environment.clone(), Arc::downgrade(&items), label.clone()));
                    Some(label)
                })
                .unwrap_or_else(|| "Saved environment".into()),
        };
        let environment_items = feed.map(SceneFeed::environment_items);
        let can_select_environment = feed.is_some_and(|feed| feed.agent().is_some());
        ui.add_enabled_ui(can_select_environment, |ui| {
            egui::ComboBox::from_label("Environment").selected_text(selected_text).show_ui(ui, |ui| {
                ui.selectable_value(&mut selected_environment, scene::EnvironmentChoice::Shared, "Shared environment");
                for preset in scene::EnvironmentPreset::ALL {
                    ui.selectable_value(
                        &mut selected_environment,
                        scene::EnvironmentChoice::Local(scene::LocalChoice::Preset(preset)),
                        preset.label(),
                    );
                }
                ui.separator();
                match environment_items {
                    Some(crate::inventory::environments::EnvironmentItems::Ready(items)) => {
                        if items.items().is_empty() {
                            ui.label("No saved environments");
                        } else {
                            let rows = items.items();
                            let row_height = ui.text_style_height(&egui::TextStyle::Button);
                            egui::ScrollArea::vertical().max_height(320.0).show_rows(ui, row_height, rows.len(), |ui, visible| {
                                for index in visible {
                                    let Some(item) = rows.get(index) else { continue };
                                    let choice = scene::EnvironmentChoice::Local(scene::LocalChoice::Inventory {
                                        owner_id: item.key.owner().id(),
                                        item_id: item.key.id().uuid(),
                                        asset_id: item.asset_id,
                                    });
                                    ui.add_enabled_ui(!item.asset_id.uuid().is_nil(), |ui| {
                                        let label = item.label();
                                        ui.selectable_value(&mut selected_environment, choice, &label);
                                    });
                                }
                            });
                        }
                    }
                    Some(crate::inventory::environments::EnvironmentItems::Failed) => {
                        ui.label("Saved environments unavailable");
                    }
                    Some(crate::inventory::environments::EnvironmentItems::Loading) | None => {
                        ui.label("Loading saved environments…");
                    }
                }
            });
        });
        if selected_environment != current_environment
            && let Some(feed) = feed
        {
            feed.set_environment_choice(selected_environment);
        }
        if let Some(failure) = feed.and_then(|feed| feed.view.viewer_environment()).and_then(|environment| environment.local_error()) {
            ui.colored_label(egui::Color32::LIGHT_RED, local_environment_failure(failure));
        }
    }
}

fn local_environment_failure(failure: assets::Failure) -> String {
    match failure {
        assets::Failure::MissingCapability => "The environment asset capability is unavailable.".into(),
        assets::Failure::NotFound => "The environment asset was not found.".into(),
        assets::Failure::Denied => "Access to the environment asset was denied.".into(),
        assets::Failure::HttpStatus { status } => format!("The environment request failed with HTTP status {status}."),
        assets::Failure::Network => "The environment server could not be reached.".into(),
        assets::Failure::MemoryPressure => "The viewer has insufficient memory to load the environment.".into(),
        assets::Failure::TooLarge => "The environment asset is too large.".into(),
        assets::Failure::Malformed => "The environment settings could not be decoded.".into(),
        assets::Failure::Unsupported => "The environment settings type is unsupported.".into(),
        assets::Failure::RouteClosed | assets::Failure::SessionClosed | assets::Failure::Cancelled => {
            "The environment request ended before it completed.".into()
        }
        assets::Failure::Deadline => "The environment request timed out.".into(),
        assets::Failure::Transient { .. } => "The environment request failed temporarily.".into(),
        assets::Failure::MissingLod(_) | assets::Failure::MalformedBlock | assets::Failure::UnsupportedVersion(_) => {
            "The environment asset is malformed or unsupported.".into()
        }
        assets::Failure::FullAssetRequired => "The environment asset could not be loaded.".into(),
    }
}

pub(super) fn sky_frame_deadline(camera: Option<scene::Camera>, animating: bool, now: Instant) -> Option<Instant> {
    let visible = camera.is_some_and(camera_sees_sky);
    (visible && animating).then(|| now + FRAME_INTERVAL)
}

pub(super) fn water_frame_deadline(camera: Option<scene::Camera>, height: Option<f64>, animating: bool, now: Instant) -> Option<Instant> {
    let visible = camera.zip(height).is_some_and(|(camera, height)| camera_sees_water(camera, height));
    (visible && animating).then(|| now + FRAME_INTERVAL)
}

fn camera_sees_sky(camera: scene::Camera) -> bool {
    let direction = camera.direction();
    let half_fov = camera.vertical_fov() as f32 * 0.5;
    let maximum_vertical = direction.z * half_fov.cos() + (1.0 - direction.z * direction.z).max(0.0).sqrt() * half_fov.sin();
    maximum_vertical > 0.0
}

fn camera_sees_water(camera: scene::Camera, height: f64) -> bool {
    if !height.is_finite() || camera.position().z <= height {
        return false;
    }
    let direction = camera.direction();
    let half_fov = camera.vertical_fov() as f32 * 0.5;
    let minimum_vertical = direction.z * half_fov.cos() - (1.0 - direction.z * direction.z).max(0.0).sqrt() * half_fov.sin();
    minimum_vertical < 0.0
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use math::{DVec3, Vec3};
    use render::scene::Camera;

    use super::{FRAME_INTERVAL, local_environment_failure, sky_frame_deadline, water_frame_deadline};

    #[test]
    fn local_environment_failure_shows_http_status_and_network_errors() {
        assert_eq!(
            local_environment_failure(assets::Failure::HttpStatus { status: 503 }),
            "The environment request failed with HTTP status 503."
        );
        assert_eq!(local_environment_failure(assets::Failure::Network), "The environment server could not be reached.");
    }

    #[test]
    fn sky_redraws_require_visible_sky_and_animation() {
        let now = Instant::now();
        let sky = Camera::new(DVec3::ZERO, Vec3::Z, 1.0, [1280, 720]).unwrap();
        let ground = Camera::new(DVec3::ZERO, Vec3::NEG_Z, 1.0, [1280, 720]).unwrap();
        assert_eq!(sky_frame_deadline(Some(sky), true, now), Some(now + FRAME_INTERVAL));
        assert_eq!(sky_frame_deadline(Some(sky), false, now), None);
        assert_eq!(sky_frame_deadline(Some(ground), true, now), None);
        assert_eq!(sky_frame_deadline(None, true, now), None);
    }

    #[test]
    fn water_redraws_require_visible_water_and_nonzero_waves() {
        let now = Instant::now();
        let water = Camera::new(DVec3::new(0.0, 0.0, 10.0), Vec3::NEG_Z, 1.0, [1280, 720]).unwrap();
        let sky = Camera::new(DVec3::new(0.0, 0.0, 10.0), Vec3::Z, 1.0, [1280, 720]).unwrap();
        let below = Camera::new(DVec3::new(0.0, 0.0, -1.0), Vec3::NEG_Z, 1.0, [1280, 720]).unwrap();
        assert_eq!(water_frame_deadline(Some(water), Some(0.0), true, now), Some(now + FRAME_INTERVAL));
        assert_eq!(water_frame_deadline(Some(water), Some(0.0), false, now), None);
        assert_eq!(water_frame_deadline(Some(sky), Some(0.0), true, now), None);
        assert_eq!(water_frame_deadline(Some(below), Some(0.0), true, now), None);
        assert_eq!(water_frame_deadline(Some(water), None, true, now), None);
    }
}
