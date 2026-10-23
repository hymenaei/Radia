use std::sync::{Arc, OnceLock};

use hecs::{EntityRef, Iter, Ref};
use math::{Quat, Vec3};
use uuid::Uuid;

use crate::mutation::facet::{Facet, FacetRefs};
use crate::{
    Appearance, AttachmentPoint, AvatarAppearance, CoordinateFrame, Entity, Kind, LinearMotion, LocalAddress, LocalTransform, ObjectMedia,
    Parent, PbrMaterialAssignments, PbrMaterialOverrides, PoseError, PoseState, Properties, PropertiesProvenance, Shape, SitTarget,
    SurfaceAppearance, Transform, WorldTransformState,
};
use crate::{ObjectOrdering, ObjectRecord, World};

pub struct Object<'a> {
    pub(crate) entity: Entity,
    entry: EntityRef<'a>,
    record: OnceLock<Ref<'a, ObjectRecord>>,
    facets: OnceLock<FacetRefs<'a>>,
    pub(crate) world: &'a World,
}

impl<'a> Object<'a> {
    #[inline(always)]
    pub(crate) fn new(entry: EntityRef<'a>, world: &'a World) -> Self {
        let entity = Entity(entry.entity(), world.id);
        Self { entity, entry, record: OnceLock::new(), facets: OnceLock::new(), world }
    }

    pub(crate) fn record(&self) -> &ObjectRecord {
        self.record.get_or_init(|| self.entry.get::<&ObjectRecord>().expect("every object has a record"))
    }

    fn facet<T: Facet>(&self) -> Option<&T::View> {
        self.facet_refs().get::<T>()
    }

    fn facet_refs(&self) -> &FacetRefs<'a> {
        self.facets.get_or_init(|| FacetRefs::read(self.entry))
    }

    pub(crate) fn ordering(&self) -> Ref<'a, ObjectOrdering> {
        self.world.store.ordering(self.entity.0)
    }

    #[inline(always)]
    pub const fn entity(&self) -> Entity {
        self.entity
    }

    pub fn id(&self) -> Uuid {
        self.record().id
    }

    pub fn kind(&self) -> Kind {
        self.record().kind
    }

    pub fn transform(&self) -> Transform {
        self.record().state.transform
    }

    pub fn position(&self) -> Option<Vec3> {
        self.record().state.transform.position()
    }

    pub fn rotation(&self) -> Option<Quat> {
        self.record().state.transform.rotation()
    }

    pub fn scale(&self) -> Option<Vec3> {
        self.record().state.transform.scale()
    }

    pub fn local_transform(&self) -> Option<LocalTransform> {
        self.record().state.transform.local_transform()
    }

    pub fn address(&self) -> LocalAddress {
        self.record().state.binding.address
    }

    pub fn parent(&self) -> Parent {
        self.record().state.parent
    }

    pub fn linear_motion(&self) -> LinearMotion {
        self.record().state.linear_motion
    }

    pub fn angular_velocity(&self) -> Option<Vec3> {
        self.record().state.angular_velocity
    }

    pub fn shape(&self) -> Option<Shape> {
        self.facet::<Shape>().copied()
    }

    pub fn appearance(&self) -> Option<Appearance> {
        self.facet::<Appearance>().copied()
    }

    pub fn surface_appearance(&self) -> Option<&SurfaceAppearance> {
        self.facet::<Arc<SurfaceAppearance>>()
    }

    pub fn pbr_materials(&self) -> Option<&PbrMaterialAssignments> {
        self.facet::<Arc<PbrMaterialAssignments>>()
    }

    pub fn pbr_material_overrides(&self) -> Option<&PbrMaterialOverrides> {
        self.facet::<Arc<PbrMaterialOverrides>>()
    }

    pub fn object_media(&self) -> Option<&ObjectMedia> {
        self.facet::<Arc<ObjectMedia>>()
    }

    pub fn avatar_appearance(&self) -> Option<&AvatarAppearance> {
        self.facet::<Arc<AvatarAppearance>>()
    }

    pub fn properties(&self) -> Properties {
        self.facet_refs().properties().properties
    }

    pub fn properties_provenance(&self) -> PropertiesProvenance {
        self.facet_refs().properties().properties_provenance
    }

    pub fn attachment_point(&self) -> Option<AttachmentPoint> {
        self.record().state.attachment_point
    }

    pub fn sit_target(&self) -> Option<SitTarget> {
        self.facet::<SitTarget>().copied()
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

impl Clone for Object<'_> {
    fn clone(&self) -> Self {
        let entry = self.world.store.object(self.entity).expect("object clone target is live");
        Self::new(entry, self.world)
    }
}

pub struct Objects<'a> {
    pub(crate) entries: Iter<'a>,
    pub(crate) world: &'a World,
}

impl<'a> Objects<'a> {
    #[inline(always)]
    pub fn iter(&mut self) -> impl Iterator<Item = Object<'a>> + '_ {
        let world = self.world;
        self.entries.by_ref().map(move |entry| Object::new(entry, world))
    }
}
