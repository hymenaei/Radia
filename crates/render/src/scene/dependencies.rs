use std::collections::{
    BTreeMap,
    Bound::{Excluded, Unbounded},
    HashMap, HashSet, VecDeque,
};

use world::{Entity, LocalAddress, Parent};

#[derive(Default)]
pub(in crate::scene) struct Dependencies {
    edges: HashMap<Entity, (u64, Parent)>,
    resolved: HashMap<Entity, BTreeMap<u64, Entity>>,
    pending: HashMap<LocalAddress, BTreeMap<u64, Entity>>,
    unknown: usize,
    ticket: u64,
}

impl Dependencies {
    pub(in crate::scene) fn replace(&mut self, entity: Entity, parent: Parent) {
        self.remove(entity);
        self.ticket = self.ticket.checked_add(1).expect("dependency ticket space exhausted");
        let ticket = self.ticket;
        self.edges.insert(entity, (ticket, parent));
        match parent {
            Parent::Resolved(parent) => {
                self.resolved.entry(parent).or_default().insert(ticket, entity);
            }
            Parent::Pending { address, .. } => {
                self.pending.entry(address).or_default().insert(ticket, entity);
            }
            Parent::Unknown => {
                self.unknown += 1;
            }
            Parent::Root => {}
        }
    }

    pub(in crate::scene) fn remove(&mut self, entity: Entity) {
        match self.edges.remove(&entity) {
            Some((ticket, Parent::Resolved(parent))) => {
                if let Some(children) = self.resolved.get_mut(&parent) {
                    children.remove(&ticket);
                    if children.is_empty() {
                        self.resolved.remove(&parent);
                    }
                }
            }
            Some((ticket, Parent::Pending { address, .. })) => {
                if let Some(children) = self.pending.get_mut(&address) {
                    children.remove(&ticket);
                    if children.is_empty() {
                        self.pending.remove(&address);
                    }
                }
            }
            Some((_, Parent::Unknown)) => {
                self.unknown -= 1;
            }
            _ => {}
        }
    }

    fn next_pending(&self, address: LocalAddress, after: u64) -> Option<(u64, Entity)> {
        self.pending.get(&address)?.range((Excluded(after), Unbounded)).next().map(|(ticket, entity)| (*ticket, *entity))
    }

    pub(in crate::scene) fn is_complete(&self) -> bool {
        self.unknown == 0
    }

    fn next_child(&self, parent: Entity, after: u64) -> Option<(u64, Entity)> {
        self.resolved.get(&parent)?.range((Excluded(after), Unbounded)).next().map(|(ticket, entity)| (*ticket, *entity))
    }
}

#[derive(Default)]
pub(in crate::scene) struct DependencyTraversal {
    queue: VecDeque<(Dependency, Option<u64>)>,
    seen: HashSet<Dependency>,
}

impl DependencyTraversal {
    pub(in crate::scene) fn enqueue_entity(&mut self, entity: Entity) {
        self.insert(Dependency::Entity(entity));
    }

    pub(in crate::scene) fn enqueue_address(&mut self, address: LocalAddress) {
        self.insert(Dependency::Address(address));
    }

    fn insert(&mut self, dependency: Dependency) {
        if self.seen.insert(dependency) {
            self.queue.push_back((dependency, None));
        }
    }

    pub(in crate::scene) fn step(&mut self, dependencies: &Dependencies) -> Option<Entity> {
        let (dependency, after) = *self.queue.front()?;
        let Some(after) = after else {
            self.queue.front_mut().expect("queued root").1 = Some(0);
            return match dependency {
                Dependency::Entity(entity) => Some(entity),
                Dependency::Address(_) => None,
            };
        };
        let next = match dependency {
            Dependency::Entity(entity) => dependencies.next_child(entity, after),
            Dependency::Address(address) => dependencies.next_pending(address, after),
        };
        if let Some((ticket, child)) = next {
            self.queue.front_mut().expect("queued parent").1 = Some(ticket);
            self.enqueue_entity(child);
        } else {
            self.queue.pop_front();
        }
        None
    }

    pub(in crate::scene) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    pub(in crate::scene) fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Dependency {
    Entity(Entity),
    Address(LocalAddress),
}
