use super::{ObjectContent, ObjectOrdering, ObjectRecord, ObjectRecordRef, World};
use crate::{
    Appearance, AttachmentPoint, AvatarAppearance, CoordinateFrame, Entity, GltfMaterialOverrides, Kind, LinearMotion, LocalAddress,
    LocalTransform, ObjectMedia, Parent, PoseError, PoseState, Properties, PropertiesProvenance, RenderMaterials, Shape, SitTarget,
    SurfaceAppearance, Transform, WorldTransformState,
};
use hecs::{QueryBorrow, Ref};
use math::{Quat, Vec3};
use uuid::Uuid;

#[derive(Clone)]
pub struct Object<'a> {
    pub(super) entity: Entity,
    pub(super) record: ObjectRecordRef<'a>,
    pub(super) world: &'a World,
}

pub struct Objects<'a> {
    pub(super) records: QueryBorrow<'a, (hecs::Entity, &'a ObjectRecord, Option<&'a ObjectContent>)>,
    pub(super) world: &'a World,
}

impl Objects<'_> {
    pub fn iter(&mut self) -> impl Iterator<Item = Object<'_>> {
        let world = self.world;
        self.records.iter().map(move |(entity, record, content)| Object {
            entity: Entity(entity, world.id),
            record: ObjectRecordRef::Borrowed { record, content },
            world,
        })
    }
}

impl<'a> Object<'a> {
    pub(super) fn content(&self) -> &ObjectContent {
        self.record.content()
    }

    pub(super) fn ordering(&self) -> Ref<'a, ObjectOrdering> {
        self.world.store.ordering(self.entity.0)
    }

    pub const fn entity(&self) -> Entity {
        self.entity
    }

    pub fn id(&self) -> Uuid {
        self.record.id
    }

    pub fn kind(&self) -> Kind {
        self.record.kind
    }

    pub fn transform(&self) -> Transform {
        self.record.state.transform
    }

    pub fn position(&self) -> Option<Vec3> {
        self.record.state.transform.position()
    }

    pub fn rotation(&self) -> Option<Quat> {
        self.record.state.transform.rotation()
    }

    pub fn scale(&self) -> Option<Vec3> {
        self.record.state.transform.scale()
    }

    pub fn local_transform(&self) -> Option<LocalTransform> {
        self.record.state.transform.local_transform()
    }

    pub fn address(&self) -> LocalAddress {
        self.record.state.binding.address
    }

    pub fn parent(&self) -> Parent {
        self.record.state.parent
    }

    pub fn linear_motion(&self) -> LinearMotion {
        self.record.state.linear_motion
    }

    pub fn angular_velocity(&self) -> Option<Vec3> {
        self.record.state.angular_velocity
    }

    pub fn shape(&self) -> Option<Shape> {
        self.content().shape
    }

    pub fn appearance(&self) -> Option<Appearance> {
        self.content().appearance
    }

    pub fn surface_appearance(&self) -> Option<&SurfaceAppearance> {
        self.content().surface_appearance.as_deref()
    }

    pub fn render_materials(&self) -> Option<&RenderMaterials> {
        self.content().render_materials.as_deref()
    }

    pub fn gltf_material_overrides(&self) -> Option<&GltfMaterialOverrides> {
        self.content().gltf_material_overrides.as_deref()
    }

    pub fn object_media(&self) -> Option<&ObjectMedia> {
        self.content().object_media.as_deref()
    }

    pub fn avatar_appearance(&self) -> Option<&AvatarAppearance> {
        self.content().avatar_appearance.as_deref()
    }

    pub fn properties(&self) -> Properties {
        self.content().properties
    }

    pub fn properties_provenance(&self) -> PropertiesProvenance {
        self.content().properties_provenance
    }

    pub fn attachment_point(&self) -> Option<AttachmentPoint> {
        self.record.state.attachment_point
    }

    pub fn sit_target(&self) -> Option<SitTarget> {
        self.content().sit_target
    }

    pub fn coordinate_frame(&self) -> Result<CoordinateFrame, PoseError> {
        self.world.coordinate_frame(self.entity)
    }

    pub fn world_pose(&self) -> Result<PoseState, PoseError> {
        self.world.resolve_pose(self.entity)
    }

    pub fn world_transform(&self) -> Result<WorldTransformState, PoseError> {
        self.world.resolve_transform(self.entity)
    }
}
