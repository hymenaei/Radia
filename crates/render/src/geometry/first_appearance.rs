use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use utils::AssetId;
use world::Entity;

use super::items::CachedItem;
use crate::{
    material::{AssetState, FaceMaterial, FaceParams, Resolution},
    scene::{DrawnMeshKey, Geometry, Object, SceneView, ViewVersion},
    texture::Textures,
};

pub(super) const ASSET_WAIT: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct AppearanceKey {
    pub(super) entity: Entity,
    pub(super) asset: AssetId,
}

#[derive(Default)]
pub(super) struct FirstAppearance {
    pub(super) deadline: Option<Instant>,
    pub(super) shown: bool,
}

impl FirstAppearance {
    pub(super) fn allows_at(&mut self, now: Instant, complete: bool, geometry_ready: bool, assets_ready: bool) -> bool {
        if !complete {
            return false;
        }
        self.deadline.get_or_insert_with(|| now.checked_add(ASSET_WAIT).unwrap_or(now));
        geometry_ready && (self.shown || assets_ready || self.deadline.is_some_and(|deadline| now >= deadline))
    }

    pub(super) fn deadline(&self, now: Instant) -> Option<Instant> {
        if self.shown { None } else { self.deadline.filter(|deadline| *deadline > now) }
    }
}

#[derive(Default)]
pub(super) struct FirstAppearances {
    entries: HashMap<AppearanceKey, FirstAppearance>,
    version: Option<ViewVersion>,
}

pub(super) struct AppearanceDecision {
    pub(super) key: Option<AppearanceKey>,
    pub(super) eligible: bool,
    pub(super) waiting: bool,
    pub(super) deadline: Option<Instant>,
}

impl FirstAppearances {
    pub(super) fn sync(&mut self, view: &SceneView) {
        if self.version.as_ref() == Some(&view.geometry_version()) {
            return;
        }
        self.entries.retain(|key, _| view.object(key.entity).and_then(appearance_key) == Some(*key));
        self.version = Some(view.geometry_version());
    }

    pub(super) fn allows_at(&mut self, key: AppearanceKey, now: Instant, complete: bool, geometry_ready: bool, assets_ready: bool) -> bool {
        if !complete {
            return false;
        }
        self.entries.entry(key).or_default().allows_at(now, complete, geometry_ready, assets_ready)
    }

    pub(super) fn mark_shown(&mut self, key: AppearanceKey) {
        if let Some(appearance) = self.entries.get_mut(&key) {
            appearance.shown = true;
        }
    }

    pub(super) fn deadline(&self, key: AppearanceKey, now: Instant) -> Option<Instant> {
        self.entries.get(&key)?.deadline(now)
    }

    pub(super) fn shown(&self, key: AppearanceKey) -> bool {
        self.entries.get(&key).is_some_and(|appearance| appearance.shown)
    }
}

pub(super) fn appearance_key(object: &Object) -> Option<AppearanceKey> {
    let Geometry::Mesh(asset) = object.geometry() else { return None };
    Some(AppearanceKey { entity: object.detail().summary.entity, asset })
}

pub(super) fn item_key(item: &CachedItem) -> Option<AppearanceKey> {
    appearance_key(item.source.as_ref()?)
}

pub(super) fn current_item_key(view: &SceneView, item: &CachedItem) -> Option<AppearanceKey> {
    let key = item_key(item)?;
    (view.object(key.entity).and_then(appearance_key) == Some(key)).then_some(key)
}

pub(super) fn decide_item(
    appearances: &mut FirstAppearances,
    view: &SceneView,
    textures: &Textures,
    item: &CachedItem,
    geometry_ready: bool,
    now: Instant,
) -> AppearanceDecision {
    let Some(key) = item_key(item) else {
        return AppearanceDecision { key: None, eligible: geometry_ready, waiting: false, deadline: None };
    };
    if view.object(key.entity).and_then(appearance_key) != Some(key) {
        return AppearanceDecision { key: None, eligible: geometry_ready, waiting: false, deadline: None };
    }
    if !complete_candidate(item) {
        return AppearanceDecision { key: Some(key), eligible: false, waiting: false, deadline: None };
    }
    let assets_ready = item.source.as_ref().and_then(Object::materials).is_some_and(|faces| assets_renderable(faces, textures));
    let eligible = appearances.allows_at(key, now, true, geometry_ready, assets_ready);
    AppearanceDecision {
        key: Some(key),
        eligible,
        waiting: !eligible,
        deadline: (!eligible).then(|| appearances.deadline(key, now)).flatten(),
    }
}

pub(super) fn complete_candidate(item: &CachedItem) -> bool {
    let Some(object) = item.source.as_ref() else { return false };
    let Some((DrawnMeshKey::Asset { asset, .. }, mesh)) = object.mesh() else { return false };
    let Geometry::Mesh(current_asset) = object.geometry() else { return false };
    if asset != current_asset || mesh.id() != item.mesh.id() || !object.mesh_matches_geometry() {
        return false;
    }
    let materials = object.materials();
    let mut material_slots = [None; 256];
    if let Some(materials) = materials {
        for (index, material) in materials.iter().enumerate() {
            material_slots[usize::from(material.slot)].get_or_insert(index);
        }
    }
    mesh.mesh().faces().iter().all(|face| {
        if face.indices.is_empty() {
            return true;
        }
        let material = materials.and_then(|materials| material_slots[usize::from(face.slot)].and_then(|index| materials.get(index)));
        !FaceParams::new(material).is_visible()
            || item.faces.iter().any(|cached| cached.order.3 == face.slot && cached.indices == face.indices)
    })
}

pub(super) fn assets_renderable(faces: &[FaceMaterial], textures: &Textures) -> bool {
    faces.iter().all(|face| {
        !FaceParams::new(Some(face)).is_visible()
            || (material_resolved(face.legacy_state)
                && material_resolved(face.pbr_state)
                && face.base_color_texture.as_ref().is_none_or(|binding| {
                    texture_resolved(binding.state(), binding.image().is_some_and(|image| textures.is_resident(image.id())))
                }))
    })
}

fn material_resolved(state: Option<AssetState>) -> bool {
    state.is_none_or(|state| state.resolution != Resolution::Pending)
}

fn texture_resolved(state: AssetState, resident: bool) -> bool {
    matches!(state.resolution, Resolution::Failed(_)) || resident
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn ready_assets_show_early() {
        let now = Instant::now();
        let mut appearance = FirstAppearance::default();

        assert!(appearance.allows_at(now, true, true, true));
        assert_eq!(appearance.deadline, Some(now + ASSET_WAIT));
        assert!(!appearance.shown);
    }

    #[test]
    fn pending_assets_expire_at_cap() {
        let now = Instant::now();
        let mut appearance = FirstAppearance::default();

        assert!(!appearance.allows_at(now, true, true, false));
        assert!(!appearance.allows_at(now + ASSET_WAIT - Duration::from_nanos(1), true, true, false));
        assert!(appearance.allows_at(now + ASSET_WAIT, true, true, false));
        assert_eq!(appearance.deadline, Some(now + ASSET_WAIT));
    }

    #[test]
    fn failed_assets_use_fallback() {
        let asset = AssetId::new(uuid::Uuid::from_u128(1));
        assert!(texture_resolved(AssetState { asset, resolution: Resolution::Failed(assets::Failure::Malformed) }, false));
        assert!(!texture_resolved(AssetState { asset, resolution: Resolution::Pending }, false));
        assert!(texture_resolved(AssetState { asset, resolution: Resolution::Pending }, true));
    }

    #[test]
    fn geometry_upload_stays_mandatory() {
        let now = Instant::now();
        let mut appearance = FirstAppearance::default();

        assert!(!appearance.allows_at(now, true, false, true));
        assert!(!appearance.allows_at(now + ASSET_WAIT, true, false, false));
        assert!(appearance.allows_at(now + ASSET_WAIT, true, true, false));
    }

    #[test]
    fn incomplete_candidate_has_no_deadline() {
        let now = Instant::now();
        let mut appearance = FirstAppearance::default();

        assert!(!appearance.allows_at(now, false, true, true));
        assert_eq!(appearance.deadline, None);
        assert!(appearance.allows_at(now + Duration::from_millis(10), true, true, true));
        assert_eq!(appearance.deadline, Some(now + Duration::from_millis(10) + ASSET_WAIT));
    }

    #[test]
    fn deadline_survives_reentry_and_lod() {
        let now = Instant::now();
        let mut appearance = FirstAppearance::default();

        assert!(!appearance.allows_at(now, true, true, false));
        let deadline = appearance.deadline;
        assert!(!appearance.allows_at(now + Duration::from_millis(100), true, true, false));
        assert_eq!(appearance.deadline, deadline);
        assert!(appearance.allows_at(now + ASSET_WAIT, true, true, false));
    }

    #[test]
    fn shown_survives_refinement() {
        let now = Instant::now();
        let mut appearance = FirstAppearance { deadline: Some(now + ASSET_WAIT), shown: true };

        assert!(appearance.allows_at(now + Duration::from_secs(1), true, true, false));
    }
}
