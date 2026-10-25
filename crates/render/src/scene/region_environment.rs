use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    mem::size_of,
    sync::Arc,
};

use assets::{Content, Environment, EnvironmentDescription, Priority, Request, Status};
use world::{EnvironmentAssignment, EnvironmentRead, EnvironmentSource, Region};

use crate::scene::{
    AssetChange, AssetRequest, AssetTicket, Budget, MAX_REGIONS,
    budget::{Lease, MeshWork},
    model::SceneLifetime,
};

const DEFAULT_DAY_LENGTH_SECONDS: u32 = 14_400;
const DEFAULT_DAY_OFFSET_SECONDS: i32 = 57_600;

#[derive(Clone)]
pub struct RegionEnvironment(Arc<ResolvedEnvironment>);

impl RegionEnvironment {
    pub fn description(&self) -> &EnvironmentDescription {
        match &self.0.description {
            ResolvedDescription::Asset(environment) => environment,
            ResolvedDescription::Inline(description) => description,
        }
    }

    pub fn day_length_seconds(&self) -> u32 {
        self.0.day_length_seconds
    }

    pub fn day_offset_seconds(&self) -> i32 {
        self.0.day_offset_seconds
    }

    pub fn altitudes(&self) -> [f32; 3] {
        self.0.altitudes
    }

    pub fn identity(&self) -> u64 {
        self.0.identity
    }

    pub(in crate::scene) fn retained_bytes(&self) -> usize {
        match &self.0.description {
            ResolvedDescription::Asset(_) => size_of::<ResolvedEnvironment>() + 32,
            ResolvedDescription::Inline(description) => inline_bytes(description),
        }
    }

    pub(in crate::scene) fn asset(environment: Environment, candidate: Candidate, identity: u64, budget: &Budget) -> Option<Self> {
        let lease = budget.reserve(size_of::<ResolvedEnvironment>() + 32)?;
        Some(Self(Arc::new(ResolvedEnvironment {
            description: ResolvedDescription::Asset(environment),
            day_length_seconds: candidate.day_length_seconds,
            day_offset_seconds: candidate.day_offset_seconds,
            altitudes: candidate.altitudes,
            identity,
            _lease: lease,
        })))
    }

    pub(in crate::scene) fn inline(
        description: EnvironmentDescription,
        candidate: Candidate,
        identity: u64,
        work: &mut MeshWork,
    ) -> Option<Self> {
        let bytes = inline_bytes(&description);
        let lease = work.lease.split(bytes)?;
        Some(Self(Arc::new(ResolvedEnvironment {
            description: ResolvedDescription::Inline(description),
            day_length_seconds: candidate.day_length_seconds,
            day_offset_seconds: candidate.day_offset_seconds,
            altitudes: candidate.altitudes,
            identity,
            _lease: lease,
        })))
    }

    pub(in crate::scene) fn same_environment(&self, candidate: &ReadyValue, schedule: &Candidate) -> bool {
        self.description() == candidate.description()
            && self.day_length_seconds() == schedule.day_length_seconds
            && self.day_offset_seconds() == schedule.day_offset_seconds
            && self.altitudes() == schedule.altitudes
    }
}

fn inline_bytes(description: &EnvironmentDescription) -> usize {
    description.retained_bytes().saturating_sub(size_of::<EnvironmentDescription>()) + size_of::<ResolvedEnvironment>() + 32
}

struct ResolvedEnvironment {
    description: ResolvedDescription,
    day_length_seconds: u32,
    day_offset_seconds: i32,
    altitudes: [f32; 3],
    identity: u64,
    _lease: Lease,
}

enum ResolvedDescription {
    Asset(Environment),
    Inline(EnvironmentDescription),
}

pub(in crate::scene) struct EnvironmentDecodeJob {
    region: Region,
    generation: u64,
    epoch: u64,
    session: SceneLifetime,
    source: Arc<[u8]>,
    index: usize,
    candidate: Candidate,
    work: MeshWork,
}

impl EnvironmentDecodeJob {
    pub(in crate::scene) fn run(self) -> EnvironmentDecodeCompletion {
        let result = EnvironmentDescription::decode(&self.source);
        EnvironmentDecodeCompletion {
            region: self.region,
            generation: self.generation,
            epoch: self.epoch,
            session: self.session,
            index: self.index,
            candidate: self.candidate,
            result,
            work: self.work,
        }
    }
}

pub(in crate::scene) struct EnvironmentDecodeCompletion {
    region: Region,
    generation: u64,
    epoch: u64,
    session: SceneLifetime,
    index: usize,
    candidate: Candidate,
    result: Result<EnvironmentDescription, assets::Failure>,
    work: MeshWork,
}

pub(in crate::scene) enum EnvironmentStart {
    Asset(AssetChange),
    Inline(EnvironmentDecodeJob),
}

pub(in crate::scene) enum Resolution {
    Unhandled,
    Pending,
    Failed { region: Region, exhausted: bool },
    Ready(ReadyCandidate),
}

pub(in crate::scene) struct ReadyCandidate {
    pub(in crate::scene) region: Region,
    pub(in crate::scene) generation: u64,
    pub(in crate::scene) index: usize,
    pub(in crate::scene) candidate: Candidate,
    pub(in crate::scene) value: ReadyValue,
}

pub(in crate::scene) enum ReadyValue {
    Asset(Environment),
    Inline(EnvironmentDescription, MeshWork),
}

impl ReadyValue {
    pub(in crate::scene) fn description(&self) -> &EnvironmentDescription {
        match self {
            Self::Asset(environment) => environment,
            Self::Inline(description, _) => description,
        }
    }
}

#[derive(Clone)]
pub(in crate::scene) struct Candidate {
    source: CandidateSource,
    pub(in crate::scene) day_length_seconds: u32,
    pub(in crate::scene) day_offset_seconds: i32,
    pub(in crate::scene) altitudes: [f32; 3],
}

#[derive(Clone)]
enum CandidateSource {
    Asset(utils::AssetId),
    Inline(Arc<[u8]>),
}

enum Pending {
    Asset { ticket: AssetTicket, index: usize },
    Inline { index: usize },
}

struct Entry {
    source: EnvironmentRead,
    _lease: Lease,
    candidates: Vec<Candidate>,
    next: usize,
    generation: u64,
    pending: Option<Pending>,
}

pub(in crate::scene) struct RegionEnvironmentResolver {
    budget: Budget,
    entries: BTreeMap<Region, Entry>,
    tickets: HashMap<AssetTicket, (Region, u64, usize)>,
    events: VecDeque<AssetChange>,
    generation: u64,
}

impl RegionEnvironmentResolver {
    pub(in crate::scene) fn new(budget: Budget) -> Self {
        Self { budget, entries: BTreeMap::new(), tickets: HashMap::new(), events: VecDeque::new(), generation: 0 }
    }

    pub(in crate::scene) fn matches(&self, region: Region, source: &EnvironmentRead) -> bool {
        self.entries
            .get(&region)
            .is_some_and(|entry| entry.source.region_assignment == source.region_assignment && entry.source.parcel == source.parcel)
    }

    pub(in crate::scene) fn source_bytes(&self, region: Region) -> usize {
        self.entries.get(&region).map_or(0, |entry| entry_bytes(&entry.source, entry.candidates.capacity()))
    }

    pub(in crate::scene) fn retained_bytes(source: &EnvironmentRead) -> usize {
        let candidates = candidates(source);
        entry_bytes(source, candidates.capacity())
    }

    pub(in crate::scene) fn has_source(source: &EnvironmentRead) -> bool {
        !candidates(source).is_empty()
    }

    pub(in crate::scene) fn observe(&mut self, source: EnvironmentRead, lease: Lease) -> bool {
        let region = source.region;
        let candidates = candidates(&source);
        if candidates.is_empty() {
            return false;
        }
        if self.entries.len() >= MAX_REGIONS && !self.entries.contains_key(&region) {
            return false;
        }
        self.cancel_pending(region);
        self.generation = self.generation.checked_add(1).expect("environment generations exhausted");
        self.entries.insert(region, Entry { source, _lease: lease, candidates, next: 0, generation: self.generation, pending: None });
        true
    }

    pub(in crate::scene) fn remove(&mut self, region: Region) -> bool {
        self.cancel_pending(region);
        self.entries.remove(&region).is_some()
    }

    pub(in crate::scene) fn retire(&mut self, region: Region) {
        self.remove(region);
    }

    pub(in crate::scene) fn reset(&mut self) {
        let regions: Vec<_> = self.entries.keys().copied().collect();
        for region in regions {
            self.cancel_pending(region);
        }
        self.entries.clear();
        self.tickets.clear();
    }

    pub(in crate::scene) fn event(&mut self) -> Option<AssetChange> {
        self.events.pop_front()
    }

    pub(in crate::scene) fn start(&mut self, epoch: u64, session: SceneLifetime) -> Option<EnvironmentStart> {
        let regions: Vec<_> = self.entries.keys().copied().collect();
        for region in regions {
            let Some(entry) = self.entries.get_mut(&region) else { continue };
            if entry.pending.is_some() || entry.next >= entry.candidates.len() {
                continue;
            }
            let index = entry.next;
            let Some(candidate) = entry.candidates.get(index).cloned() else { continue };
            match &candidate.source {
                CandidateSource::Asset(asset) => {
                    let ticket = AssetTicket::new();
                    entry.pending = Some(Pending::Asset { ticket, index });
                    self.tickets.insert(ticket, (region, entry.generation, index));
                    return Some(EnvironmentStart::Asset(AssetChange::Request(AssetRequest {
                        ticket,
                        region,
                        request: Request::Environment(*asset),
                        priority: Priority::Visible,
                    })));
                }
                CandidateSource::Inline(source) => {
                    let Some(work) = self.budget.mesh_work() else { continue };
                    entry.pending = Some(Pending::Inline { index });
                    return Some(EnvironmentStart::Inline(EnvironmentDecodeJob {
                        region,
                        generation: entry.generation,
                        epoch,
                        session,
                        source: Arc::clone(source),
                        index,
                        candidate,
                        work,
                    }));
                }
            }
        }
        None
    }

    pub(in crate::scene) fn waiting(&self) -> bool {
        self.entries.values().any(|entry| entry.pending.is_none() && entry.next < entry.candidates.len())
    }

    pub(in crate::scene) fn complete_asset(&mut self, ticket: AssetTicket, status: &Status) -> Resolution {
        let Some((region, generation, index)) = self.tickets.get(&ticket).copied() else {
            return Resolution::Unhandled;
        };
        let result = match status {
            Status::Ready(Content::Environment(environment)) => Some(Ok(environment.clone())),
            Status::Failed { .. } | Status::StaleSession | Status::Ready(_) => Some(Err(())),
            _ => None,
        };
        let Some(result) = result else { return Resolution::Pending };
        self.tickets.remove(&ticket);
        self.events.push_back(AssetChange::Release(ticket));
        let Some(entry) = self.entries.get_mut(&region) else { return Resolution::Unhandled };
        if entry.generation != generation
            || !matches!(entry.pending, Some(Pending::Asset { ticket: pending, index: pending_index }) if pending == ticket && pending_index == index)
        {
            return Resolution::Pending;
        }
        entry.pending = None;
        match result {
            Ok(environment) => {
                entry.next = entry.candidates.len();
                let Some(candidate) = entry.candidates.get(index).cloned() else {
                    return Resolution::Failed { region, exhausted: true };
                };
                Resolution::Ready(ReadyCandidate { region, generation, index, candidate, value: ReadyValue::Asset(environment) })
            }
            Err(()) => failure(entry, region, index),
        }
    }

    pub(in crate::scene) fn complete_inline(
        &mut self,
        completion: EnvironmentDecodeCompletion,
        epoch: u64,
        session: &SceneLifetime,
    ) -> Resolution {
        if completion.epoch != epoch || !completion.session.is_active() || !completion.session.same(session) {
            return Resolution::Unhandled;
        }
        let Some(entry) = self.entries.get_mut(&completion.region) else { return Resolution::Unhandled };
        if entry.generation != completion.generation
            || !matches!(entry.pending, Some(Pending::Inline { index }) if index == completion.index)
        {
            return Resolution::Unhandled;
        }
        entry.pending = None;
        match completion.result {
            Ok(description) => {
                entry.next = entry.candidates.len();
                Resolution::Ready(ReadyCandidate {
                    region: completion.region,
                    generation: completion.generation,
                    index: completion.index,
                    candidate: completion.candidate,
                    value: ReadyValue::Inline(description, completion.work),
                })
            }
            Err(_) => failure(entry, completion.region, completion.index),
        }
    }

    pub(in crate::scene) fn failed(&mut self, region: Region, generation: u64, index: usize) -> Resolution {
        let Some(entry) = self.entries.get_mut(&region) else { return Resolution::Unhandled };
        if entry.generation != generation || entry.pending.is_some() {
            return Resolution::Unhandled;
        }
        failure(entry, region, index)
    }

    fn cancel_pending(&mut self, region: Region) {
        if let Some(entry) = self.entries.get_mut(&region)
            && let Some(Pending::Asset { ticket, .. }) = entry.pending.take()
        {
            self.tickets.remove(&ticket);
            self.events.push_back(AssetChange::Release(ticket));
        }
    }
}

fn failure(entry: &mut Entry, region: Region, index: usize) -> Resolution {
    entry.pending = None;
    entry.next = index.saturating_add(1);
    Resolution::Failed { region, exhausted: entry.next >= entry.candidates.len() }
}

fn candidates(source: &EnvironmentRead) -> Vec<Candidate> {
    let region = source.region_assignment.as_ref();
    let mut result = Vec::with_capacity(2);
    if let Some(parcel) = source.parcel.as_ref().and_then(|parcel| parcel.assignment.as_ref())
        && !matches!(parcel.source, EnvironmentSource::Inherit)
        && let Some(candidate) = candidate(parcel, region)
    {
        result.push(candidate);
    }
    if let Some(region) = region.filter(|assignment| !matches!(assignment.source, EnvironmentSource::Inherit))
        && let Some(candidate) = candidate(region, Some(region))
    {
        result.push(candidate);
    }
    result
}

fn candidate(assignment: &EnvironmentAssignment, region: Option<&EnvironmentAssignment>) -> Option<Candidate> {
    let source = match &assignment.source {
        EnvironmentSource::Asset(asset) => CandidateSource::Asset(*asset),
        EnvironmentSource::Inline(bytes) => CandidateSource::Inline(Arc::clone(bytes)),
        EnvironmentSource::Inherit => return None,
    };
    Some(Candidate {
        source,
        day_length_seconds: assignment
            .day_length_seconds
            .or_else(|| region.and_then(|value| value.day_length_seconds))
            .unwrap_or(DEFAULT_DAY_LENGTH_SECONDS),
        day_offset_seconds: assignment
            .day_offset_seconds
            .or_else(|| region.and_then(|value| value.day_offset_seconds))
            .unwrap_or(DEFAULT_DAY_OFFSET_SECONDS),
        altitudes: region.and_then(|value| value.altitudes).unwrap_or([0.0; 3]),
    })
}

fn entry_bytes(source: &EnvironmentRead, candidates: usize) -> usize {
    source.retained_bytes().saturating_sub(size_of::<EnvironmentRead>()) + size_of::<Entry>() + 32 + candidates * size_of::<Candidate>()
}
