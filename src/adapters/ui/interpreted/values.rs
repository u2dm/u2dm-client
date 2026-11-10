use slint::{Image, Model, ModelRc, StyledText, VecModel};
use slint_interpreter::{SharedString, Struct, Value};

use super::InterpretedBackend;
use super::names::{composer_draft, draft_verdict, mention_word, text_splice};
use crate::adapters::ui::dto::{
    BodyLineDto, BodyPieceDto, CallConnectionDto, CallDeviceDto, CallNoticeMark, CallOfferMark,
    CallPersonDto, CallScreenMark, CallTileDto, CallTileMark, EmojiArt, IgnoredPersonDto,
    InviteeDto, KeywordDto, LogLineDto, MediaFailureKind, MediaState, MemberRowDto, MemberRowKind,
    MessageDto, PackCellDto, PackRowDto, PackSurface, PackTabDto, PollAnswerDto, ReactionDto,
    ReactorAvatarDto, RoomCallMark, RoomDto, RoomRowKind, SheetPreviewMark, SpaceChildDto,
    SpaceDto, SpoilerState, TestPhaseMark, UserDeviceDto,
};
use crate::adapters::ui::fields::{
    BodyLineFields, BodyPieceFields, CallConnectionFields, CallDeviceFields, CallPersonFields,
    CallTileFields, IgnoredPersonFields, InviteeFields, KeywordFields, LogLineFields,
    MemberRowFields, MessageFields, PackCellFields, PackRowFields, PackTabFields, PollAnswerFields,
    ReactionFields, ReactorFields, RoomFields, SpaceChildFields, SpaceFields, UserDeviceFields,
};
use crate::adapters::ui::mention;
use crate::adapters::ui::present::{
    Delivery, IdentityAlertKind, MessageKind, PollPhase, ServiceKind, ShieldReason, VerifyStep,
};
use crate::adapters::ui::router::DraftVerdict;
use crate::adapters::ui::schema::{
    body_line_fields, body_piece_fields, call_connection_fields, call_device_fields,
    call_person_fields, call_tile_fields, ignored_person_fields, invitee_fields, keyword_fields,
    log_line_fields, member_row_fields, message_fields, pack_cell_fields, pack_row_fields,
    pack_tab_fields, poll_answer_fields, reaction_fields, reactor_fields, room_fields, slint_enums,
    space_child_fields, space_fields, user_device_fields,
};
use crate::commands::effects::VerificationActivity;
use crate::commands::messages::UserMessageKind;
use crate::commands::view::{
    AccountActivity, AccountRule, AccountStep, AttachmentKind, AuthStage, CallPhase, CameraView,
    ChildAccess, CreateRoomStatus, DeviceStatus, DeviceTrouble, DirectChat, DownloadedMedia,
    EncryptionActivity, EncryptionStep, HandView, InviteeState, JoinRefusal, LinkView,
    LoginActivity, LoginStep, MicrophoneView, PendingAnswer, PendingModeration, PeopleSearchStatus,
    PreviewTrouble, ReactionView, ReadersStatus, RoomInfoPlacement, RoomPreviewSheet, RoomScope,
    RosterStatus, SettingsLayout, SettingsPage, SourceState, SpaceIndexStatus, UpgradeLink,
    VerifyOffer,
};
use crate::domain::auth::LoginMethod;
use crate::domain::call::devices::DeviceKind;
use crate::domain::call::video::ScreenRefusal;
use crate::domain::departure::RemovalKind;
use crate::domain::encryption::{Recovery, Standing};
use crate::domain::image_pack::PackOrigin;
use crate::domain::invite::InviteKind;
use crate::domain::media::AudioKind;
use crate::domain::message::{
    DecryptionFailure, MessagePreviewKind, ReactionSend, SendState, Shield,
};
use crate::domain::presence::PresenceLine;
use crate::domain::room::NotifyMode;
use crate::domain::room_creation::{RoomAccess, RoomKind};
use crate::domain::room_info::MemberRole;
use crate::domain::room_log::LogLevel;
use crate::domain::server_rules::{RoomClass, RoomDefault};
use crate::domain::settings::{
    CallLayout, MotionChoice, ReceiptVisibility, SendKey, SettingFlag, ThemeChoice,
};
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::{SourceEncryption, TimelineStatus};
use crate::domain::user_info::{DeviceTrust, IdentityTrust, RoomMembership};

pub(super) trait SlintEnum {
    fn slint(&self) -> (&'static str, &'static str);
}

impl<E: SlintEnum> SlintEnum for &E {
    fn slint(&self) -> (&'static str, &'static str) {
        E::slint(self)
    }
}

pub(super) fn enum_value(value: &impl SlintEnum) -> Value {
    let (enumeration, variant) = value.slint();
    Value::EnumerationValue(enumeration.to_string(), variant.to_string())
}

macro_rules! impl_slint_enum {
    ($src:ident $dst:ident;
        $($rust:ident $(($($p:tt)*))? $({$($b:tt)*})? $ui:ident $lit:literal;)*) => {
        impl SlintEnum for $src {
            fn slint(&self) -> (&'static str, &'static str) {
                (stringify!($dst), match self { $($src::$rust $(($($p)*))? $({$($b)*})? => $lit,)* })
            }
        }
    };
}

slint_enums!(impl_slint_enum);

pub(super) fn draft_verdict_value(verdict: &DraftVerdict) -> Value {
    struct_value([
        (draft_verdict::ACCEPTED, Value::Bool(verdict.accepted())),
        (draft_verdict::SHORTEN_BY, num(verdict.shorten_by())),
    ])
}

pub(super) fn no_draft_value() -> Value {
    struct_value([(composer_draft::HELD, Value::Bool(false))])
}

pub(super) fn text_splice_value(text: String, caret: i32) -> Value {
    struct_value([
        (text_splice::TEXT, text_value(text)),
        (text_splice::CARET, num(caret)),
    ])
}

pub(super) fn mention_word_value(text: &str, caret: i32) -> Value {
    let (start, query) = mention::word_at(text, caret);
    word_value(start, query)
}

pub(super) fn word_value(start: i32, query: String) -> Value {
    struct_value([
        (mention_word::START, num(start)),
        (mention_word::QUERY, text_value(query)),
    ])
}

pub(super) fn struct_value<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Struct(
        fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

pub(super) fn text_value(text: impl Into<SharedString>) -> Value {
    Value::String(text.into())
}

pub(super) fn model_value(values: Vec<Value>) -> Value {
    Value::Model(ModelRc::new(VecModel::from(values)))
}

pub(super) fn num(value: i32) -> Value {
    Value::Number(f64::from(value))
}

fn ratio(value: f32) -> Value {
    Value::Number(value.into())
}

fn string_list(items: Vec<SharedString>) -> Value {
    model_value(items.into_iter().map(Value::String).collect())
}

fn float_list(items: Vec<f32>) -> Value {
    model_value(items.into_iter().map(ratio).collect())
}

fn struct_list<T>(items: Vec<T>) -> Value
where
    Value: From<T>,
{
    model_value(items.into_iter().map(Value::from).collect())
}

macro_rules! field_value {
    ($s:ident, $lit:literal, $val:expr, text) => {
        $s.set_field($lit.to_string(), Value::String($val));
    };
    ($s:ident, $lit:literal, $val:expr, int) => {
        $s.set_field($lit.to_string(), num($val));
    };
    ($s:ident, $lit:literal, $val:expr, ratio) => {
        $s.set_field($lit.to_string(), ratio($val));
    };
    ($s:ident, $lit:literal, $val:expr, flag) => {
        $s.set_field($lit.to_string(), Value::Bool($val));
    };
    ($s:ident, $lit:literal, $val:expr, list) => {
        $s.set_field($lit.to_string(), string_list($val));
    };
    ($s:ident, $lit:literal, $val:expr, floats) => {
        $s.set_field($lit.to_string(), float_list($val));
    };
    ($s:ident, $lit:literal, $val:expr, structs) => {
        $s.set_field($lit.to_string(), struct_list($val));
    };
    ($s:ident, $lit:literal, $val:expr, image) => {
        if let Some(img) = $val {
            $s.set_field($lit.to_string(), Value::Image(img));
        }
    };
    ($s:ident, $lit:literal, $val:expr, enumk) => {
        $s.set_field($lit.to_string(), enum_value(&$val));
    };
    ($s:ident, $lit:literal, $val:expr, styled) => {
        $s.set_field($lit.to_string(), Value::StyledText($val));
    };
}

macro_rules! value_accessors {
    ($f:ident $set:ident $lit:literal text) => {
        value_accessors!(@with $f $set $lit &str, SharedString, text_of, Value::String);
    };
    ($f:ident $set:ident $lit:literal int) => {
        value_accessors!(@with $f $set $lit i32, i32, int_of, num);
    };
    ($f:ident $set:ident $lit:literal ratio) => {
        value_accessors!(@with $f $set $lit f32, f32, float_of, ratio);
    };
    ($f:ident $set:ident $lit:literal flag) => {
        value_accessors!(@with $f $set $lit bool, bool, flag_of, Value::Bool);
    };
    ($f:ident $set:ident $lit:literal list) => {
        value_accessors!(@with $f $set $lit
            Vec<SharedString>, Vec<SharedString>, strings_of, string_list);
    };
    ($f:ident $set:ident $lit:literal floats) => {
        value_accessors!(@with $f $set $lit Vec<f32>, Vec<f32>, floats_of, float_list);
    };
    ($f:ident $set:ident $lit:literal image) => {
        value_accessors!(@with $f $set $lit Image, Image, image_of, Value::Image);
    };
    ($f:ident $set:ident $lit:literal styled) => {
        value_accessors!(@with $f $set $lit
            StyledText, StyledText, styled_of, Value::StyledText);
    };
    ($f:ident $set:ident $lit:literal structs($row:ident)) => {
        value_accessors!(@with $f $set $lit
            ModelRc<Value>, ModelRc<Value>, model_of, Value::Model);
    };
    ($f:ident $set:ident $lit:literal enumk($ty:ident)) => {
        fn $f(&self) -> &str { variant_of(field_of(self, $lit)) }
        fn $set(&mut self, value: $ty) { put_field(self, $lit, enum_value(&value)); }
    };
    (@with $f:ident $set:ident $lit:literal $out:ty, $in:ty, $read:ident, $write:path) => {
        fn $f(&self) -> $out { $read(field_of(self, $lit)) }
        fn $set(&mut self, value: $in) { put_field(self, $lit, $write(value)); }
    };
}

macro_rules! impl_value {
    ($dto:ident $fields:ident; $($f:ident $set:ident $lit:literal $k:ident $(($arg:ident))?;)*) => {
        impl From<$dto> for Value {
            fn from(d: $dto) -> Self {
                let mut fields = Struct::default();
                $( field_value!(fields, $lit, d.$f, $k); )*
                Value::Struct(fields)
            }
        }

        impl $fields<InterpretedBackend> for Value {
            $( value_accessors!($f $set $lit $k $(($arg))?); )*
        }
    };
}

message_fields!(impl_value MessageDto MessageFields;);

body_line_fields!(impl_value BodyLineDto BodyLineFields;);

body_piece_fields!(impl_value BodyPieceDto BodyPieceFields;);

reaction_fields!(impl_value ReactionDto ReactionFields;);

reactor_fields!(impl_value ReactorAvatarDto ReactorFields;);

poll_answer_fields!(impl_value PollAnswerDto PollAnswerFields;);

room_fields!(impl_value RoomDto RoomFields;);

space_fields!(impl_value SpaceDto SpaceFields;);

space_child_fields!(impl_value SpaceChildDto SpaceChildFields;);

member_row_fields!(impl_value MemberRowDto MemberRowFields;);

invitee_fields!(impl_value InviteeDto InviteeFields;);

log_line_fields!(impl_value LogLineDto LogLineFields;);

call_tile_fields!(impl_value CallTileDto CallTileFields;);

call_person_fields!(impl_value CallPersonDto CallPersonFields;);

call_connection_fields!(impl_value CallConnectionDto CallConnectionFields;);

call_device_fields!(impl_value CallDeviceDto CallDeviceFields;);

user_device_fields!(impl_value UserDeviceDto UserDeviceFields;);

keyword_fields!(impl_value KeywordDto KeywordFields;);

ignored_person_fields!(impl_value IgnoredPersonDto IgnoredPersonFields;);

pack_cell_fields!(impl_value PackCellDto PackCellFields;);

pack_tab_fields!(impl_value PackTabDto PackTabFields;);

pack_row_fields!(impl_value PackRowDto PackRowFields;);

fn field_of<'a>(row: &'a Value, name: &str) -> Option<&'a Value> {
    match row {
        Value::Struct(fields) => fields.get_field(name),
        _ => None,
    }
}

fn put_field(row: &mut Value, name: &str, value: Value) {
    if let Value::Struct(fields) = row {
        fields.set_field(name.to_string(), value);
    }
}

pub(super) fn text_of(value: Option<&Value>) -> &str {
    match value {
        Some(Value::String(text)) => text.as_str(),
        _ => "",
    }
}

pub(super) fn variant_of(value: Option<&Value>) -> &str {
    match value {
        Some(Value::EnumerationValue(_, variant)) => variant.as_str(),
        _ => "",
    }
}

pub(super) fn int_of(value: Option<&Value>) -> i32 {
    match value {
        Some(Value::Number(n)) if n.is_finite() && n.fract() == 0.0 => {
            n.to_string().parse().unwrap_or_default()
        }
        _ => 0,
    }
}

fn float_of(value: Option<&Value>) -> f32 {
    match value {
        Some(Value::Number(n)) => n.to_string().parse().unwrap_or_default(),
        _ => 0.0,
    }
}

pub(super) fn flag_of(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Bool(true)))
}

fn image_of(value: Option<&Value>) -> Image {
    match value {
        Some(Value::Image(image)) => image.clone(),
        _ => Image::default(),
    }
}

fn styled_of(value: Option<&Value>) -> StyledText {
    match value {
        Some(Value::StyledText(styled)) => styled.clone(),
        _ => StyledText::default(),
    }
}

pub(super) fn model_of(value: Option<&Value>) -> ModelRc<Value> {
    match value {
        Some(Value::Model(model)) => model.clone(),
        _ => ModelRc::default(),
    }
}

pub(super) fn strings_of(value: Option<&Value>) -> Vec<SharedString> {
    model_of(value)
        .iter()
        .map(|item| SharedString::from(text_of(Some(&item))))
        .collect()
}

fn floats_of(value: Option<&Value>) -> Vec<f32> {
    model_of(value)
        .iter()
        .map(|item| float_of(Some(&item)))
        .collect()
}
