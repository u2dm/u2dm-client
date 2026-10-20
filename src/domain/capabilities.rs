pub const DEFAULT_ROOM_VERSION: &str = "1";
pub const DISPLAY_NAME_FIELD: &str = "displayname";
pub const AVATAR_FIELD: &str = "avatar_url";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub change_password: bool,
    pub profile_fields: ProfileFields,
    pub default_room_version: String,
    pub forget_forced_upon_leave: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            change_password: true,
            profile_fields: ProfileFields::unrestricted(),
            default_room_version: DEFAULT_ROOM_VERSION.to_owned(),
            forget_forced_upon_leave: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFields {
    enabled: bool,
    allowed: Option<Vec<String>>,
    disallowed: Vec<String>,
}

impl ProfileFields {
    pub fn new(enabled: bool, allowed: Option<Vec<String>>, disallowed: Vec<String>) -> Self {
        Self {
            enabled,
            allowed,
            disallowed,
        }
    }

    pub fn unrestricted() -> Self {
        Self::new(true, None, Vec::new())
    }

    pub fn only(fields: impl IntoIterator<Item = &'static str>) -> Self {
        Self::new(
            true,
            Some(fields.into_iter().map(str::to_owned).collect()),
            Vec::new(),
        )
    }

    pub fn can_set(&self, field: &str) -> bool {
        self.enabled
            && self.allowed.as_ref().map_or_else(
                || !self.disallowed.iter().any(|listed| listed == field),
                |allowed| allowed.iter().any(|listed| listed == field),
            )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AccountApi {
    #[default]
    Legacy,
    OAuth {
        pages: bool,
    },
}

impl AccountApi {
    pub fn is_oauth(self) -> bool {
        matches!(self, Self::OAuth { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountPage {
    Account,
    Sessions,
    SignOut(String),
    Deactivate,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountFacts {
    pub api: AccountApi,
    pub capabilities: Capabilities,
}
