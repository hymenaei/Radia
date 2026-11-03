use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

use world::Region;

use crate::scene::{ExperienceProperty, PropertyChange, PropertyKind, PropertyValue, ViewerEnvironment};

use super::{Evaluated, SkyState, WaterState, blend_sky_property, blend_water_property, overlay, overlay_water};

const FRAME_CADENCE: Duration = Duration::from_millis(16);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Key {
    region: Region,
    kind: PropertyKind,
    name: String,
}

#[derive(Clone)]
enum PropertyState {
    Sky(SkyState),
    Water(WaterState),
}

#[derive(Clone)]
struct Tween {
    sequence: u64,
    affected: PropertyValue,
    from: PropertyState,
    target: Option<PropertyValue>,
    started: Instant,
    duration: Duration,
}

impl Tween {
    fn active(&self, now: Instant) -> bool {
        self.duration > Duration::ZERO && self.started.checked_add(self.duration).is_some_and(|end| now < end)
    }

    fn factor(&self, now: Instant) -> f32 {
        if self.duration == Duration::ZERO {
            return 1.0;
        }
        (now.saturating_duration_since(self.started).as_secs_f64() / self.duration.as_secs_f64()).clamp(0.0, 1.0) as f32
    }

    fn state_at(&self, base: &Evaluated, now: Instant) -> PropertyState {
        let mut output = base.clone();
        self.apply(base, &mut output, now);
        match self.affected {
            PropertyValue::Sky(_) => PropertyState::Sky(output.sky),
            PropertyValue::Water(_) => PropertyState::Water(output.water),
        }
    }

    fn apply(&self, base: &Evaluated, output: &mut Evaluated, now: Instant) {
        let factor = self.factor(now);
        match (&self.affected, &self.from) {
            (PropertyValue::Sky(property), PropertyState::Sky(from)) => {
                let mut to = base.sky.clone();
                if let Some(PropertyValue::Sky(target)) = &self.target {
                    overlay(&mut to, target);
                }
                blend_sky_property(&mut output.sky, from, &to, property, factor);
            }
            (PropertyValue::Water(property), PropertyState::Water(from)) => {
                let mut to = base.water.clone();
                if let Some(PropertyValue::Water(target)) = &self.target {
                    overlay_water(&mut to, target);
                }
                blend_water_property(&mut output.water, from, &to, property, factor);
            }
            _ => {}
        }
    }
}

#[derive(Default)]
pub(super) struct PropertyTransitions {
    active: BTreeMap<Key, Tween>,
    seen: BTreeMap<Key, u64>,
}

impl PropertyTransitions {
    pub(super) fn active_for_region(&self, region: Region, now: Instant) -> bool {
        self.active.iter().any(|(key, tween)| key.region == region && tween.active(now))
    }

    pub(super) fn apply(
        &mut self,
        viewer: Option<&ViewerEnvironment>,
        region: Region,
        base: &Evaluated,
        now: Instant,
    ) -> (Evaluated, Option<Instant>) {
        let Some(viewer) = viewer else {
            *self = Self::default();
            return (base.clone(), None);
        };
        let changes = viewer.property_changes();
        for change in changes {
            let previous = change.previous.as_ref().filter(|property| property.region() == region);
            let target = change.target.as_ref().filter(|property| property.region() == region);
            let Some(_) = target.or(previous) else { continue };
            let key = Key { region, kind: change.kind, name: change.name.clone() };
            if change.sequence <= self.seen.get(&key).copied().unwrap_or(0) {
                continue;
            }
            self.seen.insert(key.clone(), change.sequence);
            self.update_change(&key, change, previous, target, base);
        }

        let mut current = BTreeMap::new();
        for (kind, properties) in [(PropertyKind::Sky, viewer.sky_properties()), (PropertyKind::Water, viewer.water_properties())] {
            for (name, property) in properties {
                if property.region() == region {
                    current.insert(Key { region, kind, name: name.clone() }, property.clone());
                }
            }
        }
        for (key, property) in &current {
            if property.sequence() > self.seen.get(key).copied().unwrap_or(0) {
                self.seen.insert(key.clone(), property.sequence());
                self.active.remove(key);
            }
        }

        self.active.retain(|key, tween| {
            if !tween.active(now) {
                return false;
            }
            if key.region != region {
                return true;
            }
            if current.get(key).is_some_and(|property| property.sequence() == tween.sequence) {
                return true;
            }
            tween.target.is_none()
                && changes.iter().any(|change| {
                    change.sequence == tween.sequence
                        && change.kind == key.kind
                        && change.name == key.name
                        && change.previous.as_ref().is_some_and(|property| property.region() == key.region)
                })
        });

        let mut output = base.clone();
        for (key, property) in &current {
            if let Some(tween) = self.active.get(key).filter(|tween| tween.sequence == property.sequence()) {
                tween.apply(base, &mut output, now);
            } else {
                apply_value(&mut output, property.value());
            }
        }
        for (key, tween) in &self.active {
            if key.region == region && tween.target.is_none() && !current.contains_key(key) {
                tween.apply(base, &mut output, now);
            }
        }

        let mut retained = BTreeSet::new();
        retained.extend(current.keys().cloned());
        for change in changes {
            for property in change.previous.iter().chain(change.target.iter()) {
                retained.insert(Key { region: property.region(), kind: change.kind, name: change.name.clone() });
            }
        }
        retained.extend(self.active.keys().cloned());
        self.seen.retain(|key, _| retained.contains(key));

        let next = self
            .active
            .iter()
            .filter_map(|(key, tween)| {
                if key.region != region || !tween.active(now) {
                    return None;
                }
                let finish = tween.started.checked_add(tween.duration)?;
                Some(now.checked_add(FRAME_CADENCE).map_or(finish, |frame| frame.min(finish)))
            })
            .min();
        (output, next)
    }

    fn update_change(
        &mut self,
        key: &Key,
        change: &PropertyChange,
        previous: Option<&ExperienceProperty>,
        target: Option<&ExperienceProperty>,
        base: &Evaluated,
    ) {
        let affected = target.or(previous).map(ExperienceProperty::value).cloned();
        let Some(affected) = affected else { return };
        let from = self
            .active
            .get(key)
            .map(|tween| tween.state_at(base, change.started))
            .or_else(|| previous.map(|property| state_with_value(base, property.value())))
            .unwrap_or_else(|| state_for(base, &affected));
        let target = target.map(|property| property.value().clone());
        let tween = Tween { sequence: change.sequence, affected, from, target, started: change.started, duration: change.duration };
        self.active.insert(key.clone(), tween);
    }
}

fn state_for(base: &Evaluated, value: &PropertyValue) -> PropertyState {
    match value {
        PropertyValue::Sky(_) => PropertyState::Sky(base.sky.clone()),
        PropertyValue::Water(_) => PropertyState::Water(base.water.clone()),
    }
}

fn state_with_value(base: &Evaluated, value: &PropertyValue) -> PropertyState {
    let mut output = base.clone();
    apply_value(&mut output, value);
    match value {
        PropertyValue::Sky(_) => PropertyState::Sky(output.sky),
        PropertyValue::Water(_) => PropertyState::Water(output.water),
    }
}

fn apply_value(output: &mut Evaluated, value: &PropertyValue) {
    match value {
        PropertyValue::Sky(sky) => overlay(&mut output.sky, sky),
        PropertyValue::Water(water) => overlay_water(&mut output.water, water),
    }
}
