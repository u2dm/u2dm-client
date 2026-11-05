macro_rules! string_id {
    ($(#[$meta:meta])* pub struct $name:ident(String);) => {
        $(#[$meta])*
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

macro_rules! derefs_to_str {
    ($name:ident) => {
        impl ::std::ops::Deref for $name {
            type Target = str;

            fn deref(&self) -> &str {
                &self.0
            }
        }
    };
}

macro_rules! redacted {
    ($(#[$meta:meta])* pub struct $name:ident(String) => $shown:literal;) => {
        $(#[$meta])*
        pub struct $name(String);

        impl $name {
            pub fn new(secret: impl Into<String>) -> Self {
                Self(secret.into())
            }

            pub fn expose(&self) -> &str {
                &self.0
            }
        }

        impl ::std::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str($shown)
            }
        }
    };
}

pub mod account;
pub mod auth;
pub mod call;
pub mod capabilities;
pub mod departure;
pub mod emote;
pub mod encryption;
pub mod event_size;
pub mod identity;
pub mod image_pack;
pub mod invite;
pub mod invite_people;
pub mod link;
pub mod location;
pub mod matrix_link;
pub mod media;
pub mod member_name;
pub mod mention;
pub mod message;
pub mod poll;
pub mod presence;
pub mod push;
pub mod room;
pub mod room_creation;
pub mod room_info;
pub mod room_log;
pub mod room_preview;
pub mod room_search;
pub mod room_state;
pub mod sessions;
pub mod space_index;
pub mod sync;
pub mod timeline;
pub mod typing;
pub mod uia;
pub mod user_info;
pub mod verification;
pub mod viewport;
