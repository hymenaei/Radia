use wgpu::{CommandEncoder, Device, Queue, TextureFormat};

use crate::{
    Stats, TextureCompletion, TextureJob,
    geometry::GeometryRenderer,
    scene::{Camera, SceneView},
};

pub struct GeometryPrepare(GeometryRenderer);

impl GeometryPrepare {
    pub fn new(device: &Device, queue: &Queue, format: TextureFormat) -> Self {
        Self(GeometryRenderer::new(device, queue, format))
    }

    pub fn prepare(
        &mut self,
        device: &Device,
        encoder: &mut CommandEncoder,
        view: &SceneView,
        camera: Camera,
        size: [u32; 2],
        water_height: Option<f64>,
    ) {
        self.0.prepare(device, encoder, view, camera, size, water_height);
    }

    pub fn stats(&self) -> Stats {
        self.0.stats
    }

    pub fn next_texture_job(&mut self) -> Option<TextureJob> {
        self.0.next_texture_job()
    }

    pub fn complete_texture(&mut self, completion: TextureCompletion) {
        self.0.complete_texture(completion);
    }
}
