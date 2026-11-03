use std::{
    collections::{BTreeMap, VecDeque},
    mem::size_of,
    sync::Arc,
    time::{Duration, Instant},
};

use assets::{Content, Environment, EnvironmentDescription, EnvironmentSettings, Failure, Priority, Request, Sky, Status, Water};
use utils::{AssetId, ExperienceId};
use uuid::Uuid;
use world::Region;

use crate::scene::{AssetChange, AssetRequest, AssetTicket, Budget, budget::Lease};

const MAX_PENDING: usize = 64;
const MAX_PROPERTIES: usize = 56;
const MAX_PROPERTY_BYTES: usize = 2048;
const MAX_PROPERTY_CHANGES: usize = MAX_PROPERTIES * 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentPreset {
    Sunrise,
    Midday,
    Sunset,
    Midnight,
}

impl EnvironmentPreset {
    pub const ALL: [Self; 4] = [Self::Sunrise, Self::Midday, Self::Sunset, Self::Midnight];

    pub fn label(self) -> &'static str {
        match self {
            Self::Sunrise => "Sunrise",
            Self::Midday => "Midday",
            Self::Sunset => "Sunset",
            Self::Midnight => "Midnight",
        }
    }

    pub fn asset_id(self) -> Option<AssetId> {
        let uuid = match self {
            Self::Sunrise => Uuid::from_u128(0x01e41537ff512f1f8ef717e4df760bfb),
            Self::Midday => Uuid::from_u128(0xc46226b40e435a569708d27ca1df3292),
            Self::Sunset => Uuid::from_u128(0x084e26cda90028e808d064a9de5c15e2),
            Self::Midnight => Uuid::from_u128(0x8a01b97acb20c1eaac63f7ea84ad0090),
        };
        Some(AssetId::new(uuid))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum EnvironmentChoice {
    #[default]
    Shared,
    Local(LocalChoice),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalChoice {
    Preset(EnvironmentPreset),
    Inventory { owner_id: Uuid, item_id: Uuid, asset_id: AssetId },
}

impl EnvironmentChoice {
    fn local(&self) -> Option<&LocalChoice> {
        match self {
            Self::Shared => None,
            Self::Local(choice) => Some(choice),
        }
    }
}

impl LocalChoice {
    fn asset_id(&self) -> Option<AssetId> {
        match self {
            Self::Preset(preset) => preset.asset_id(),
            Self::Inventory { asset_id, .. } => Some(*asset_id),
        }
    }

    fn accepts(&self, environment: &EnvironmentDescription) -> bool {
        match self {
            Self::Preset(_) => matches!(&environment.settings, EnvironmentSettings::Sky(_)),
            Self::Inventory { .. } => true,
        }
    }
}

static SHARED_ENVIRONMENT: EnvironmentChoice = EnvironmentChoice::Shared;

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
    pub fn region(&self) -> Region {
        self.0.region
    }
    pub(crate) fn sequence(&self) -> u64 {
        self.0.sequence
    }
}

struct Setting {
    region: Region,
    experience: ExperienceId,
    sequence: u64,
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
    pub fn region(&self) -> Region {
        self.0.region
    }
    pub(crate) fn value(&self) -> &PropertyValue {
        &self.0.value
    }
    pub(crate) fn sequence(&self) -> u64 {
        self.0.sequence
    }
}

struct Property {
    region: Region,
    experience: ExperienceId,
    source: Arc<[u8]>,
    transition: Duration,
    sequence: u64,
    value: PropertyValue,
    _lease: Lease,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum PropertyKind {
    Sky,
    Water,
}

#[derive(Clone)]
pub(crate) enum PropertyValue {
    Sky(Arc<Sky>),
    Water(Arc<Water>),
}

#[derive(Clone)]
pub(crate) struct PropertyChange {
    pub(crate) sequence: u64,
    pub(crate) kind: PropertyKind,
    pub(crate) name: String,
    pub(crate) previous: Option<ExperienceProperty>,
    pub(crate) target: Option<ExperienceProperty>,
    pub(crate) started: Instant,
    pub(crate) duration: Duration,
}

#[derive(Clone, Default)]
struct State {
    day: Option<ExperienceSetting>,
    sky: Option<ExperienceSetting>,
    water: Option<ExperienceSetting>,
    sky_properties: BTreeMap<String, ExperienceProperty>,
    water_properties: BTreeMap<String, ExperienceProperty>,
    local: Option<LocalEnvironment>,
    selected: EnvironmentChoice,
    local_error: Option<Failure>,
}

#[derive(Clone)]
pub struct LocalEnvironment {
    setting: Arc<LocalSetting>,
    region: Region,
}

impl LocalEnvironment {
    pub fn choice(&self) -> &LocalChoice {
        self.setting.choice()
    }
    pub fn environment(&self) -> &EnvironmentDescription {
        self.setting.environment()
    }
    pub fn region(&self) -> Region {
        self.region
    }
    fn with_region(&self, region: Region) -> Self {
        Self { setting: Arc::clone(&self.setting), region }
    }
}

enum LocalSetting {
    Sky { choice: LocalChoice, environment: Environment, _lease: Lease },
    Water { choice: LocalChoice, environment: Environment, _lease: Lease },
    DayCycle { choice: LocalChoice, environment: Environment, _lease: Lease },
}

impl LocalSetting {
    fn new(choice: LocalChoice, environment: Environment, lease: Lease) -> Self {
        match &environment.settings {
            EnvironmentSettings::Sky(_) => Self::Sky { choice, environment, _lease: lease },
            EnvironmentSettings::Water(_) => Self::Water { choice, environment, _lease: lease },
            EnvironmentSettings::DayCycle(_) => Self::DayCycle { choice, environment, _lease: lease },
        }
    }

    fn choice(&self) -> &LocalChoice {
        match self {
            Self::Sky { choice, .. } | Self::Water { choice, .. } | Self::DayCycle { choice, .. } => choice,
        }
    }

    fn environment(&self) -> &EnvironmentDescription {
        match self {
            Self::Sky { environment, .. } | Self::Water { environment, .. } | Self::DayCycle { environment, .. } => environment,
        }
    }
}

#[derive(Clone)]
pub struct ViewerEnvironment(Arc<Snapshot>);

impl ViewerEnvironment {
    pub fn day(&self) -> Option<&ExperienceSetting> {
        self.0.state.day.as_ref()
    }
    pub fn sky(&self) -> Option<&ExperienceSetting> {
        self.0.state.sky.as_ref().or(self.0.state.day.as_ref())
    }
    pub fn water(&self) -> Option<&ExperienceSetting> {
        self.0.state.water.as_ref().or(self.0.state.day.as_ref())
    }
    pub fn local(&self) -> Option<&LocalEnvironment> {
        self.0.state.local.as_ref()
    }
    pub fn selected(&self) -> &EnvironmentChoice {
        &self.0.state.selected
    }
    pub fn local_error(&self) -> Option<Failure> {
        self.0.state.local_error
    }
    pub fn sky_properties(&self) -> &BTreeMap<String, ExperienceProperty> {
        &self.0.state.sky_properties
    }
    pub fn water_properties(&self) -> &BTreeMap<String, ExperienceProperty> {
        &self.0.state.water_properties
    }
    pub fn sky_overrides(&self) -> Option<&Sky> {
        self.0.sky_overrides.as_ref()
    }
    pub fn water_overrides(&self) -> Option<&Water> {
        self.0.water_overrides.as_ref()
    }
    pub(crate) fn sky_overrides_for_region(&self, region: Region) -> Option<&Sky> {
        self.0.sky_overrides_by_region.get(&region)
    }
    pub(crate) fn water_overrides_for_region(&self, region: Region) -> Option<&Water> {
        self.0.water_overrides_by_region.get(&region)
    }
    pub(crate) fn property_changes(&self) -> &VecDeque<PropertyChange> {
        &self.0.property_changes
    }
    pub fn clear_transition(&self) -> Duration {
        self.0.clear_transition.map_or(Duration::ZERO, |(_, duration)| duration)
    }
    pub(crate) fn clear_transition_for_region(&self, region: Region) -> Option<Duration> {
        self.0.clear_transition.filter(|(affected, _)| *affected == region).map(|(_, duration)| duration)
    }
    pub fn version(&self) -> u64 {
        self.0.version
    }
    pub fn transition(&self) -> Option<Duration> {
        let sky = self.sky().map(ExperienceSetting::transition);
        let water = self.water().map(ExperienceSetting::transition);
        self.day()
            .map(ExperienceSetting::transition)
            .into_iter()
            .chain(sky)
            .chain(water)
            .max()
            .or(self.0.clear_transition.map(|(_, duration)| duration))
    }
    pub fn transition_for_region(&self, region: Region) -> Option<Duration> {
        let transitions = self
            .day()
            .filter(|setting| setting.region() == region)
            .map(ExperienceSetting::transition)
            .into_iter()
            .chain(self.sky().filter(|setting| setting.region() == region).map(ExperienceSetting::transition))
            .chain(self.water().filter(|setting| setting.region() == region).map(ExperienceSetting::transition));
        transitions.max()
    }
}

struct Snapshot {
    state: State,
    sky_overrides: Option<Sky>,
    water_overrides: Option<Water>,
    sky_overrides_by_region: BTreeMap<Region, Sky>,
    water_overrides_by_region: BTreeMap<Region, Water>,
    property_changes: VecDeque<PropertyChange>,
    clear_transition: Option<(Region, Duration)>,
    version: u64,
    _lease: Lease,
}

struct Pending {
    ticket: AssetTicket,
    region: Region,
    experience: ExperienceId,
    transition: Duration,
    result: Option<Result<Environment, Failure>>,
    _lease: Lease,
}

struct LocalPending {
    ticket: AssetTicket,
    region: Region,
    choice: LocalChoice,
    result: Option<Result<Environment, Failure>>,
    _lease: Lease,
}

pub(in crate::scene) struct ViewerEnvironments {
    budget: Budget,
    current: Option<ViewerEnvironment>,
    pending: VecDeque<Pending>,
    local_pending: Option<LocalPending>,
    local_region: Option<Region>,
    events: VecDeque<AssetChange>,
    version: u64,
    setting_sequence: u64,
    property_sequence: u64,
}

impl ViewerEnvironments {
    pub(in crate::scene) fn new(budget: Budget) -> Self {
        Self {
            budget,
            current: None,
            pending: VecDeque::new(),
            local_pending: None,
            local_region: None,
            events: VecDeque::new(),
            version: 0,
            setting_sequence: 0,
            property_sequence: 0,
        }
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
            || self.local_pending.as_ref().is_some_and(|pending| pending.result.is_some())
            || self.local_pending.is_none()
                && self.selected().local().is_some_and(|choice| {
                    self.current.as_ref().and_then(ViewerEnvironment::local).is_none_or(|local| local.choice() != choice)
                })
    }

    pub(in crate::scene) fn selected(&self) -> &EnvironmentChoice {
        self.current.as_ref().map_or(&SHARED_ENVIRONMENT, ViewerEnvironment::selected)
    }

    pub(in crate::scene) fn set_local(&mut self, choice: EnvironmentChoice, region: Region) -> Result<(), EnvironmentPushError> {
        let local_choice = choice.local();
        if matches!(local_choice, Some(LocalChoice::Inventory { owner_id, item_id, asset_id }) if owner_id.is_nil() || item_id.is_nil() || asset_id.uuid().is_nil())
        {
            return Err(EnvironmentPushError::Invalid);
        }
        let mut state = self.state();
        let current_local_matches = local_choice.is_some_and(|choice| state.local.as_ref().is_some_and(|local| local.choice() == choice));
        let pending_matches = local_choice
            .is_some_and(|choice| self.local_pending.as_ref().is_some_and(|pending| pending.choice == *choice && pending.region == region));
        if state.selected == choice && (local_choice.is_none() || current_local_matches || pending_matches) {
            if current_local_matches
                && let Some(local) = state.local.as_ref()
                && local.region() != region
            {
                state.local = Some(local.with_region(region));
                self.store(state)?;
            }
            self.local_region = Some(region);
            self.route_local();
            return Ok(());
        }
        let release_count = usize::from(self.local_pending.is_some());
        let request_count = usize::from(local_choice.is_some() && !current_local_matches && !pending_matches);
        if self.events.len().saturating_add(release_count).saturating_add(request_count) > 2 * MAX_PENDING {
            return Err(EnvironmentPushError::Capacity);
        }
        state.selected = choice;
        state.local_error = None;
        if matches!(&state.selected, EnvironmentChoice::Shared) {
            state.local = None;
        }
        self.store(state)?;
        self.local_region = Some(region);
        if !pending_matches {
            self.cancel_local_pending();
        }
        self.route_local();
        Ok(())
    }

    pub(in crate::scene) fn retry_local_route(&mut self) {
        self.route_local();
    }

    fn route_local(&mut self) {
        let Some(region) = self.local_region else { return };
        let Some(choice) = self.selected().local().cloned() else { return };
        let Some(asset) = choice.asset_id() else { return };
        if self.current.as_ref().and_then(ViewerEnvironment::local).is_some_and(|local| local.choice() == &choice)
            || self.local_pending.as_ref().is_some_and(|pending| pending.choice == choice)
            || self.events.len() >= 2 * MAX_PENDING
        {
            return;
        }
        self.cancel_local_pending();
        let Some(lease) = self.budget.reserve(size_of::<LocalPending>() + 64) else { return };
        let ticket = AssetTicket::new();
        self.local_pending = Some(LocalPending { ticket, region, choice, result: None, _lease: lease });
        self.events.push_back(AssetChange::Request(AssetRequest {
            ticket,
            region,
            request: Request::Environment(asset),
            priority: Priority::Visible,
            importance: 0,
        }));
    }

    pub(in crate::scene) fn close(&mut self) {
        self.cancel_local_pending();
        self.local_region = None;
        for pending in self.pending.drain(..) {
            self.events.push_back(AssetChange::Release(pending.ticket));
        }
        self.current = None;
    }

    fn cancel_local_pending(&mut self) {
        if let Some(pending) = self.local_pending.take() {
            self.events.push_back(AssetChange::Release(pending.ticket));
        }
    }

    pub(in crate::scene) fn push_at(&mut self, region: Region, push: EnvironmentPush, now: Instant) -> Result<(), EnvironmentPushError> {
        let transition = match &push {
            EnvironmentPush::Clear { transition, .. }
            | EnvironmentPush::Full { transition, .. }
            | EnvironmentPush::Partial { transition, .. } => *transition,
        };
        if now.checked_add(transition).is_none() {
            return Err(EnvironmentPushError::Invalid);
        }
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
                self.pending.push_back(Pending { ticket, region, experience, transition, result: None, _lease: lease });
                self.events.push_back(AssetChange::Request(AssetRequest {
                    ticket,
                    region,
                    request: Request::Environment(asset),
                    priority: Priority::Visible,
                    importance: 0,
                }));
            }
            EnvironmentPush::Clear { experience, transition } => {
                let mut state = self.state();
                let mut property_changes = self.property_changes(now);
                let mut property_sequence = self.property_sequence;
                if state.day.as_ref().is_some_and(|setting| experience.is_none_or(|id| id == setting.experience())) {
                    state.day = None;
                }
                if state.sky.as_ref().is_some_and(|setting| experience.is_none_or(|id| id == setting.experience())) {
                    state.sky = None;
                }
                if state.water.as_ref().is_some_and(|setting| experience.is_none_or(|id| id == setting.experience())) {
                    state.water = None;
                }
                clear_properties(
                    &mut state.sky_properties,
                    PropertyKind::Sky,
                    experience,
                    transition,
                    now,
                    &mut property_changes,
                    &mut property_sequence,
                )?;
                clear_properties(
                    &mut state.water_properties,
                    PropertyKind::Water,
                    experience,
                    transition,
                    now,
                    &mut property_changes,
                    &mut property_sequence,
                )?;
                self.store_with_clear_transition(state, Some((region, transition)), property_changes)?;
                self.property_sequence = property_sequence;
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
                if sky.len() > MAX_PROPERTIES || water.len() > MAX_PROPERTIES {
                    return Err(EnvironmentPushError::Capacity);
                }
                Sky::from_properties(sky.iter().map(|property| (property.name.as_str(), property.source.as_ref())))
                    .map_err(|_| EnvironmentPushError::Invalid)?;
                Water::from_properties(water.iter().map(|property| (property.name.as_str(), property.source.as_ref())))
                    .map_err(|_| EnvironmentPushError::Invalid)?;
                let mut state = self.state();
                let mut property_changes = self.property_changes(now);
                let mut property_sequence = self.property_sequence;
                self.properties(
                    &mut state.sky_properties,
                    sky,
                    PropertyKind::Sky,
                    region,
                    experience,
                    transition,
                    now,
                    &mut property_changes,
                    &mut property_sequence,
                )?;
                self.properties(
                    &mut state.water_properties,
                    water,
                    PropertyKind::Water,
                    region,
                    experience,
                    transition,
                    now,
                    &mut property_changes,
                    &mut property_sequence,
                )?;
                self.store_with_clear_transition(state, None, property_changes)?;
                self.property_sequence = property_sequence;
            }
        }
        Ok(())
    }

    pub(in crate::scene) fn complete(&mut self, ticket: AssetTicket, status: &Status) -> bool {
        if let Some(pending) = self.local_pending.as_mut().filter(|pending| pending.ticket == ticket) {
            pending.result = match status {
                Status::Ready(Content::Environment(environment)) if pending.choice.accepts(environment) => Some(Ok(environment.clone())),
                Status::Failed { failure, .. } => Some(Err(*failure)),
                Status::StaleSession => Some(Err(Failure::SessionClosed)),
                Status::Ready(_) => Some(Err(Failure::Malformed)),
                _ => None,
            };
            self.advance_local();
            return true;
        }
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
        self.advance_local();
        while let Some(pending) = self.pending.front().filter(|pending| pending.result.is_some()) {
            if let Some(Ok(environment)) = &pending.result {
                let mut state = self.state();
                let Some(lease) = self.budget.reserve(size_of::<Setting>() + 32) else { break };
                let sequence = self.setting_sequence.checked_add(1).expect("environment setting sequence exhausted");
                let setting = ExperienceSetting(Arc::new(Setting {
                    region: pending.region,
                    experience: pending.experience,
                    sequence,
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
                self.setting_sequence = sequence;
            }
            let pending = self.pending.pop_front().expect("completed environment push");
            self.events.push_back(AssetChange::Release(pending.ticket));
        }
    }

    fn advance_local(&mut self) {
        let Some(pending) = self.local_pending.as_ref().filter(|pending| pending.result.is_some()) else { return };
        let ticket = pending.ticket;
        let choice = pending.choice.clone();
        let region = pending.region;
        let result = pending.result.clone().expect("completed local environment request");
        let mut state = self.state();
        match result {
            Ok(environment) => {
                let Some(lease) = self.budget.reserve(size_of::<LocalSetting>() + 32) else { return };
                state.local = Some(LocalEnvironment { setting: Arc::new(LocalSetting::new(choice.clone(), environment, lease)), region });
                state.selected = EnvironmentChoice::Local(choice);
            }
            Err(error) => {
                state.selected =
                    state.local.as_ref().map_or_else(EnvironmentChoice::default, |local| EnvironmentChoice::Local(local.choice().clone()));
                state.local_error = Some(error);
            }
        }
        if self.store(state).is_err() {
            return;
        }
        self.local_pending = None;
        self.events.push_back(AssetChange::Release(ticket));
    }

    fn state(&self) -> State {
        self.current.as_ref().map_or_else(State::default, |current| current.0.state.clone())
    }

    fn store(&mut self, state: State) -> Result<(), EnvironmentPushError> {
        let property_changes = self.property_changes(Instant::now());
        self.store_with_clear_transition(state, None, property_changes)
    }

    fn store_with_clear_transition(
        &mut self,
        state: State,
        clear_transition: Option<(Region, Duration)>,
        property_changes: VecDeque<PropertyChange>,
    ) -> Result<(), EnvironmentPushError> {
        let sky_overrides = overrides_sky(&state.sky_properties)?;
        let water_overrides = overrides_water(&state.water_properties)?;
        let sky_overrides_by_region = overrides_sky_by_region(&state.sky_properties)?;
        let water_overrides_by_region = overrides_water_by_region(&state.water_properties)?;
        let bytes = size_of::<Snapshot>()
            + 32
            + sky_overrides.as_ref().map_or(0, |sky| sky.retained_bytes().saturating_sub(size_of::<Sky>()))
            + sky_overrides_by_region.values().map(Sky::retained_bytes).sum::<usize>()
            + water_overrides_by_region.len() * size_of::<Water>()
            + state
                .sky_properties
                .keys()
                .chain(state.water_properties.keys())
                .map(|key| key.capacity() + size_of::<ExperienceProperty>() + 128)
                .sum::<usize>()
            + property_changes.iter().map(|change| size_of::<PropertyChange>() + change.name.capacity() + 32).sum::<usize>();
        let lease = self.budget.reserve(bytes).ok_or(EnvironmentPushError::Capacity)?;
        let version = self.version.checked_add(1).expect("viewer environment version exhausted");
        self.current = Some(ViewerEnvironment(Arc::new(Snapshot {
            state,
            sky_overrides,
            water_overrides,
            sky_overrides_by_region,
            water_overrides_by_region,
            property_changes,
            clear_transition,
            version,
            _lease: lease,
        })));
        self.version = version;
        Ok(())
    }

    fn property_changes(&self, now: Instant) -> VecDeque<PropertyChange> {
        let mut changes = self.current.as_ref().map_or_else(VecDeque::new, |current| current.0.property_changes.clone());
        changes
            .retain(|change| change.duration > Duration::ZERO && change.started.checked_add(change.duration).is_some_and(|end| now < end));
        changes
    }

    fn properties(
        &self,
        target: &mut BTreeMap<String, ExperienceProperty>,
        properties: Vec<EnvironmentProperty>,
        kind: PropertyKind,
        region: Region,
        experience: ExperienceId,
        transition: Duration,
        now: Instant,
        changes: &mut VecDeque<PropertyChange>,
        sequence: &mut u64,
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
            let value = match kind {
                PropertyKind::Sky => PropertyValue::Sky(Arc::new(
                    Sky::from_properties([(property.name.as_str(), property.source.as_ref())])
                        .map_err(|_| EnvironmentPushError::Invalid)?,
                )),
                PropertyKind::Water => PropertyValue::Water(Arc::new(
                    Water::from_properties([(property.name.as_str(), property.source.as_ref())])
                        .map_err(|_| EnvironmentPushError::Invalid)?,
                )),
            };
            *sequence = sequence.checked_add(1).expect("environment property sequence exhausted");
            let next = ExperienceProperty(Arc::new(Property {
                region,
                experience,
                source: property.source,
                transition,
                sequence: *sequence,
                value,
                _lease: lease,
            }));
            let previous = target.insert(property.name.clone(), next.clone());
            push_property_change(changes, kind, property.name, previous, Some(next), now, transition, sequence)?;
        }
        Ok(())
    }
}

fn overrides_sky(properties: &BTreeMap<String, ExperienceProperty>) -> Result<Option<Sky>, EnvironmentPushError> {
    if properties.is_empty() {
        return Ok(None);
    }
    Sky::from_properties(properties.iter().map(|(name, property)| (name.as_str(), property.source())))
        .map(Some)
        .map_err(|_| EnvironmentPushError::Invalid)
}

fn overrides_water(properties: &BTreeMap<String, ExperienceProperty>) -> Result<Option<Water>, EnvironmentPushError> {
    if properties.is_empty() {
        return Ok(None);
    }
    Water::from_properties(properties.iter().map(|(name, property)| (name.as_str(), property.source())))
        .map(Some)
        .map_err(|_| EnvironmentPushError::Invalid)
}

fn overrides_sky_by_region(properties: &BTreeMap<String, ExperienceProperty>) -> Result<BTreeMap<Region, Sky>, EnvironmentPushError> {
    let mut groups: BTreeMap<Region, Vec<(&str, &[u8])>> = BTreeMap::new();
    for (name, property) in properties {
        groups.entry(property.region()).or_default().push((name.as_str(), property.source()));
    }
    groups
        .into_iter()
        .map(|(region, properties)| Sky::from_properties(properties).map(|sky| (region, sky)).map_err(|_| EnvironmentPushError::Invalid))
        .collect()
}

fn overrides_water_by_region(properties: &BTreeMap<String, ExperienceProperty>) -> Result<BTreeMap<Region, Water>, EnvironmentPushError> {
    let mut groups: BTreeMap<Region, Vec<(&str, &[u8])>> = BTreeMap::new();
    for (name, property) in properties {
        groups.entry(property.region()).or_default().push((name.as_str(), property.source()));
    }
    groups
        .into_iter()
        .map(|(region, properties)| {
            Water::from_properties(properties).map(|water| (region, water)).map_err(|_| EnvironmentPushError::Invalid)
        })
        .collect()
}

fn push_property_change(
    changes: &mut VecDeque<PropertyChange>,
    kind: PropertyKind,
    name: String,
    previous: Option<ExperienceProperty>,
    target: Option<ExperienceProperty>,
    started: Instant,
    duration: Duration,
    sequence: &mut u64,
) -> Result<(), EnvironmentPushError> {
    if changes.len() >= MAX_PROPERTY_CHANGES {
        return Err(EnvironmentPushError::Capacity);
    }
    changes.push_back(PropertyChange { sequence: *sequence, kind, name, previous, target, started, duration });
    Ok(())
}

fn clear_properties(
    properties: &mut BTreeMap<String, ExperienceProperty>,
    kind: PropertyKind,
    experience: Option<ExperienceId>,
    duration: Duration,
    started: Instant,
    changes: &mut VecDeque<PropertyChange>,
    sequence: &mut u64,
) -> Result<(), EnvironmentPushError> {
    let names = properties
        .iter()
        .filter(|(_, property)| experience.is_none_or(|experience| experience == property.experience()))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    if changes.len().saturating_add(names.len()) > MAX_PROPERTY_CHANGES {
        return Err(EnvironmentPushError::Capacity);
    }
    for name in names {
        let Some(previous) = properties.remove(&name) else { continue };
        *sequence = sequence.checked_add(1).expect("environment property sequence exhausted");
        push_property_change(changes, kind, name, Some(previous), None, started, duration, sequence)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requested(manager: &mut ViewerEnvironments, preset: EnvironmentPreset) -> AssetRequest {
        let Some(AssetChange::Request(request)) = manager.event() else { panic!("local preset request is queued") };
        assert_eq!(request.request, Request::Environment(preset.asset_id().expect("sky preset has an asset")));
        request
    }

    #[test]
    fn preset_ids_match_viewer() {
        assert_eq!(EnvironmentPreset::Sunrise.asset_id(), Some(AssetId::new(Uuid::from_u128(0x01e41537ff512f1f8ef717e4df760bfb))));
        assert_eq!(EnvironmentPreset::Midday.asset_id(), Some(AssetId::new(Uuid::from_u128(0xc46226b40e435a569708d27ca1df3292))));
        assert_eq!(EnvironmentPreset::Sunset.asset_id(), Some(AssetId::new(Uuid::from_u128(0x084e26cda90028e808d064a9de5c15e2))));
        assert_eq!(EnvironmentPreset::Midnight.asset_id(), Some(AssetId::new(Uuid::from_u128(0x8a01b97acb20c1eaac63f7ea84ad0090))));
        assert_eq!(EnvironmentChoice::Shared.local(), None);
    }

    #[test]
    fn changing_preset_releases_old_request() {
        let mut manager = ViewerEnvironments::new(Budget::default());
        let region = Region::new(1);
        manager.set_local(EnvironmentChoice::Local(LocalChoice::Preset(EnvironmentPreset::Midday)), region).unwrap();
        let first = requested(&mut manager, EnvironmentPreset::Midday);
        manager.set_local(EnvironmentChoice::Local(LocalChoice::Preset(EnvironmentPreset::Sunset)), region).unwrap();
        assert!(matches!(manager.event(), Some(AssetChange::Release(ticket)) if ticket == first.ticket));
        let second = requested(&mut manager, EnvironmentPreset::Sunset);
        assert_ne!(first.ticket, second.ticket);
    }

    #[test]
    fn clear_duration_presence_tracks_snapshots() {
        let mut manager = ViewerEnvironments::new(Budget::default());
        let region = Region::new(1);
        manager.push_at(region, EnvironmentPush::Clear { experience: None, transition: Duration::ZERO }, Instant::now()).unwrap();
        let cleared = manager.current().expect("clear snapshot");
        assert_eq!(cleared.clear_transition_for_region(region), Some(Duration::ZERO));
        assert_eq!(cleared.clear_transition_for_region(Region::new(2)), None);
        let state = manager.state();
        manager.store(state).unwrap();
        assert_eq!(manager.current().expect("ordinary snapshot").clear_transition_for_region(region), None);
    }
}
