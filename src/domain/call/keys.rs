use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Duration;

use super::{CallMember, MemberKey};

pub const KEY_LEN: usize = 16;
pub const KEY_REUSE_WINDOW: Duration = Duration::from_secs(10);
pub const KEY_SWITCH_DELAY: Duration = Duration::from_secs(1);
const QUEUED_KEYS_MAX: usize = 64;

#[derive(Clone, PartialEq, Eq)]
pub struct MediaKey([u8; KEY_LEN]);

impl MediaKey {
    pub const fn new(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl fmt::Debug for MediaKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MediaKey(..)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyIndex(u8);

impl KeyIndex {
    pub const FIRST: Self = Self(0);

    pub const fn new(index: u8) -> Self {
        Self(index)
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

impl fmt::Display for KeyIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyHolder {
    pub member: MemberKey,
    pub since_ms: u64,
}

impl KeyHolder {
    pub fn of(member: &CallMember) -> Self {
        Self {
            member: MemberKey::of(member),
            since_ms: member.created_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedKey {
    pub member: MemberKey,
    pub index: KeyIndex,
    pub key: MediaKey,
    pub sent_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyAction {
    Send {
        to: Vec<MemberKey>,
        index: KeyIndex,
        key: MediaKey,
    },
    InstallOwn {
        index: KeyIndex,
        key: MediaKey,
    },
    InstallRemote {
        member: MemberKey,
        index: KeyIndex,
        key: MediaKey,
    },
    UseOwnIndex(KeyIndex),
    WakeAt(Duration),
}

struct OwnKey {
    index: KeyIndex,
    key: MediaKey,
    made: Duration,
}

#[derive(Default)]
pub struct KeyExchange {
    own: Option<OwnKey>,
    switch: Option<(KeyIndex, Duration)>,
    holders: HashSet<KeyHolder>,
    newest: HashMap<(MemberKey, KeyIndex), u64>,
    queued: Vec<ReceivedKey>,
}

impl KeyExchange {
    pub fn members_changed(
        &mut self,
        now: Duration,
        others: &[KeyHolder],
        fresh: MediaKey,
    ) -> Vec<KeyAction> {
        let mut actions = self.release_queued(others);
        let Some(own) = &self.own else {
            actions.extend(self.first_key(now, others, fresh));
            return actions;
        };
        let someone_left = self.holders.iter().any(|holder| !others.contains(holder));
        let joiners: Vec<&KeyHolder> = others
            .iter()
            .filter(|holder| !self.holders.contains(*holder))
            .collect();
        let key_is_old = now.saturating_sub(own.made) >= KEY_REUSE_WINDOW;
        if someone_left || (!joiners.is_empty() && key_is_old) {
            actions.extend(self.rotate(now, others, fresh));
        } else if !joiners.is_empty() {
            actions.push(KeyAction::Send {
                to: joiners.iter().map(|holder| holder.member.clone()).collect(),
                index: own.index,
                key: own.key.clone(),
            });
            let joiners: Vec<KeyHolder> = joiners.into_iter().cloned().collect();
            self.holders.extend(joiners);
        }
        actions
    }

    pub fn received(&mut self, key: ReceivedKey, present: &[KeyHolder]) -> Vec<KeyAction> {
        if self.is_stale(&key) {
            return Vec::new();
        }
        if !present.iter().any(|holder| holder.member == key.member) {
            self.queue(key);
            return Vec::new();
        }
        vec![self.install(key)]
    }

    pub fn tick(&mut self, now: Duration) -> Vec<KeyAction> {
        match self.switch {
            Some((index, at)) if at <= now => {
                self.switch = None;
                vec![KeyAction::UseOwnIndex(index)]
            }
            Some((_, at)) => vec![KeyAction::WakeAt(at)],
            None => Vec::new(),
        }
    }

    pub fn has_key_from(&self, member: &MemberKey) -> bool {
        self.newest.keys().any(|(holder, _)| holder == member)
    }

    fn first_key(
        &mut self,
        now: Duration,
        others: &[KeyHolder],
        fresh: MediaKey,
    ) -> Vec<KeyAction> {
        let index = KeyIndex::FIRST;
        self.own = Some(OwnKey {
            index,
            key: fresh.clone(),
            made: now,
        });
        self.holders = others.iter().cloned().collect();
        let mut actions = vec![
            KeyAction::InstallOwn {
                index,
                key: fresh.clone(),
            },
            KeyAction::UseOwnIndex(index),
        ];
        actions.extend(send_to(others, index, fresh));
        actions
    }

    fn rotate(&mut self, now: Duration, others: &[KeyHolder], fresh: MediaKey) -> Vec<KeyAction> {
        let index = self
            .own
            .as_ref()
            .map_or(KeyIndex::FIRST, |own| own.index.next());
        self.own = Some(OwnKey {
            index,
            key: fresh.clone(),
            made: now,
        });
        self.holders = others.iter().cloned().collect();
        let at = now.saturating_add(KEY_SWITCH_DELAY);
        self.switch = Some((index, at));
        let mut actions = vec![KeyAction::InstallOwn {
            index,
            key: fresh.clone(),
        }];
        actions.extend(send_to(others, index, fresh));
        actions.push(KeyAction::WakeAt(at));
        actions
    }

    fn is_stale(&self, key: &ReceivedKey) -> bool {
        self.newest
            .get(&(key.member.clone(), key.index))
            .is_some_and(|newest| *newest >= key.sent_ms)
    }

    fn install(&mut self, key: ReceivedKey) -> KeyAction {
        self.newest
            .insert((key.member.clone(), key.index), key.sent_ms);
        KeyAction::InstallRemote {
            member: key.member,
            index: key.index,
            key: key.key,
        }
    }

    fn queue(&mut self, key: ReceivedKey) {
        self.queued
            .retain(|queued| !(queued.member == key.member && queued.index == key.index));
        if self.queued.len() >= QUEUED_KEYS_MAX {
            self.queued.remove(0);
        }
        self.queued.push(key);
    }

    fn release_queued(&mut self, present: &[KeyHolder]) -> Vec<KeyAction> {
        let (ready, waiting): (Vec<ReceivedKey>, Vec<ReceivedKey>) = self
            .queued
            .drain(..)
            .partition(|key| present.iter().any(|holder| holder.member == key.member));
        self.queued = waiting;
        let mut installs = Vec::new();
        for key in ready {
            if !self.is_stale(&key) {
                installs.push(self.install(key));
            }
        }
        installs
    }
}

fn send_to(others: &[KeyHolder], index: KeyIndex, key: MediaKey) -> Option<KeyAction> {
    (!others.is_empty()).then(|| KeyAction::Send {
        to: others.iter().map(|holder| holder.member.clone()).collect(),
        index,
        key,
    })
}
