use super::kept_document;
use crate::protected::{Opener, Protected};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{btree_map, BTreeMap, BTreeSet};

fn id_of(info: &Value) -> Option<&str> {
    info.get("targetId").and_then(Value::as_str)
}

fn iframe(info: &Value) -> bool {
    info.get("type").and_then(Value::as_str) == Some("iframe")
}

#[derive(Clone, Copy)]
pub(super) struct Verdicts {
    pub(super) protected: bool,
    pub(super) inheriting: bool,
    pub(super) unparseable: bool,
    pub(super) extension_frame: bool,
    pub(super) kept: bool,
}

impl Verdicts {
    fn of(guard: &Protected, url: &str) -> Self {
        Self {
            protected: guard.url(url),
            inheriting: guard.inheriting(url),
            unparseable: guard.unparseable(url),
            extension_frame: guard.extension_frame(url),
            kept: kept_document(guard, url),
        }
    }

    pub(super) fn target(&self, previous: bool, opener: Opener) -> bool {
        Protected::target_judged(self.inheriting, || self.protected, previous, opener)
    }
}

#[derive(Default)]
pub(super) struct Targets {
    infos: BTreeMap<String, Value>,
    ids: BTreeMap<String, BTreeSet<String>>,
    iframes: BTreeSet<String>,
    changed: BTreeSet<String>,
    verdicts: RefCell<BTreeMap<String, (String, Verdicts)>>,
}

impl Targets {
    pub(super) fn get(&self, key: &str) -> Option<&Value> {
        self.infos.get(key)
    }

    pub(super) fn contains_key(&self, key: &str) -> bool {
        self.infos.contains_key(key)
    }

    pub(super) fn iter(&self) -> btree_map::Iter<'_, String, Value> {
        self.infos.iter()
    }

    #[cfg(test)]
    pub(super) fn values(&self) -> btree_map::Values<'_, String, Value> {
        self.infos.values()
    }

    pub(super) fn insert(&mut self, key: String, info: Value) -> Option<Value> {
        let previous = self.forget(&key);
        if let Some(id) = id_of(&info) {
            self.ids.entry(id.to_string()).or_default().insert(key.clone());
        }
        if iframe(&info) {
            self.iframes.insert(key.clone());
        }
        self.changed.insert(key.clone());
        self.infos.insert(key, info);
        previous
    }

    pub(super) fn remove(&mut self, key: &str) -> Option<Value> {
        self.verdicts.get_mut().remove(key);
        let previous = self.forget(key);
        if previous.is_some() {
            self.changed.insert(key.to_string());
        }
        previous
    }

    fn forget(&mut self, key: &str) -> Option<Value> {
        let previous = self.infos.remove(key)?;
        if let Some(id) = id_of(&previous) {
            if let Some(keys) = self.ids.get_mut(id) {
                keys.remove(key);
                if keys.is_empty() {
                    self.ids.remove(id);
                }
            }
        }
        self.iframes.remove(key);
        Some(previous)
    }

    pub(super) fn with_id<'a>(&'a self, id: &str) -> impl Iterator<Item = &'a Value> + 'a {
        self.ids.get(id).into_iter().flatten().filter_map(|key| self.infos.get(key))
    }

    pub(super) fn by_id(&self, id: &str) -> Option<&Value> {
        self.with_id(id).next()
    }

    pub(super) fn iframe_values(&self) -> impl Iterator<Item = &Value> {
        self.iframes.iter().filter_map(|key| self.infos.get(key))
    }

    pub(super) fn verdicts(&self, info: &Value, guard: &Protected) -> Verdicts {
        let url = info.get("url").and_then(Value::as_str).unwrap_or("");
        let Some(id) = id_of(info).filter(|id| self.infos.contains_key(*id)) else {
            return Verdicts::of(guard, url);
        };
        if let Some((seen, verdicts)) = self.verdicts.borrow().get(id) {
            if seen == url {
                return *verdicts;
            }
        }
        let verdicts = Verdicts::of(guard, url);
        self.verdicts.borrow_mut().insert(id.to_string(), (url.to_string(), verdicts));
        verdicts
    }

    pub(super) fn forget_verdicts(&mut self) {
        self.verdicts.get_mut().clear();
    }

    pub(super) fn take_changed(&mut self) -> BTreeSet<String> {
        std::mem::take(&mut self.changed)
    }
}

impl std::ops::Index<&str> for Targets {
    type Output = Value;

    fn index(&self, key: &str) -> &Value {
        &self.infos[key]
    }
}

impl<'a> IntoIterator for &'a Targets {
    type Item = (&'a String, &'a Value);
    type IntoIter = btree_map::Iter<'a, String, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.infos.iter()
    }
}

pub(crate) struct Listing<'a> {
    infos: &'a [Value],
    ids: BTreeMap<&'a str, Vec<usize>>,
    iframes: Vec<usize>,
}

impl<'a> Listing<'a> {
    pub(crate) const NONE: Listing<'static> = Listing { infos: &[], ids: BTreeMap::new(), iframes: Vec::new() };

    pub(crate) fn new(infos: &'a [Value]) -> Self {
        let mut ids: BTreeMap<&'a str, Vec<usize>> = BTreeMap::new();
        let mut iframes = Vec::new();
        for (at, info) in infos.iter().enumerate() {
            if let Some(id) = id_of(info) {
                ids.entry(id).or_default().push(at);
            }
            if iframe(info) {
                iframes.push(at);
            }
        }
        Self { infos, ids, iframes }
    }

    pub(crate) fn iframes<'s>(&'s self) -> impl Iterator<Item = &'a Value> + 's {
        let infos = self.infos;
        self.iframes.iter().map(move |at| &infos[*at])
    }

    pub(crate) fn with_id<'s>(&'s self, id: &str) -> impl Iterator<Item = &'a Value> + 's {
        let infos = self.infos;
        self.ids.get(id).into_iter().flatten().map(move |at| &infos[*at])
    }

    pub(crate) fn first(&self, id: &str) -> Option<&'a Value> {
        self.with_id(id).next()
    }
}

pub(crate) trait Infos {
    fn listing<R>(&self, judge: impl FnOnce(&Listing) -> R) -> R;
}

impl Infos for [Value] {
    fn listing<R>(&self, judge: impl FnOnce(&Listing) -> R) -> R {
        judge(&Listing::new(self))
    }
}

impl<const N: usize> Infos for [Value; N] {
    fn listing<R>(&self, judge: impl FnOnce(&Listing) -> R) -> R {
        judge(&Listing::new(self))
    }
}

impl Infos for Vec<Value> {
    fn listing<R>(&self, judge: impl FnOnce(&Listing) -> R) -> R {
        judge(&Listing::new(self))
    }
}

impl Infos for Listing<'_> {
    fn listing<R>(&self, judge: impl FnOnce(&Listing) -> R) -> R {
        judge(self)
    }
}

#[derive(Default)]
pub(super) struct FrameOwners {
    owners: BTreeMap<String, String>,
    touched: BTreeSet<String>,
    bound: BTreeMap<String, String>,
}

impl FrameOwners {
    pub(super) fn get(&self, frame: &str) -> Option<&String> {
        self.owners.get(frame)
    }

    pub(super) fn contains_key(&self, frame: &str) -> bool {
        self.owners.contains_key(frame)
    }

    pub(super) fn insert(&mut self, frame: String, owner: String) -> Option<String> {
        self.touched.insert(frame.clone());
        self.owners.insert(frame, owner)
    }

    pub(super) fn retain(&mut self, mut keep: impl FnMut(&String, &mut String) -> bool) {
        let touched = &mut self.touched;
        self.owners.retain(|frame, owner| {
            let kept = keep(frame, owner);
            if !kept {
                touched.insert(frame.clone());
            }
            kept
        });
    }

    pub(super) fn bind(&mut self, targets: &mut Targets, documents: BTreeMap<String, String>) {
        let mut frames = targets.take_changed();
        frames.append(&mut self.touched);
        frames.extend(self.bound.values().cloned());
        let mut writers: BTreeMap<&str, &str> = BTreeMap::new();
        for (target, frame) in &documents {
            frames.insert(frame.clone());
            let best = writers.entry(frame.as_str()).or_insert(target.as_str());
            if *best < target.as_str() {
                *best = target.as_str();
            }
        }
        for frame in &frames {
            if frame.is_empty() {
                continue;
            }
            let own = targets.get(frame).is_some_and(iframe).then_some(frame.as_str());
            let Some(owner) = own.max(writers.get(frame.as_str()).copied()) else {
                continue;
            };
            if self.owners.get(frame).map(String::as_str) != Some(owner) {
                self.owners.insert(frame.clone(), owner.to_string());
            }
        }
        self.bound = documents;
    }
}

impl<'a> IntoIterator for &'a FrameOwners {
    type Item = (&'a String, &'a String);
    type IntoIter = btree_map::Iter<'a, String, String>;

    fn into_iter(self) -> Self::IntoIter {
        self.owners.iter()
    }
}
