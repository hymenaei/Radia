mod impls;
mod policy;

pub(super) use policy::Origin;
pub(crate) use policy::{Facet, FacetChanges, FacetContext, FacetMerge, FacetSet, FacetSupport, FacetUpdates};
