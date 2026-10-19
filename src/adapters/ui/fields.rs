use slint::{Image, ModelRc, SharedString, StyledText};

use super::backend::UiBackend;
use super::dto::{
    CallTileMark, EmojiArt, MediaFailureKind, MediaState, MemberRowKind, RoomCallMark, RoomRowKind,
    SpoilerState,
};
use super::present::{Delivery, MessageKind, PollPhase, ServiceKind, ShieldReason};
use super::schema::{
    body_line_fields, body_piece_fields, call_tile_fields, invitee_fields, log_line_fields,
    member_row_fields, message_fields, pack_cell_fields, pack_row_fields, pack_tab_fields,
    poll_answer_fields, reaction_fields, reactor_fields, room_fields, space_child_fields,
    space_fields, user_device_fields,
};
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{ChildAccess, InviteeState};
use crate::domain::image_pack::PackOrigin;
use crate::domain::invite::InviteKind;
use crate::domain::media::AudioKind;
use crate::domain::message::{
    DecryptionFailure, MessagePreviewKind, ReactionSend, SendState, Shield,
};
use crate::domain::room_info::MemberRole;
use crate::domain::room_log::LogLevel;
use crate::domain::user_info::DeviceTrust;

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
body_line_fields!(declare_fields BodyLineFields;);
body_piece_fields!(declare_fields BodyPieceFields;);
reaction_fields!(declare_fields ReactionFields;);
reactor_fields!(declare_fields ReactorFields;);
poll_answer_fields!(declare_fields PollAnswerFields;);
room_fields!(declare_fields RoomFields;);
space_fields!(declare_fields SpaceFields;);
space_child_fields!(declare_fields SpaceChildFields;);
member_row_fields!(declare_fields MemberRowFields;);
invitee_fields!(declare_fields InviteeFields;);
log_line_fields!(declare_fields LogLineFields;);
call_tile_fields!(declare_fields CallTileFields;);
user_device_fields!(declare_fields UserDeviceFields;);
pack_cell_fields!(declare_fields PackCellFields;);
pack_tab_fields!(declare_fields PackTabFields;);
pack_row_fields!(declare_fields PackRowFields;);
