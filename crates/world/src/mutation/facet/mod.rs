mod impls;
#[path = "facet.rs"]
mod policy;

pub(super) use policy::Origin;
pub(crate) use policy::{Facet, FacetChanges, FacetContext, FacetMerge, FacetSet, FacetSupport, FacetUpdates};
