use std::sync::{Arc, OnceLock};

use hecs::{EntityBuilder, EntityRef, Ref};

use super::impls::{apply_facet, check_support, reject_untrusted};
use crate::history::WorldChangeClasses;
use crate::object::components::{Fenced, ObjectProperties, RegionalStamp, newer_region_field};
use crate::update::{FieldStamp, Stamp};
use crate::{
    Appearance, AvatarAppearance, FieldGroup, FieldRejectionReason, Kind, ObjectMedia, PbrMaterialAssignments, PbrMaterialOverrides,
    Rejections, Shape, SitTarget, SurfaceAppearance, UpdateAuthority, UpdateSource,
};

#[derive(Clone, Copy)]
pub(in crate::mutation) struct Origin {
    pub(in crate::mutation) source: UpdateSource,
    pub(in crate::mutation) stamp: Stamp,
}

impl Origin {
    pub(in crate::mutation) fn field(self) -> FieldStamp {
        FieldStamp { authority: self.source.authority(), stamp: self.stamp }
    }

    pub(in crate::mutation) fn regional(self) -> RegionalStamp {
        RegionalStamp { region: self.source.region, stamp: self.field() }
    }

    pub(in crate::mutation) fn newer(self, fence: Option<FieldStamp>) -> bool {
        crate::object::components::newer_field(self.field(), fence)
    }

    pub(in crate::mutation) fn newer_regional(self, fence: Option<RegionalStamp>) -> bool {
        newer_region_field(self.source.region, self.field(), fence)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FacetContext {
    pub(super) authority: UpdateAuthority,
    pub(super) kind: Kind,
    pub(super) appearance: Option<Appearance>,
    pub(super) object_media_bytes: usize,
}

pub(crate) enum FacetSupport {
    Supported,
    Ignored,
    Rejected(FieldRejectionReason),
}

pub(crate) enum FacetMerge<T> {
    Ignored,
    Applied(Option<T>),
    Rejected(FieldRejectionReason),
}

pub(crate) trait Facet: Clone + PartialEq + Send + Sync + 'static {
    type Update;
    type View: ?Sized;

    const GROUP: FieldGroup;
    const CLASSES: WorldChangeClasses;
    const AVATAR: bool;
    const REGIONAL_FENCE: bool = true;
    const TRACKED_ONLY: bool = false;
    const ORDERS_BINDING: bool = true;
    const OBSERVATION_FRESHNESS: bool = false;
    const RESET_ON_TRANSFER: bool = false;

    fn allowed_kind(kind: Kind) -> bool {
        kind != Kind::Avatar || Self::AVATAR
    }

    fn support(kind: Kind, _appearance: Option<Appearance>, _update: &Self::Update) -> FacetSupport {
        if Self::allowed_kind(kind) { FacetSupport::Supported } else { FacetSupport::Ignored }
    }

    fn read_required(_updates: &FacetUpdates) -> bool {
        false
    }

    fn merge(current: Option<&Self>, update: Self::Update, context: FacetContext) -> FacetMerge<Self>;
    fn read<'a>(facets: &'a FacetRefs<'_>) -> Option<&'a Self::View>;
    fn view(&self) -> &Self::View;
}

macro_rules! define_facets {
    ($( $field:ident: $ty:ty ),+ $(,)?) => {
        #[derive(Clone, Debug, Default, PartialEq)]
        pub(crate) struct FacetSet {
            $(pub(crate) $field: Fenced<$ty>,)+
        }

        #[derive(Default)]
        pub(crate) struct FacetUpdates {
            $(pub(crate) $field: Option<<$ty as Facet>::Update>,)+
        }

        pub(crate) struct FacetRefs<'a> {
            pub(super) entry: EntityRef<'a>,
            pub(super) cache: OnceLock<Box<FacetCache<'a>>>,
        }

        #[derive(Default)]
        pub(super) struct FacetCache<'a> {
            $(pub(super) $field: OnceLock<Option<Ref<'a, Fenced<$ty>>>>,)+
            pub(super) properties: OnceLock<Option<Ref<'a, ObjectProperties>>>,
        }

        impl FacetSet {
            pub(crate) fn read(entry: EntityRef<'_>, updates: &FacetUpdates, reset: bool) -> Self {
                Self {
                    $($field: if reset || updates.$field.is_some() || <$ty as Facet>::read_required(updates) {
                        entry.get::<&Fenced<$ty>>().map(|component| (*component).clone()).unwrap_or_default()
                    } else {
                        Fenced::default()
                    },)+
                }
            }

            pub(crate) fn add_components(&self, builder: &mut EntityBuilder) {
                $(if self.$field.is_stored() { builder.add(self.$field.clone()); })+
            }

            pub(crate) fn add_new(&self, previous: &Self, builder: &mut EntityBuilder) -> bool {
                let mut added = false;
                $(if !previous.$field.is_stored() && self.$field.is_stored() {
                    builder.add(self.$field.clone());
                    added = true;
                })+
                added
            }

            pub(crate) fn update_existing(&self, previous: &Self, ecs: &mut hecs::World, entity: hecs::Entity) {
                $(if self.$field != previous.$field && previous.$field.is_stored() && self.$field.is_stored() {
                    *ecs.query_one_mut::<&mut Fenced<$ty>>(entity).expect("changed facet component is present") = self.$field.clone();
                })+
            }

            pub(crate) fn remove_cleared(&self, previous: &Self, ecs: &mut hecs::World, entity: hecs::Entity) {
                $(if self.$field != previous.$field && previous.$field.is_stored() && !self.$field.is_stored() {
                    ecs.remove_one::<Fenced<$ty>>(entity).expect("changed facet component is present");
                })+
            }

            pub(crate) fn changed_classes(&self, previous: &Self) -> WorldChangeClasses {
                let mut classes = WorldChangeClasses::default();
                $(if self.$field.value != previous.$field.value { classes |= <$ty as Facet>::CLASSES; })+
                classes
            }

            pub(crate) fn reset_region_relative(&mut self) {
                $(
                    if <$ty as Facet>::RESET_ON_TRANSFER {
                        self.$field = Fenced::default();
                    } else if <$ty as Facet>::REGIONAL_FENCE {
                        self.$field.fence = None;
                    }
                )+
            }

            #[cfg(feature = "invariants")]
            pub(crate) fn components_valid(entry: EntityRef<'_>) -> bool {
                true $(&& entry.get::<&Fenced<$ty>>().map_or(true, |component| component.is_stored()))+
            }
        }

        impl<'a> FacetRefs<'a> {
            pub(crate) fn read(entry: EntityRef<'a>) -> Self {
                Self { entry, cache: OnceLock::new() }
            }

            pub(crate) fn get<T: Facet>(&self) -> Option<&T::View> {
                T::read(self)
            }

            pub(super) fn cached(&self) -> &FacetCache<'a> {
                self.cache.get_or_init(|| Box::default())
            }

            pub(crate) fn properties(&self) -> ObjectProperties {
                self.cached()
                    .properties
                    .get_or_init(|| self.entry.get::<&ObjectProperties>())
                    .as_deref()
                    .copied()
                    .unwrap_or_default()
            }
        }

        impl FacetUpdates {
            pub(crate) fn is_empty(&self) -> bool {
                true $(&& self.$field.is_none())+
            }

            pub(crate) fn orders_binding(&self) -> bool {
                false $(|| (self.$field.is_some() && <$ty as Facet>::ORDERS_BINDING))+
            }

            pub(in crate::mutation) fn has_newer(&self, facets: &FacetSet, kind: Kind, origin: Origin) -> bool {
                false $(|| (<$ty as Facet>::OBSERVATION_FRESHNESS
                    && <$ty as Facet>::allowed_kind(kind)
                    && self.$field.is_some()
                    && origin.newer_regional(facets.$field.fence)))+
            }

            pub(in crate::mutation) fn next_appearance(&self, facets: &FacetSet, origin: Origin) -> Option<Appearance> {
                let Some(update) = self.appearance else { return facets.appearance.value };
                if !origin.newer_regional(facets.appearance.fence) {
                    return facets.appearance.value;
                }
                match <Appearance as Facet>::merge(
                    facets.appearance.value.as_ref(),
                    update,
                    FacetContext { authority: origin.source.authority(), kind: Kind::Object, appearance: None, object_media_bytes: 0 },
                ) {
                    FacetMerge::Applied(value) => value,
                    FacetMerge::Ignored | FacetMerge::Rejected(_) => facets.appearance.value,
                }
            }

            pub(crate) fn reject_unsupported(&mut self, kind: Kind, appearance: Option<Appearance>, rejections: &mut Rejections) {
                $(check_support::<$ty>(&mut self.$field, kind, appearance, rejections);)+
            }

            pub(crate) fn reject_untrusted(
                &mut self,
                facets: &FacetSet,
                authority: UpdateAuthority,
                rejections: &mut Rejections,
            ) {
                $(reject_untrusted::<$ty>(&mut self.$field, &facets.$field, authority, rejections);)+
            }

            pub(in crate::mutation) fn apply(
                self,
                facets: &mut FacetSet,
                origin: Origin,
                kind: Kind,
                object_media_bytes: usize,
                rejections: &mut Rejections,
            ) {
                let mut updates = self;
                $(
                    {
                        let context = FacetContext {
                            authority: origin.source.authority(),
                            kind,
                            appearance: facets.appearance.value,
                            object_media_bytes,
                        };
                        apply_facet::<$ty>(&mut facets.$field, updates.$field.take(), origin, context, rejections);
                    }
                )+
            }
        }
    };
}

define_facets! {
    shape: Shape,
    appearance: Appearance,
    surface_appearance: Arc<SurfaceAppearance>,
    pbr_materials: Arc<PbrMaterialAssignments>,
    pbr_material_overrides: Arc<PbrMaterialOverrides>,
    avatar_appearance: Arc<AvatarAppearance>,
    object_media: Arc<ObjectMedia>,
    sit_target: SitTarget,
}

pub(crate) struct FacetChanges {
    pub(crate) before: FacetSet,
    pub(crate) after: FacetSet,
}

impl FacetChanges {
    pub(crate) fn new(before: FacetSet, after: FacetSet) -> Self {
        Self { before, after }
    }

    pub(crate) fn apply(self, ecs: &mut hecs::World, entity: hecs::Entity) {
        if self.before == self.after {
            return;
        }
        let mut builder = EntityBuilder::new();
        self.after.update_existing(&self.before, ecs, entity);
        let has_components = self.after.add_new(&self.before, &mut builder);
        if has_components {
            ecs.insert(entity, builder.build()).expect("facet commit target is live");
        }
        self.after.remove_cleared(&self.before, ecs, entity);
    }

    pub(crate) fn media_bytes(&self) -> Option<(usize, usize)> {
        (self.before.object_media != self.after.object_media).then(|| {
            (
                self.before.object_media.value.as_deref().map_or(0, ObjectMedia::retained_bytes),
                self.after.object_media.value.as_deref().map_or(0, ObjectMedia::retained_bytes),
            )
        })
    }
}
