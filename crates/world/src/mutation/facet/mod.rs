mod impls;
#[path = "facet.rs"]
mod policy;

pub(super) use policy::Origin;
pub(crate) use policy::{Facet, FacetChanges, FacetContext, FacetMerge, FacetRefs, FacetSet, FacetSupport, FacetUpdates};
