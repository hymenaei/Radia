use std::{
    collections::{BTreeMap, VecDeque},
    mem::size_of,
    sync::Arc,
    time::Duration,
};

use assets::{Content, Environment, EnvironmentSettings, Failure, Priority, Request, Status};
use utils::{AssetId, ExperienceId};
use world::Region;

use crate::scene::{AssetChange, AssetRequest, AssetTicket, Budget, budget::Lease};

const MAX_PENDING: usize = 64;
const MAX_PROPERTIES: usize = 56;
const MAX_PROPERTY_BYTES: usize = 2048;

#[derive(Clone)]
pub struct EnvironmentProperty {
    pub name: String,
    pub source: Arc<[u8]>,
}

pub enum EnvironmentPush {
    Clear { experience: Option<ExperienceId>, transition: Duration },
    Full { experience: ExperienceId, asset: AssetId, transition: Duration },
    Partial { experience: ExperienceId, sky: Vec<EnvironmentProperty>, water: Vec<EnvironmentProperty>, transition: Duration },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentPushError {
    Invalid,
    Capacity,
}

#[derive(Clone)]
pub struct ExperienceSetting(Arc<Setting>);

impl ExperienceSetting {
    pub fn experience(&self) -> ExperienceId {
        self.0.experience
    }
    pub fn environment(&self) -> &Environment {
        &self.0.environment
    }
    pub fn transition(&self) -> Duration {
        self.0.transition
    }
}

struct Setting {
    experience: ExperienceId,
    environment: Environment,
    transition: Duration,
    _lease: Lease,
}

#[derive(Clone)]
pub struct ExperienceProperty(Arc<Property>);

impl ExperienceProperty {
    pub fn experience(&self) -> ExperienceId {
        self.0.experience
    }
    pub fn source(&self) -> &[u8] {
        &self.0.source
    }
    pub fn transition(&self) -> Duration {
        self.0.transition
    }
}

struct Property {
    experience: ExperienceId,
    source: Arc<[u8]>,
    transition: Duration,
    _lease: Lease,
}

#[derive(Clone, Default)]
struct State {
    day: Option<ExperienceSetting>,
    sky: Option<ExperienceSetting>,
    water: Option<ExperienceSetting>,
    sky_properties: BTreeMap<String, ExperienceProperty>,
    water_properties: BTreeMap<String, ExperienceProperty>,
    clear_transition: Duration,
}

#[derive(Clone)]
pub struct ViewerEnvironment(Arc<Snapshot>);

impl ViewerEnvironment {
    pub fn sky(&self) -> Option<&ExperienceSetting> {
        self.0.state.sky.as_ref().or(self.0.state.day.as_ref())
    }
    pub fn water(&self) -> Option<&ExperienceSetting> {
        self.0.state.water.as_ref().or(self.0.state.day.as_ref())
    }
    pub fn sky_properties(&self) -> &BTreeMap<String, ExperienceProperty> {
        &self.0.state.sky_properties
    }
    pub fn water_properties(&self) -> &BTreeMap<String, ExperienceProperty> {
        &self.0.state.water_properties
    }
    pub fn clear_transition(&self) -> Duration {
        self.0.state.clear_transition
    }
}

struct Snapshot {
    state: State,
    _lease: Lease,
}

struct Pending {
    ticket: AssetTicket,
    experience: ExperienceId,
    transition: Duration,
    result: Option<Result<Environment, Failure>>,
    _lease: Lease,
}

pub(in crate::scene) struct ViewerEnvironments {
    budget: Budget,
    current: Option<ViewerEnvironment>,
    pending: VecDeque<Pending>,
    events: VecDeque<AssetChange>,
    version: u64,
}

impl ViewerEnvironments {
    pub(in crate::scene) fn new(budget: Budget) -> Self {
        Self { budget, current: None, pending: VecDeque::new(), events: VecDeque::new(), version: 0 }
    }

    pub(in crate::scene) fn current(&self) -> Option<ViewerEnvironment> {
        self.current.clone()
    }
    pub(in crate::scene) fn event(&mut self) -> Option<AssetChange> {
        self.events.pop_front()
    }
    pub(in crate::scene) fn version(&self) -> u64 {
        self.version
    }
    pub(in crate::scene) fn waiting(&self) -> bool {
        self.pending.front().is_some_and(|pending| pending.result.is_some())
    }

    pub(in crate::scene) fn push(&mut self, region: Region, push: EnvironmentPush) -> Result<(), EnvironmentPushError> {
        match push {
            EnvironmentPush::Full { experience, asset, transition } => {
                if asset.uuid().is_nil() {
                    return Err(EnvironmentPushError::Invalid);
                }
                if self.pending.len() >= MAX_PENDING || self.events.len() >= 2 * MAX_PENDING {
                    return Err(EnvironmentPushError::Capacity);
                }
                let lease = self.budget.reserve(size_of::<Pending>() + 64).ok_or(EnvironmentPushError::Capacity)?;
                let ticket = AssetTicket::new();
                self.pending.push_back(Pending { ticket, experience, transition, result: None, _lease: lease });
                self.events.push_back(AssetChange::Request(AssetRequest {
                    ticket,
                    region,
                    request: Request::Environment(asset),
                    priority: Priority::Visible,
                }));
            }
            EnvironmentPush::Clear { experience, transition } => {
                let mut state = self.state();
                if state.day.as_ref().is_some_and(|setting| experience.is_none_or(|id| id == setting.experience())) {
                    state.day = None;
                }
                if state.sky.as_ref().is_some_and(|setting| experience.is_none_or(|id| id == setting.experience())) {
                    state.sky = None;
                }
                if state.water.as_ref().is_some_and(|setting| experience.is_none_or(|id| id == setting.experience())) {
                    state.water = None;
                }
                state.sky_properties.retain(|_, property| experience.is_some_and(|id| id != property.experience()));
                state.water_properties.retain(|_, property| experience.is_some_and(|id| id != property.experience()));
                state.clear_transition = transition;
                self.store(state)?;
                self.pending.retain(|pending| {
                    if experience.is_none_or(|id| id == pending.experience) {
                        self.events.push_back(AssetChange::Release(pending.ticket));
                        false
                    } else {
                        true
                    }
                });
                self.advance();
            }
            EnvironmentPush::Partial { experience, sky, water, transition } => {
                if sky.is_empty() && water.is_empty() {
                    return Ok(());
                }
                let mut state = self.state();
                self.properties(&mut state.sky_properties, sky, experience, transition)?;
                self.properties(&mut state.water_properties, water, experience, transition)?;
                self.store(state)?;
            }
        }
        Ok(())
    }

    pub(in crate::scene) fn complete(&mut self, ticket: AssetTicket, status: &Status) -> bool {
        let Some(pending) = self.pending.iter_mut().find(|pending| pending.ticket == ticket) else { return false };
        pending.result = match status {
            Status::Ready(Content::Environment(environment)) => Some(Ok(environment.clone())),
            Status::Failed { failure, .. } => Some(Err(*failure)),
            Status::StaleSession => Some(Err(Failure::SessionClosed)),
            Status::Ready(_) => Some(Err(Failure::Malformed)),
            _ => None,
        };
        self.advance();
        true
    }

    pub(in crate::scene) fn advance(&mut self) {
        while let Some(pending) = self.pending.front().filter(|pending| pending.result.is_some()) {
            if let Some(Ok(environment)) = &pending.result {
                let mut state = self.state();
                let Some(lease) = self.budget.reserve(size_of::<Setting>() + 32) else { break };
                let setting = ExperienceSetting(Arc::new(Setting {
                    experience: pending.experience,
                    environment: environment.clone(),
                    transition: pending.transition,
                    _lease: lease,
                }));
                match environment.settings {
                    EnvironmentSettings::Sky(_) => state.sky = Some(setting),
                    EnvironmentSettings::Water(_) => state.water = Some(setting),
                    EnvironmentSettings::DayCycle(_) => {
                        state.day = Some(setting);
                        state.sky = None;
                        state.water = None;
                    }
                }
                if self.store(state).is_err() {
                    break;
                }
            }
            let pending = self.pending.pop_front().expect("completed environment push");
            self.events.push_back(AssetChange::Release(pending.ticket));
        }
    }

    fn state(&self) -> State {
        self.current.as_ref().map_or_else(State::default, |current| current.0.state.clone())
    }

    fn store(&mut self, state: State) -> Result<(), EnvironmentPushError> {
        let bytes = size_of::<Snapshot>()
            + 32
            + state
                .sky_properties
                .keys()
                .chain(state.water_properties.keys())
                .map(|key| key.capacity() + size_of::<ExperienceProperty>() + 128)
                .sum::<usize>();
        let lease = self.budget.reserve(bytes).ok_or(EnvironmentPushError::Capacity)?;
        self.current = Some(ViewerEnvironment(Arc::new(Snapshot { state, _lease: lease })));
        self.version = self.version.checked_add(1).expect("viewer environment version exhausted");
        Ok(())
    }

    fn properties(
        &self,
        target: &mut BTreeMap<String, ExperienceProperty>,
        properties: Vec<EnvironmentProperty>,
        experience: ExperienceId,
        transition: Duration,
    ) -> Result<(), EnvironmentPushError> {
        if properties.len() > MAX_PROPERTIES {
            return Err(EnvironmentPushError::Capacity);
        }
        for property in properties {
            if property.name.is_empty()
                || property.name.len() > 128
                || property.source.is_empty()
                || property.source.len() > MAX_PROPERTY_BYTES
            {
                return Err(EnvironmentPushError::Invalid);
            }
            if !target.contains_key(&property.name) && target.len() >= MAX_PROPERTIES {
                return Err(EnvironmentPushError::Capacity);
            }
            let bytes = size_of::<Property>() + property.source.len() + 32;
            let lease = self.budget.reserve(bytes).ok_or(EnvironmentPushError::Capacity)?;
            target.insert(
                property.name,
                ExperienceProperty(Arc::new(Property { experience, source: property.source, transition, _lease: lease })),
            );
        }
        Ok(())
    }
}
