use std::sync::Arc;

use super::{Facet, FacetContext, FacetMerge, FacetRefs, FacetSupport, FacetUpdates, Origin};
use crate::MAX_OBJECT_MEDIA_BYTES;
use crate::history::WorldChangeClasses;
use crate::object::components::Fenced;
use crate::update::FieldUpdate;
use crate::{
    Appearance, AvatarAppearance, AvatarAppearanceUpdate, FieldGroup, FieldRejectionReason, Kind, ObjectMedia, ObjectMediaUpdate,
    PbrMaterialAssignments, PbrMaterialOverrides, Rejections, Shape, SitTarget, SurfaceAppearance, UpdateAuthority,
};

impl Facet for Shape {
    type Update = FieldUpdate<Shape>;
    type View = Shape;

    const GROUP: FieldGroup = FieldGroup::Shape;
    const CLASSES: WorldChangeClasses = WorldChangeClasses::SHAPE;
    const AVATAR: bool = true;
    const OBSERVATION_FRESHNESS: bool = true;

    fn merge(_current: Option<&Self>, update: Self::Update, _context: FacetContext) -> FacetMerge<Self> {
        merge_field(update)
    }

    fn read<'a>(facets: &'a FacetRefs<'_>) -> Option<&'a Self::View> {
        facets.cached().shape.get_or_init(|| facets.entry.get::<&Fenced<Shape>>()).as_deref()?.value.as_ref().map(Self::view)
    }

    fn view(&self) -> &Self::View {
        self
    }
}

impl Facet for Appearance {
    type Update = FieldUpdate<Appearance>;
    type View = Appearance;

    const GROUP: FieldGroup = FieldGroup::Appearance;
    const CLASSES: WorldChangeClasses = WorldChangeClasses::APPEARANCE;
    const AVATAR: bool = true;
    const OBSERVATION_FRESHNESS: bool = true;

    fn read_required(updates: &FacetUpdates) -> bool {
        updates.surface_appearance.is_some()
            || updates.pbr_materials.is_some()
            || updates.pbr_material_overrides.is_some()
            || updates.object_media.is_some()
    }

    fn merge(_current: Option<&Self>, update: Self::Update, _context: FacetContext) -> FacetMerge<Self> {
        merge_field(update)
    }

    fn read<'a>(facets: &'a FacetRefs<'_>) -> Option<&'a Self::View> {
        facets.cached().appearance.get_or_init(|| facets.entry.get::<&Fenced<Appearance>>()).as_deref()?.value.as_ref().map(Self::view)
    }

    fn view(&self) -> &Self::View {
        self
    }
}

impl Facet for Arc<SurfaceAppearance> {
    type Update = SurfaceAppearance;
    type View = SurfaceAppearance;

    const GROUP: FieldGroup = FieldGroup::TextureEntry;
    const CLASSES: WorldChangeClasses = WorldChangeClasses::APPEARANCE;
    const AVATAR: bool = false;
    const OBSERVATION_FRESHNESS: bool = true;

    fn merge(current: Option<&Self>, update: Self::Update, _context: FacetContext) -> FacetMerge<Self> {
        FacetMerge::Applied(Some(match current {
            Some(current) if current.as_ref() == &update => Arc::clone(current),
            _ => Arc::new(update),
        }))
    }

    fn read<'a>(facets: &'a FacetRefs<'_>) -> Option<&'a Self::View> {
        facets
            .cached()
            .surface_appearance
            .get_or_init(|| facets.entry.get::<&Fenced<Arc<SurfaceAppearance>>>())
            .as_deref()?
            .value
            .as_deref()
    }

    fn view(&self) -> &Self::View {
        self.as_ref()
    }
}

impl Facet for Arc<PbrMaterialAssignments> {
    type Update = PbrMaterialAssignments;
    type View = PbrMaterialAssignments;

    const GROUP: FieldGroup = FieldGroup::PbrMaterials;
    const CLASSES: WorldChangeClasses = WorldChangeClasses::APPEARANCE;
    const AVATAR: bool = false;
    const OBSERVATION_FRESHNESS: bool = true;

    fn merge(current: Option<&Self>, update: Self::Update, _context: FacetContext) -> FacetMerge<Self> {
        FacetMerge::Applied(if update.is_empty() {
            None
        } else {
            Some(match current {
                Some(current) if current.as_ref() == &update => Arc::clone(current),
                _ => Arc::new(update),
            })
        })
    }

    fn read<'a>(facets: &'a FacetRefs<'_>) -> Option<&'a Self::View> {
        facets
            .cached()
            .pbr_materials
            .get_or_init(|| facets.entry.get::<&Fenced<Arc<PbrMaterialAssignments>>>())
            .as_deref()?
            .value
            .as_deref()
    }

    fn view(&self) -> &Self::View {
        self.as_ref()
    }
}

impl Facet for Arc<PbrMaterialOverrides> {
    type Update = PbrMaterialOverrides;
    type View = PbrMaterialOverrides;

    const GROUP: FieldGroup = FieldGroup::PbrMaterialOverrides;
    const CLASSES: WorldChangeClasses = WorldChangeClasses::APPEARANCE;
    const AVATAR: bool = false;

    fn support(kind: Kind, appearance: Option<Appearance>, update: &Self::Update) -> FacetSupport {
        if update.is_empty() || accepts_surface(kind, appearance) {
            FacetSupport::Supported
        } else {
            FacetSupport::Rejected(FieldRejectionReason::InvalidValue { field: "pbr_material_overrides" })
        }
    }

    fn merge(current: Option<&Self>, update: Self::Update, _context: FacetContext) -> FacetMerge<Self> {
        FacetMerge::Applied(if update.is_empty() {
            None
        } else {
            Some(match current {
                Some(current) if current.as_ref() == &update => Arc::clone(current),
                _ => Arc::new(update),
            })
        })
    }

    fn read<'a>(facets: &'a FacetRefs<'_>) -> Option<&'a Self::View> {
        facets
            .cached()
            .pbr_material_overrides
            .get_or_init(|| facets.entry.get::<&Fenced<Arc<PbrMaterialOverrides>>>())
            .as_deref()?
            .value
            .as_deref()
    }

    fn view(&self) -> &Self::View {
        self.as_ref()
    }
}

impl Facet for Arc<AvatarAppearance> {
    type Update = AvatarAppearanceUpdate;
    type View = AvatarAppearance;

    const GROUP: FieldGroup = FieldGroup::AvatarAppearance;
    const CLASSES: WorldChangeClasses = WorldChangeClasses::APPEARANCE;
    const AVATAR: bool = true;

    fn allowed_kind(kind: Kind) -> bool {
        kind == Kind::Avatar
    }

    fn support(kind: Kind, _appearance: Option<Appearance>, _update: &Self::Update) -> FacetSupport {
        if kind == Kind::Avatar {
            FacetSupport::Supported
        } else {
            FacetSupport::Rejected(FieldRejectionReason::InvalidValue { field: "avatar_appearance" })
        }
    }

    fn merge(current: Option<&Self>, update: Self::Update, _context: FacetContext) -> FacetMerge<Self> {
        let mut value = current.map_or_else(AvatarAppearance::default, |current| current.as_ref().clone());
        value.apply(update);
        FacetMerge::Applied(Some(Arc::new(value)))
    }

    fn read<'a>(facets: &'a FacetRefs<'_>) -> Option<&'a Self::View> {
        facets.cached().avatar_appearance.get_or_init(|| facets.entry.get::<&Fenced<Arc<AvatarAppearance>>>()).as_deref()?.value.as_deref()
    }

    fn view(&self) -> &Self::View {
        self.as_ref()
    }
}

impl Facet for Arc<ObjectMedia> {
    type Update = ObjectMediaUpdate;
    type View = ObjectMedia;

    const GROUP: FieldGroup = FieldGroup::ObjectMedia;
    const CLASSES: WorldChangeClasses = WorldChangeClasses::MEDIA;
    const AVATAR: bool = false;
    const REGIONAL_FENCE: bool = false;
    const TRACKED_ONLY: bool = true;
    const ORDERS_BINDING: bool = false;

    fn support(kind: Kind, appearance: Option<Appearance>, _update: &Self::Update) -> FacetSupport {
        if accepts_surface(kind, appearance) {
            FacetSupport::Supported
        } else {
            FacetSupport::Rejected(FieldRejectionReason::InvalidValue { field: "object_media" })
        }
    }

    fn merge(current: Option<&Self>, update: Self::Update, context: FacetContext) -> FacetMerge<Self> {
        if context.authority != UpdateAuthority::Tracked {
            return FacetMerge::Rejected(FieldRejectionReason::UntrustedSource);
        }
        if current.is_some_and(|current| update.version() <= current.version()) {
            return FacetMerge::Ignored;
        }
        let current_bytes = current.map_or(0, |current| current.retained_bytes());
        let mut value = current.map_or_else(ObjectMedia::default, |current| current.as_ref().clone());
        value.apply(&update);
        if context.object_media_bytes.saturating_sub(current_bytes).saturating_add(value.retained_bytes()) > MAX_OBJECT_MEDIA_BYTES {
            FacetMerge::Rejected(FieldRejectionReason::ResourceLimit { field: "object_media" })
        } else {
            FacetMerge::Applied(Some(Arc::new(value)))
        }
    }

    fn read<'a>(facets: &'a FacetRefs<'_>) -> Option<&'a Self::View> {
        facets.cached().object_media.get_or_init(|| facets.entry.get::<&Fenced<Arc<ObjectMedia>>>()).as_deref()?.value.as_deref()
    }

    fn view(&self) -> &Self::View {
        self.as_ref()
    }
}

impl Facet for SitTarget {
    type Update = Option<SitTarget>;
    type View = SitTarget;

    const GROUP: FieldGroup = FieldGroup::SitTarget;
    const CLASSES: WorldChangeClasses = WorldChangeClasses::RELATIONSHIP;
    const AVATAR: bool = true;
    const RESET_ON_TRANSFER: bool = true;

    fn merge(_current: Option<&Self>, update: Self::Update, _context: FacetContext) -> FacetMerge<Self> {
        FacetMerge::Applied(update)
    }

    fn read<'a>(facets: &'a FacetRefs<'_>) -> Option<&'a Self::View> {
        facets.cached().sit_target.get_or_init(|| facets.entry.get::<&Fenced<SitTarget>>()).as_deref()?.value.as_ref().map(Self::view)
    }

    fn view(&self) -> &Self::View {
        self
    }
}

fn merge_field<T>(update: FieldUpdate<T>) -> FacetMerge<T> {
    match update {
        FieldUpdate::Keep => FacetMerge::Ignored,
        FieldUpdate::Set(value) => FacetMerge::Applied(Some(value)),
        FieldUpdate::Clear => FacetMerge::Applied(None),
    }
}

fn accepts_surface(kind: Kind, appearance: Option<Appearance>) -> bool {
    kind == Kind::Object && appearance == Some(Appearance::Surface)
}

pub(super) fn check_support<T: Facet>(
    update: &mut Option<T::Update>,
    kind: Kind,
    appearance: Option<Appearance>,
    rejections: &mut Rejections,
) {
    let Some(value) = update.as_ref() else { return };
    match T::support(kind, appearance, value) {
        FacetSupport::Supported => {}
        FacetSupport::Ignored => *update = None,
        FacetSupport::Rejected(reason) => {
            rejections.reject(T::GROUP, reason);
            *update = None;
        }
    }
}

pub(super) fn reject_untrusted<T: Facet>(
    update: &mut Option<T::Update>,
    current: &Fenced<T>,
    authority: UpdateAuthority,
    rejections: &mut Rejections,
) {
    let present = update.is_some();
    let tracked_fence = current.fence.is_some_and(|fence| fence.stamp.authority == UpdateAuthority::Tracked);
    if present && ((T::REGIONAL_FENCE && tracked_fence) || (T::TRACKED_ONLY && authority == UpdateAuthority::Untracked)) {
        rejections.reject(T::GROUP, FieldRejectionReason::UntrustedSource);
        *update = None;
    }
}

pub(super) fn apply_facet<T: Facet>(
    current: &mut Fenced<T>,
    update: Option<T::Update>,
    origin: Origin,
    context: FacetContext,
    rejections: &mut Rejections,
) {
    let Some(update) = update else { return };
    match T::support(context.kind, context.appearance, &update) {
        FacetSupport::Ignored => return,
        FacetSupport::Rejected(reason) => {
            rejections.reject(T::GROUP, reason);
            return;
        }
        FacetSupport::Supported => {}
    }
    if T::TRACKED_ONLY && context.authority != UpdateAuthority::Tracked {
        rejections.reject(T::GROUP, FieldRejectionReason::UntrustedSource);
        return;
    }
    if T::REGIONAL_FENCE && !origin.newer_regional(current.fence) {
        return;
    }
    match T::merge(current.value.as_ref(), update, context) {
        FacetMerge::Ignored => {}
        FacetMerge::Applied(value) => {
            current.value = value;
            if T::REGIONAL_FENCE {
                current.fence = Some(origin.regional());
            }
        }
        FacetMerge::Rejected(reason) => rejections.reject(T::GROUP, reason),
    }
}
