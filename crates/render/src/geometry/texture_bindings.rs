use std::collections::HashSet;

use assets::{Priority, TextureHandle, TextureId};
use wgpu::Device;
use world::{Entity, Region};

use super::GeometryRenderer;
use crate::{
    material::{FaceMaterial, FaceParams},
    scene::{Object, SceneView},
    texture::TextureKey,
};

#[derive(Clone)]
pub(super) struct TextureDemand {
    pub(super) key: TextureKey,
    pub(super) handle: Option<TextureHandle>,
}

#[derive(Clone)]
pub(super) struct TerrainDemand {
    pub(super) region: Region,
    pub(super) textures: [Option<TextureHandle>; 4],
}

impl GeometryRenderer {
    pub(super) fn request_frame_textures(&mut self, view: &SceneView, background: &[Entity]) {
        self.requested_textures.clear();
        let epoch = self.epoch;
        {
            let (draw_list, requested, textures) = (&self.draw_list, &mut self.requested_textures, &mut self.textures);
            for demand in draw_list.texture_demands() {
                if let Some(handle) = &demand.handle
                    && requested.insert(handle.id())
                {
                    textures.request(handle, epoch, Priority::Visible);
                }
            }
            for demand in draw_list.terrain_demands() {
                for handle in demand.textures.iter().flatten() {
                    if requested.insert(handle.id()) {
                        textures.request(handle, epoch, Priority::Visible);
                    }
                }
            }
        }
        for entity in background {
            if let Some(object) = view.object(*entity) {
                self.request_background_textures(object);
            }
        }
    }

    pub(super) fn prepare_texture_bindings(
        &mut self,
        device: &Device,
        layout: bool,
        changed: &HashSet<TextureId>,
        changed_regions: &[Region],
    ) {
        let epoch = self.epoch;
        let (draw_list, textures, pending) = (&self.draw_list, &mut self.textures, &mut self.pending_textures);
        if layout {
            pending.clear();
            textures.retain_terrain_bindings(draw_list.terrain_regions());
            for demand in draw_list.texture_demands() {
                update_pending(pending, demand.key, textures.resident(device, demand.key));
            }
            for demand in draw_list.terrain_demands() {
                textures.prepare_terrain_binding(device, demand.region, &demand.textures, epoch);
                for handle in demand.textures.iter().flatten() {
                    update_pending_id(pending, handle.id(), textures.is_resident(handle.id()));
                }
            }
            return;
        }
        if changed.is_empty() && changed_regions.is_empty() {
            return;
        }
        for demand in draw_list.texture_demands().iter().filter(|demand| changed.contains(&demand.key.id)) {
            update_pending(pending, demand.key, textures.resident(device, demand.key));
        }
        for demand in draw_list.terrain_demands().iter().filter(|demand| {
            changed_regions.contains(&demand.region) || demand.textures.iter().flatten().any(|handle| changed.contains(&handle.id()))
        }) {
            textures.prepare_terrain_binding(device, demand.region, &demand.textures, epoch);
            for handle in demand.textures.iter().flatten() {
                update_pending_id(pending, handle.id(), textures.is_resident(handle.id()));
            }
        }
    }

    fn request_background_textures(&mut self, object: &Object) {
        let Some(faces) = object.materials() else { return };
        for face in faces {
            self.request_material_texture(face, Priority::Background);
        }
    }

    fn request_material_texture(&mut self, face: &FaceMaterial, priority: Priority) {
        let Some(texture) = face.base_color_texture.as_ref().and_then(|binding| binding.image()) else { return };
        if FaceParams::new(Some(face)).texture.is_none() {
            return;
        }
        if priority == Priority::Background && !texture.info().is_some_and(|info| info.size.into_iter().all(|edge| edge <= 32)) {
            self.requested_textures.insert(texture.id());
            return;
        }
        self.request_material_texture_id(texture, priority);
    }

    fn request_material_texture_id(&mut self, texture: &TextureHandle, priority: Priority) {
        if self.requested_textures.insert(texture.id()) {
            self.textures.request(texture, self.epoch, priority);
        }
    }
}

fn update_pending(pending: &mut HashSet<TextureId>, key: TextureKey, resident: Option<TextureKey>) {
    update_pending_id(pending, key.id, resident.is_some());
}

fn update_pending_id(pending: &mut HashSet<TextureId>, id: TextureId, resident: bool) {
    if resident {
        pending.remove(&id);
    } else {
        pending.insert(id);
    }
}
