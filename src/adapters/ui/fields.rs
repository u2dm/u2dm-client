use slint::{Image, ModelRc, SharedString, StyledText};

use super::backend::UiBackend;
use super::dto::{MediaFailureKind, MediaState};
use super::present::{Delivery, MessageKind, ServiceKind};
use super::schema::{
    message_fields, reaction_fields, reactor_fields, room_fields, space_fields,
    sticker_cell_fields, sticker_pack_fields, sticker_row_fields,
};
use crate::domain::media::AudioKind;
use crate::domain::message::{MessagePreviewKind, ReactionSend, SendState};

macro_rules! declare_accessors {
    ($f:ident $set:ident text) => {
        fn $f(&self) -> &str;
        fn $set(&mut self, value: SharedString);
    };
    ($f:ident $set:ident enumk($ty:ident)) => {
        fn $f(&self) -> &str;
        fn $set(&mut self, value: $ty);
    };
    ($f:ident $set:ident int) => { declare_accessors!(@same $f $set i32); };
    ($f:ident $set:ident ratio) => { declare_accessors!(@same $f $set f32); };
    ($f:ident $set:ident flag) => { declare_accessors!(@same $f $set bool); };
    ($f:ident $set:ident list) => { declare_accessors!(@same $f $set Vec<SharedString>); };
    ($f:ident $set:ident floats) => { declare_accessors!(@same $f $set Vec<f32>); };
    ($f:ident $set:ident image) => { declare_accessors!(@same $f $set Image); };
    ($f:ident $set:ident styled) => { declare_accessors!(@same $f $set StyledText); };
    ($f:ident $set:ident structs($row:ident)) => {
        declare_accessors!(@same $f $set ModelRc<B::$row>);
    };
    (@same $f:ident $set:ident $ty:ty) => {
        fn $f(&self) -> $ty;
        fn $set(&mut self, value: $ty);
    };
}

macro_rules! declare_fields {
    ($name:ident; $($f:ident $set:ident $lit:literal $k:ident $(($arg:ident))?;)*) => {
        #[allow(dead_code)]
        pub trait $name<B: UiBackend> {
            $( declare_accessors!($f $set $k $(($arg))?); )*
        }
    };
}

message_fields!(declare_fields MessageFields;);
reaction_fields!(declare_fields ReactionFields;);
reactor_fields!(declare_fields ReactorFields;);
room_fields!(declare_fields RoomFields;);
space_fields!(declare_fields SpaceFields;);
sticker_cell_fields!(declare_fields StickerCellFields;);
sticker_pack_fields!(declare_fields StickerPackFields;);
sticker_row_fields!(declare_fields StickerRowFields;);
