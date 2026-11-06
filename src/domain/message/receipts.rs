use std::collections::BTreeSet;

use crate::domain::member_name::NamedUser;

pub const READERS_NAMED: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadBy {
    pub named: Vec<NamedUser>,
    pub total: usize,
}

impl ReadBy {
    pub fn is_read(&self) -> bool {
        self.total > 0
    }

    pub fn hidden(&self) -> usize {
        self.total.saturating_sub(self.named.len())
    }
}

pub struct ReadScan<'own> {
    own_user_id: Option<&'own str>,
    readers: BTreeSet<String>,
}

impl<'own> ReadScan<'own> {
    pub fn excluding(own_user_id: Option<&'own str>) -> Self {
        Self {
            own_user_id,
            readers: BTreeSet::new(),
        }
    }

    pub fn observe<'id>(&mut self, user_ids: impl IntoIterator<Item = &'id str>) {
        for user_id in user_ids {
            if Some(user_id) != self.own_user_id && !self.readers.contains(user_id) {
                self.readers.insert(user_id.to_owned());
            }
        }
    }

    pub fn describes(&self, read_by: &ReadBy, sender: &str) -> bool {
        read_by.total == self.total(sender)
            && read_by
                .named
                .iter()
                .map(|reader| reader.user_id.as_str())
                .eq(self.named(sender))
    }

    pub fn read_by(&self, sender: &str) -> ReadBy {
        ReadBy {
            named: self.named(sender).map(NamedUser::unnamed).collect(),
            total: self.total(sender),
        }
    }

    pub fn readers(&self, sender: &str) -> Vec<String> {
        self.readers
            .iter()
            .filter(|reader| reader.as_str() != sender)
            .cloned()
            .collect()
    }

    fn total(&self, sender: &str) -> usize {
        self.readers
            .len()
            .saturating_sub(usize::from(self.readers.contains(sender)))
    }

    fn named<'a>(&'a self, sender: &'a str) -> impl Iterator<Item = &'a str> {
        self.readers
            .iter()
            .map(String::as_str)
            .filter(move |reader| *reader != sender)
            .take(READERS_NAMED)
    }
}
