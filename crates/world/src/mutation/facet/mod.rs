mod impls;
mod policy;

pub(super) use policy::Origin;
#[cfg(test)]
pub(crate) use policy::components_valid;
pub(crate) use policy::{Facet, FacetChanges, FacetContext, FacetMerge, FacetSet, FacetSupport, FacetUpdates};
