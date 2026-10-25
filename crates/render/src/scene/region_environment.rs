use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    mem::size_of,
    sync::Arc,
    time::{Duration, Instant},
};

use assets::{Content, Environment, EnvironmentDescription, Priority, Request, Status};
use world::{EnvironmentAssignment, EnvironmentRead, EnvironmentSource, Region};

use crate::environment::Schedule;
use crate::scene::{
    AssetChange, AssetRequest, AssetTicket, Budget, MAX_REGIONS,
    budget::{Lease, MeshWork},
    model::SceneLifetime,
};

#[derive(Clone)]
pub struct RegionEnvironment(Arc<ResolvedEnvironment>);

impl RegionEnvironment {
    pub fn description(&self) -> &EnvironmentDescription {
        match &self.0.description {
            ResolvedDescription::Asset(environment) => environment,
            ResolvedDescription::Inline(description) => description,
        }
    }

    pub fn schedule(&self) -> Schedule {
        self.0.schedule
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
            schedule: candidate.schedule,
            identity,
            _lease: lease,
        })))
    }

    pub(in crate::scene) fn inline(
        description: EnvironmentDescription,
        candidate: Candidate,
        identity: u64,
        work: &mut MeshWork,
        budget: &Budget,
    ) -> Option<Self> {
        let bytes = inline_bytes(&description);
        let lease = work.lease.split(bytes).or_else(|| budget.reserve(bytes))?;
        Some(Self(Arc::new(ResolvedEnvironment {
            description: ResolvedDescription::Inline(description),
            schedule: candidate.schedule,
            identity,
            _lease: lease,
        })))
    }

    pub(in crate::scene) fn same_environment(&self, candidate: &ReadyValue, schedule: &Candidate) -> bool {
        self.description() == candidate.description() && self.schedule() == schedule.schedule
    }
}

fn inline_bytes(description: &EnvironmentDescription) -> usize {
    description.retained_bytes().saturating_sub(size_of::<EnvironmentDescription>()) + size_of::<ResolvedEnvironment>() + 32
}

struct ResolvedEnvironment {
    description: ResolvedDescription,
    schedule: Schedule,
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
    schedule: Schedule,
}

#[derive(Clone)]
enum CandidateSource {
    Asset(utils::AssetId),
    Inline(Arc<[u8]>),
}

enum Pending {
    Asset { ticket: AssetTicket, index: usize },
    Inline { index: usize },
    StaleSession,
    Retry { index: usize },
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
    retry_at: Option<Instant>,
}

impl RegionEnvironmentResolver {
    pub(in crate::scene) fn new(budget: Budget) -> Self {
        Self { budget, entries: BTreeMap::new(), tickets: HashMap::new(), events: VecDeque::new(), generation: 0, retry_at: None }
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
        self.retry_at = None;
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

    pub(in crate::scene) fn retry_ready(&mut self, now: Instant) {
        if !self.retry_at.is_some_and(|retry_at| now >= retry_at) {
            return;
        }
        for entry in self.entries.values_mut() {
            if let Some(Pending::Retry { index }) = entry.pending.take() {
                entry.next = index;
            }
        }
        self.retry_at = None;
    }

    pub(in crate::scene) fn retry_at(&mut self, now: Instant) -> Option<Instant> {
        if !self.entries.values().any(|entry| matches!(entry.pending, Some(Pending::Retry { .. }))) {
            self.retry_at = None;
            return None;
        }
        Some(*self.retry_at.get_or_insert(now + Duration::from_millis(200)))
    }

    pub(in crate::scene) fn retry_capacity(&mut self, region: Region, generation: u64, index: usize) {
        let Some(entry) = self.entries.get_mut(&region) else { return };
        if entry.generation == generation && entry.pending.is_none() {
            entry.pending = Some(Pending::Retry { index });
        }
    }

    pub(in crate::scene) fn complete_asset(&mut self, ticket: AssetTicket, status: &Status) -> Resolution {
        let Some((region, generation, index)) = self.tickets.get(&ticket).copied() else {
            return Resolution::Unhandled;
        };
        if matches!(status, Status::StaleSession) {
            self.tickets.remove(&ticket);
            self.events.push_back(AssetChange::Release(ticket));
            if let Some(entry) = self.entries.get_mut(&region)
                && entry.generation == generation
                && matches!(entry.pending, Some(Pending::Asset { ticket: pending, index: pending_index }) if pending == ticket && pending_index == index)
            {
                entry.pending = Some(Pending::StaleSession);
            }
            return Resolution::Pending;
        }
        let result = match status {
            Status::Ready(Content::Environment(environment)) => Some(Ok(environment.clone())),
            Status::Failed { .. } | Status::Ready(_) => Some(Err(())),
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
    let defaults = Schedule::default();
    Some(Candidate {
        source,
        schedule: Schedule {
            day_length: assignment
                .day_length_seconds
                .or_else(|| region.and_then(|value| value.day_length_seconds))
                .map_or(defaults.day_length, f64::from),
            day_offset: assignment
                .day_offset_seconds
                .or_else(|| region.and_then(|value| value.day_offset_seconds))
                .map_or(defaults.day_offset, f64::from),
            altitudes: region.and_then(|value| value.altitudes).unwrap_or(defaults.altitudes),
        },
    })
}

fn entry_bytes(source: &EnvironmentRead, candidates: usize) -> usize {
    source.retained_bytes().saturating_sub(size_of::<EnvironmentRead>()) + size_of::<Entry>() + 32 + candidates * size_of::<Candidate>()
}
