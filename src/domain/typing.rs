use crate::domain::member_name::NamedUser;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Typists(Vec<NamedUser>);

impl Typists {
    pub fn replaced_by(&self, mut typing: Vec<NamedUser>) -> Self {
        let mut ordered = Vec::with_capacity(typing.len());
        for shown in &self.0 {
            if let Some(at) = typing.iter().position(|user| user.user_id == shown.user_id) {
                ordered.push(typing.remove(at));
            }
        }
        ordered.extend(typing);
        Self(ordered)
    }

    pub fn users(&self) -> &[NamedUser] {
        &self.0
    }
}
