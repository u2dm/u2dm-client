use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::{Image, Model, ModelRc, Rgb8Pixel, SharedPixelBuffer, VecModel};
use slint_interpreter::{
    Compiler, ComponentHandle, ComponentInstance, SharedString, Struct, Value,
};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};

use names::{
    callback, emoji_entry, emoji_group, emoji_insert, emoji_store, message, reaction, reactor,
    room, space, sticker_cell, sticker_pack, sticker_row, user_message, verification_emoji,
};

use super::backend::{self, Models, UiBackend, reorder_spaces, selected_room_key};
use super::decode::{AvatarSlot, request_avatar, request_media, request_sticker};
use super::dto::{
    AudioRowUpdate, MediaFailureKind, MediaState, MessageDto, ReactionDto, ReactorAvatarDto,
    RoomDto, SpaceDto, StickerCellDto, StickerPackDto, StickerRowDto, ThumbUpdate,
    enrich_to_update,
};
use super::present::{MessageKind, ServiceKind, VerifyStep};
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::schema::{
    attachment_kinds, audio_kinds, connection_states, enum_props, login_activities, login_methods,
    login_phases, media_failures, media_states, message_fields, message_kinds, model_props,
    preview_kinds, reaction_fields, reaction_sends, reactor_fields, room_fields, send_states,
    service_kinds, simple_callbacks, space_fields, sticker_cell_fields, sticker_pack_fields,
    sticker_row_fields, timeline_states, user_message_kinds, verification_activities,
    verification_phases,
};
use super::video::{self, millis_to_duration};
use super::{audio, emoji, router};
use crate::app::input::CommandSender;
use crate::commands::effects::{Effect, VerificationActivity};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::ViewportChanged;
use crate::commands::view::{AppViewState, AttachmentKind, LoginActivity, LoginStep};
use crate::domain::auth::LoginMethod;
use crate::domain::media::AudioKind;
use crate::domain::message::{MessagePreviewKind, ReactionSend, SendState};
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::{EnrichmentDelta, TimelineStatus};
use crate::domain::verification::VerificationEmoji as DomainVerificationEmoji;
use crate::error::{AppError, Result};
use crate::ports::media::MediaCache;

#[allow(dead_code)]
mod names {
    pub mod callback {
        pub const GLOBAL: &str = "Actions";
        pub const MOVE_SPACE: &str = "move-space";
        pub const TOGGLE_REACTION: &str = "toggle-reaction";
        pub const REQUEST_MEDIA: &str = "request-media";
        pub const REQUEST_ROOM_AVATAR: &str = "request-room-avatar";
        pub const REQUEST_STICKER: &str = "request-sticker";
        pub const TOGGLE_VIDEO: &str = "toggle-video";
        pub const TOGGLE_VIDEO_MUTED: &str = "toggle-video-muted";
        pub const SEEK_VIDEO: &str = "seek-video";
        pub const TOGGLE_AUDIO: &str = "toggle-audio";
        pub const SEEK_AUDIO: &str = "seek-audio";
        pub const SEARCH_STICKERS: &str = "search-stickers";
        pub const SCROLL_POSITION_CHANGED: &str = "scroll-position-changed";
    }

    pub mod emoji_store {
        pub const NAME: &str = "EmojiStore";
        pub const GROUPS: &str = "groups";
        pub const RESULTS: &str = "results";
        pub const SEARCH: &str = "search";
        pub const INSERT: &str = "insert";
    }

    pub mod message {
        use crate::adapters::ui::schema::{gen_consts, message_fields};
        message_fields!(gen_consts);
    }

    pub mod reaction {
        use crate::adapters::ui::schema::{gen_consts, reaction_fields};
        reaction_fields!(gen_consts);
    }

    pub mod reactor {
        use crate::adapters::ui::schema::{gen_consts, reactor_fields};
        reactor_fields!(gen_consts);
    }

    pub mod room {
        use crate::adapters::ui::schema::{gen_consts, room_fields};
        room_fields!(gen_consts);
    }

    pub mod space {
        use crate::adapters::ui::schema::{gen_consts, space_fields};
        space_fields!(gen_consts);
    }

    pub mod sticker_cell {
        use crate::adapters::ui::schema::{gen_consts, sticker_cell_fields};
        sticker_cell_fields!(gen_consts);
    }

    pub mod sticker_pack {
        use crate::adapters::ui::schema::{gen_consts, sticker_pack_fields};
        sticker_pack_fields!(gen_consts);
    }

    pub mod sticker_row {
        use crate::adapters::ui::schema::{gen_consts, sticker_row_fields};
        sticker_row_fields!(gen_consts);
    }

    pub mod emoji_entry {
        pub const BASE: &str = "base";
        pub const TONES: &str = "tones";
        pub const NAME: &str = "name";
    }

    pub mod emoji_group {
        pub const ITEMS: &str = "items";
    }

    pub mod emoji_insert {
        pub const TEXT: &str = "text";
        pub const CARET: &str = "caret";
    }

    pub mod user_message {
        pub const KIND: &str = "kind";
        pub const DETAIL: &str = "detail";
    }

    pub mod verification_emoji {
        pub const SYMBOL: &str = "symbol";
        pub const DESCRIPTION: &str = "description";
    }
}

fn set_prop(inst: &ComponentInstance, name: &str, value: Value) {
    if let Err(e) = inst.set_property(name, value) {
        tracing::warn!("failed to set property '{name}': {e:?}");
    }
}

fn set_global_prop(inst: &ComponentInstance, global: &str, name: &str, value: Value) -> Result<()> {
    inst.set_global_property(global, name, value)
        .map_err(|e| AppError::Ui(format!("{e:?}")))
}

fn set_global(inst: &ComponentInstance, global: &str, name: &str, value: Value) {
    if let Err(e) = inst.set_global_property(global, name, value) {
        tracing::warn!("failed to set property '{global}.{name}': {e:?}");
    }
}

trait SlintEnum {
    fn slint(&self) -> (&'static str, &'static str);
}

impl<E: SlintEnum> SlintEnum for &E {
    fn slint(&self) -> (&'static str, &'static str) {
        E::slint(self)
    }
}

fn enum_value(value: &impl SlintEnum) -> Value {
    let (enumeration, variant) = value.slint();
    Value::EnumerationValue(enumeration.to_string(), variant.to_string())
}

macro_rules! impl_slint_enum {
    ($src:ident $name:literal;
        $($rust:ident $(($($p:tt)*))? $({$($b:tt)*})? $ui:ident $lit:literal;)*) => {
        impl SlintEnum for $src {
            fn slint(&self) -> (&'static str, &'static str) {
                ($name, match self { $($src::$rust $(($($p)*))? $({$($b)*})? => $lit,)* })
            }
        }
    };
}

login_phases!(impl_slint_enum LoginStep "LoginPhase";);
login_activities!(impl_slint_enum LoginActivity "LoginActivity";);
login_methods!(impl_slint_enum LoginMethod "LoginMethodKind";);
connection_states!(impl_slint_enum ConnectionStatus "ConnectionState";);
timeline_states!(impl_slint_enum TimelineStatus "TimelineState";);
verification_phases!(impl_slint_enum VerifyStep "VerificationPhase";);
verification_activities!(impl_slint_enum VerificationActivity "VerificationActivity";);
user_message_kinds!(impl_slint_enum UserMessageKind "UserMessageKind";);
media_states!(impl_slint_enum MediaState "MediaState";);
send_states!(impl_slint_enum SendState "SendState";);
reaction_sends!(impl_slint_enum ReactionSend "ReactionSend";);
media_failures!(impl_slint_enum MediaFailureKind "MediaFailure";);
message_kinds!(impl_slint_enum MessageKind "MessageKind";);
attachment_kinds!(impl_slint_enum AttachmentKind "AttachmentKind";);
preview_kinds!(impl_slint_enum MessagePreviewKind "PreviewKind";);
audio_kinds!(impl_slint_enum AudioKind "AudioKind";);
service_kinds!(impl_slint_enum ServiceKind "ServiceKind";);

fn string_arg(args: &[Value], index: usize) -> String {
    args.get(index)
        .and_then(|v| match v {
            Value::String(s) => Some(s.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn bool_arg(args: &[Value], index: usize) -> bool {
    args.get(index)
        .and_then(|v| match v {
            Value::Bool(b) => Some(*b),
            _ => None,
        })
        .unwrap_or_default()
}

fn usize_arg(args: &[Value], index: usize) -> Option<usize> {
    match args.get(index) {
        Some(Value::Number(n))
            if n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && *n <= u32::MAX.into() =>
        {
            n.to_string().parse().ok()
        }
        _ => None,
    }
}

fn struct_arg(args: &[Value], index: usize) -> Option<&Struct> {
    match args.get(index) {
        Some(Value::Struct(s)) => Some(s),
        _ => None,
    }
}

fn flag(s: &Struct, name: &str) -> bool {
    s.get_field(name)
        .and_then(|v| match v {
            Value::Bool(b) => Some(*b),
            _ => None,
        })
        .unwrap_or_default()
}

fn field(s: &Struct, name: &str) -> String {
    s.get_field(name)
        .and_then(|v| match v {
            Value::String(s) => Some(s.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn bind(
    inst: &ComponentInstance,
    global: &str,
    name: &str,
    handler: impl Fn(&[Value]) -> Value + 'static,
) -> Result<()> {
    inst.set_global_callback(global, name, handler)
        .map_err(|e| AppError::Ui(format!("{e:?}")))
}

fn bind_action(
    inst: &ComponentInstance,
    name: &str,
    handler: impl Fn(&[Value]) -> Value + 'static,
) -> Result<()> {
    bind(inst, callback::GLOBAL, name, handler)
}

macro_rules! bind_interpreted_callbacks {
    ($inst:expr, $tx:ident;
        $($on:ident $lit:literal $fn:ident $kind:ident $(($($arg:tt)*))? $cmd:ident;)*) => {
        $( bind_interpreted_callbacks!(@one $inst, $tx, $lit, $fn, $kind $(($($arg)*))?)?; )*
    };
    (@one $inst:expr, $tx:ident, $lit:literal, $fn:ident, plain) => {
        bind_interpreted_callbacks!(@unit $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $tx:ident, $lit:literal, $fn:ident, pass) => {
        bind_interpreted_callbacks!(@string $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $tx:ident, $lit:literal, $fn:ident, room) => {
        bind_interpreted_callbacks!(@string $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $tx:ident, $lit:literal, $fn:ident, opt_room) => {
        bind_interpreted_callbacks!(@string $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $tx:ident, $lit:literal, $fn:ident, manual_string) => {
        bind_interpreted_callbacks!(@string $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $tx:ident, $lit:literal, $fn:ident, room_key) => {{
        let tx = $tx.clone();
        let weak = $inst.as_weak();
        bind_action($inst, $lit, move |_args| {
            router::$fn(&tx, selected_room_key::<InterpretedBackend>(&weak));
            Value::Void
        })
    }};
    (@one $inst:expr, $tx:ident, $lit:literal, $fn:ident,
        request($($field:ident $name:literal $field_kind:ident),*)) => {{
        let tx = $tx.clone();
        bind_action($inst, $lit, move |args| {
            let Some(s) = struct_arg(args, 0) else {
                return Value::Void;
            };
            router::$fn(&tx, $(bind_interpreted_callbacks!(@field s $name $field_kind)),*);
            Value::Void
        })
    }};
    (@unit $inst:expr, $tx:ident, $lit:literal, $fn:ident) => {{
        let tx = $tx.clone();
        bind_action($inst, $lit, move |_args| { router::$fn(&tx); Value::Void })
    }};
    (@string $inst:expr, $tx:ident, $lit:literal, $fn:ident) => {{
        let tx = $tx.clone();
        bind_action($inst, $lit, move |args| {
            router::$fn(&tx, string_arg(args, 0));
            Value::Void
        })
    }};
    (@field $s:ident $name:literal text) => { field($s, $name) };
    (@field $s:ident $name:literal flag) => { flag($s, $name) };
}

macro_rules! impl_enum_setters {
    ($($fn:ident($ty:ty) $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        $( fn $fn(&self, value: $ty) { set_global(self, $gname, $lit, enum_value(&value)); } )*
    };
}

impl UiProps for ComponentInstance {
    fn set_string(&self, prop: StringProp, value: SharedString) {
        set_global(self, prop.global(), prop.as_str(), Value::String(value));
    }

    fn set_bool(&self, prop: BoolProp, value: bool) {
        set_global(self, prop.global(), prop.as_str(), Value::Bool(value));
    }

    fn set_int(&self, prop: IntProp, value: i32) {
        set_global(
            self,
            prop.global(),
            prop.as_str(),
            Value::Number(value.into()),
        );
    }

    enum_props!(impl_enum_setters);

    fn apply_video_frame(&self, buffer: SharedPixelBuffer<Rgb8Pixel>) {
        set_global(self, "VideoView", "frame", Value::Image(Image::from_rgb8(buffer)));
        set_global(self, "VideoView", "has-frame", Value::Bool(true));
    }

    fn clear_video_frame(&self) {
        set_global(self, "VideoView", "frame", Value::Image(Image::default()));
        set_global(self, "VideoView", "has-frame", Value::Bool(false));
    }

    fn get_string(&self, prop: StringProp) -> SharedString {
        self.get_global_property(prop.global(), prop.as_str())
            .ok()
            .and_then(|v| match v {
                Value::String(s) => Some(s),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn get_int(&self, prop: IntProp) -> i32 {
        match self.get_global_property(prop.global(), prop.as_str()) {
            Ok(Value::Number(n)) if n.is_finite() && n.fract() == 0.0 => {
                n.to_string().parse().unwrap_or_default()
            }
            _ => 0,
        }
    }

    fn apply_user_avatar(&self, avatar: Option<slint::Image>) {
        match avatar {
            Some(img) => {
                set_global(self, "SessionView", "user-avatar", Value::Image(img));
                set_global(self, "SessionView", "user-has-avatar", Value::Bool(true));
            }
            None => set_global(self, "SessionView", "user-has-avatar", Value::Bool(false)),
        }
    }

    fn apply_attachment_preview(&self, preview: Option<slint::Image>) {
        match preview {
            Some(img) => {
                set_global(self, "AttachmentView", "preview", Value::Image(img));
                set_global(self, "AttachmentView", "has-preview", Value::Bool(true));
            }
            None => set_global(self, "AttachmentView", "has-preview", Value::Bool(false)),
        }
    }

    fn apply_login_messages(&self, messages: &[UserMessage]) {
        let entries: Vec<Value> = messages
            .iter()
            .map(|m| {
                Value::Struct(Struct::from_iter([
                    (user_message::KIND.to_string(), enum_value(&m.kind)),
                    (
                        user_message::DETAIL.to_string(),
                        Value::String(SharedString::from(&m.detail)),
                    ),
                ]))
            })
            .collect();
        set_global(
            self,
            "LoginView",
            "messages",
            Value::Model(ModelRc::new(VecModel::from(entries))),
        );
    }

    fn apply_emoji_model(&self, emojis: &[DomainVerificationEmoji]) {
        let entries: Vec<Value> = emojis
            .iter()
            .map(|e| {
                Value::Struct(Struct::from_iter([
                    (
                        verification_emoji::SYMBOL.to_string(),
                        Value::String(SharedString::from(&e.symbol)),
                    ),
                    (
                        verification_emoji::DESCRIPTION.to_string(),
                        Value::String(SharedString::from(&e.description)),
                    ),
                ]))
            })
            .collect();
        set_global(
            self,
            "VerificationView",
            "emojis",
            Value::Model(ModelRc::new(VecModel::from(entries))),
        );
    }

    fn clear_emoji_model(&self) {
        set_global(
            self,
            "VerificationView",
            "emojis",
            Value::Model(ModelRc::new(VecModel::<Value>::default())),
        );
    }

    fn clear_text_inputs(&self) {
        set_prop(
            self,
            "input-username",
            Value::String(SharedString::default()),
        );
        set_prop(
            self,
            "input-message",
            Value::String(SharedString::default()),
        );
    }
}

fn set_value_avatar(entry: &mut Value, avatar_field: &str, has_field: &str, image: &Image) {
    if let Value::Struct(s) = entry {
        s.set_field(avatar_field.to_string(), Value::Image(image.clone()));
        s.set_field(has_field.to_string(), Value::Bool(true));
    }
}

macro_rules! attach_interpreted_models {
    ($window:ident $models:ident;
        $($field:ident $row:ident $model:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        $( set_global($window, $gname, $lit, Value::Model(ModelRc::from(Rc::clone(&$models.$field)))); )*
    };
}

pub struct InterpretedBackend;

impl UiBackend for InterpretedBackend {
    type Window = ComponentInstance;
    type Message = Value;
    type Room = Value;
    type Space = Value;
    type StickerRow = Value;
    type StickerPack = Value;

    fn attach_models(window: &ComponentInstance, models: &Models<Self>) {
        model_props!(attach_interpreted_models window models;);
    }

    fn bind_sticker_search(window: &ComponentInstance, search: impl Fn(&str) + 'static) {
        let bound = bind_action(window, callback::SEARCH_STICKERS, move |args| {
            search(&string_arg(args, 0));
            Value::Void
        });
        if let Err(e) = bound {
            tracing::warn!("failed to bind the sticker search callback: {e}");
        }
    }

    fn enrich_message(value: &mut Value, delta: &EnrichmentDelta, media: &dyn MediaCache) {
        let Value::Struct(entry) = value else {
            return;
        };
        let update = enrich_to_update(delta, media);
        match update.thumbnail {
            ThumbUpdate::Ready(img) => {
                entry.set_field(message::THUMBNAIL.to_string(), Value::Image(img));
                entry.set_field(
                    message::MEDIA_STATE.to_string(),
                    enum_value(&MediaState::Ready),
                );
            }
            ThumbUpdate::Failed(reason) => {
                entry.set_field(
                    message::MEDIA_STATE.to_string(),
                    enum_value(&MediaState::Failed),
                );
                entry.set_field(message::MEDIA_FAILURE.to_string(), enum_value(&reason));
            }
            ThumbUpdate::Unchanged => {}
        }
        if let Some(img) = update.avatar {
            entry.set_field(message::AVATAR.to_string(), Value::Image(img));
            entry.set_field(message::HAS_AVATAR.to_string(), Value::Bool(true));
        }
        if let Some(pronouns) = update.pronouns {
            entry.set_field(message::PRONOUNS.to_string(), string_list(pronouns));
        }
    }

    fn message_id(entry: &Value) -> &str {
        entry_id_from_value(entry).map_or("", SharedString::as_str)
    }

    fn message_event_id(entry: &Value) -> &str {
        message_text_field(entry, message::EVENT_ID).map_or("", SharedString::as_str)
    }

    fn message_is_first_unread(entry: &Value) -> bool {
        matches!(entry, Value::Struct(s)
            if matches!(s.get_field(message::FIRST_UNREAD), Some(Value::Bool(true))))
    }

    fn room_id(entry: &Value) -> &str {
        room_id_from_value(entry).map_or("", SharedString::as_str)
    }

    fn space_id(entry: &Value) -> &str {
        room_id_from_value(entry).map_or("", SharedString::as_str)
    }

    fn sticker_pack_with_icon(pack: &Value, pack_id: &str, image: &Image) -> Option<Value> {
        let Value::Struct(fields) = pack else {
            return None;
        };
        if !matches!(
            fields.get_field(sticker_pack::HAS_ICON),
            Some(Value::Bool(false))
        ) {
            return None;
        }
        match fields.get_field(sticker_pack::ID) {
            Some(Value::String(id)) if id == pack_id => {}
            _ => return None,
        }
        let mut updated = fields.clone();
        updated.set_field(sticker_pack::ICON.to_string(), Value::Image(image.clone()));
        updated.set_field(sticker_pack::HAS_ICON.to_string(), Value::Bool(true));
        Some(Value::Struct(updated))
    }

    fn patch_sticker_cell(row: &Value, key: &str, art: Option<&Image>) -> bool {
        let Value::Struct(fields) = row else {
            return false;
        };
        let Some(Value::Model(cells)) = fields.get_field(sticker_row::CELLS) else {
            return false;
        };
        let Some(index) = cells
            .iter()
            .position(|cell| cell_key_of(&cell).is_some_and(|k| k == key))
        else {
            return false;
        };
        let Some(model) = cells.as_any().downcast_ref::<VecModel<Value>>() else {
            return false;
        };
        let Some(Value::Struct(mut cell)) = model.row_data(index) else {
            return false;
        };
        match art {
            Some(art) => {
                cell.set_field(sticker_cell::IMAGE.to_string(), Value::Image(art.clone()));
                cell.set_field(
                    sticker_cell::MEDIA_STATE.to_string(),
                    enum_value(&MediaState::Ready),
                );
            }
            None => cell.set_field(
                sticker_cell::MEDIA_STATE.to_string(),
                enum_value(&MediaState::Failed),
            ),
        }
        model.set_row_data(index, Value::Struct(cell));
        true
    }

    fn patch_reactor_avatar(entry: &Value, user_id: &str, image: &Image) -> bool {
        let Value::Struct(fields) = entry else {
            return false;
        };
        let Some(Value::Model(reactions)) = fields.get_field(message::REACTIONS) else {
            return false;
        };
        let mut patched = false;
        for reaction in reactions.iter() {
            let Value::Struct(reaction) = reaction else {
                continue;
            };
            let Some(Value::Model(faces)) = reaction.get_field(reaction::AVATARS) else {
                continue;
            };
            let Some(index) = faces.iter().position(|face| unclaimed_face(&face, user_id)) else {
                continue;
            };
            let Some(model) = faces.as_any().downcast_ref::<VecModel<Value>>() else {
                continue;
            };
            let Some(Value::Struct(mut face)) = model.row_data(index) else {
                continue;
            };
            face.set_field(reactor::AVATAR.to_string(), Value::Image(image.clone()));
            face.set_field(reactor::HAS_AVATAR.to_string(), Value::Bool(true));
            model.set_row_data(index, Value::Struct(face));
            patched = true;
        }
        patched
    }

    fn set_message_avatar(entry: &mut Value, image: &Image) {
        set_value_avatar(entry, message::AVATAR, message::HAS_AVATAR, image);
    }

    fn set_room_avatar(entry: &mut Value, image: &Image) {
        set_value_avatar(entry, room::AVATAR, room::HAS_AVATAR, image);
    }

    fn set_space_avatar(entry: &mut Value, image: &Image) {
        set_value_avatar(entry, space::AVATAR, space::HAS_AVATAR, image);
    }

    fn set_message_thumbnail(entry: &mut Value, image: &Image) {
        if let Value::Struct(s) = entry {
            s.set_field(message::THUMBNAIL.to_string(), Value::Image(image.clone()));
            s.set_field(
                message::MEDIA_STATE.to_string(),
                enum_value(&MediaState::Ready),
            );
        }
    }

    fn set_message_audio(entry: &mut Value, update: &AudioRowUpdate) {
        if let Value::Struct(s) = entry {
            s.set_field(
                message::MEDIA_STATE.to_string(),
                enum_value(&update.media_state),
            );
            s.set_field(
                message::MEDIA_FAILURE.to_string(),
                enum_value(&update.media_failure),
            );
            s.set_field(
                message::WAVEFORM.to_string(),
                float_list(update.waveform.clone()),
            );
        }
    }

    fn set_message_media_failed(entry: &mut Value, reason: MediaFailureKind) {
        if let Value::Struct(s) = entry {
            s.set_field(
                message::MEDIA_STATE.to_string(),
                enum_value(&MediaState::Failed),
            );
            s.set_field(message::MEDIA_FAILURE.to_string(), enum_value(&reason));
        }
    }
}

pub struct SlintUiAdapter {
    instance: ComponentInstance,
}

impl SlintUiAdapter {
    pub fn compile(rt: &Runtime) -> Result<Self> {
        let instance = rt.block_on(async {
            let mut compiler = Compiler::new();
            compiler.set_library_paths(HashMap::from([(
                "lucide".to_string(),
                PathBuf::from(lucide_slint::lib()),
            )]));
            let result = compiler.build_from_path("ui/main.slint").await;
            for diag in result.diagnostics() {
                tracing::error!("slint: {diag}");
            }
            let def = result
                .component("AppWindow")
                .ok_or_else(|| AppError::Ui("failed to load ui/main.slint".into()))?;
            let inst = def.create().map_err(|e| AppError::Ui(e.to_string()))?;
            Ok::<_, AppError>(inst)
        })?;
        Ok(Self { instance })
    }

    fn bind_audio_callbacks(&self, cmd_tx: &CommandSender) -> Result<()> {
        audio::install_commands(cmd_tx);

        let weak = self.instance.as_weak();
        bind_action(&self.instance, callback::TOGGLE_AUDIO, move |_| {
            if let Some(window) = weak.upgrade() {
                audio::toggle(&window);
            }
            Value::Void
        })?;

        let weak = self.instance.as_weak();
        bind_action(&self.instance, callback::SEEK_AUDIO, move |args| {
            if let Some(window) = weak.upgrade() {
                audio::seek(&window, millis_to_duration(usize_arg(args, 0)));
            }
            Value::Void
        })
    }

    fn bind_decode_requests(&self) -> Result<()> {
        bind_action(&self.instance, callback::REQUEST_MEDIA, move |args| {
            request_media(&string_arg(args, 0));
            Value::Void
        })?;

        bind_action(&self.instance, callback::REQUEST_ROOM_AVATAR, move |args| {
            request_avatar(&AvatarSlot::Room(string_arg(args, 0)));
            Value::Void
        })?;

        bind_action(&self.instance, callback::REQUEST_STICKER, move |args| {
            request_sticker(&string_arg(args, 0));
            Value::Void
        })?;

        let weak = self.instance.as_weak();
        bind_action(&self.instance, callback::TOGGLE_VIDEO, move |_| {
            if let Some(window) = weak.upgrade() {
                video::toggle(&window);
            }
            Value::Void
        })?;

        let weak = self.instance.as_weak();
        bind_action(&self.instance, callback::TOGGLE_VIDEO_MUTED, move |_| {
            if let Some(window) = weak.upgrade() {
                video::toggle_muted(&window);
            }
            Value::Void
        })?;

        let weak = self.instance.as_weak();
        bind_action(&self.instance, callback::SEEK_VIDEO, move |args| {
            if let Some(window) = weak.upgrade() {
                video::seek(&window, millis_to_duration(usize_arg(args, 0)));
            }
            Value::Void
        })
    }

    pub fn register_callbacks(
        &self,
        cmd_tx: &CommandSender,
        scroll_tx: &watch::Sender<ViewportChanged>,
    ) -> Result<()> {
        setup_emoji_store(&self.instance)?;

        simple_callbacks!(bind_interpreted_callbacks &self.instance, cmd_tx;);

        let tx = cmd_tx.clone();
        bind_action(&self.instance, callback::MOVE_SPACE, move |args| {
            if let (Some(from), Some(to)) = (usize_arg(args, 0), usize_arg(args, 1)) {
                router::move_space(&tx, from, to, reorder_spaces::<InterpretedBackend>);
            }
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        bind_action(&self.instance, callback::TOGGLE_REACTION, move |args| {
            router::toggle_reaction(&tx, string_arg(args, 0), string_arg(args, 1));
            Value::Void
        })?;

        self.bind_decode_requests()?;
        self.bind_audio_callbacks(cmd_tx)?;

        let scroll_tx = scroll_tx.clone();
        let weak = self.instance.as_weak();
        bind_action(
            &self.instance,
            callback::SCROLL_POSITION_CHANGED,
            move |args| {
                router::scroll_position(
                    &scroll_tx,
                    selected_room_key::<InterpretedBackend>(&weak),
                    bool_arg(args, 0),
                );
                Value::Void
            },
        )
    }

    pub fn spawn_event_handler(
        &self,
        ui_rx: mpsc::Receiver<Effect>,
        view_rx: watch::Receiver<Arc<AppViewState>>,
        media_cache: Arc<dyn MediaCache>,
    ) {
        backend::spawn_event_handler::<InterpretedBackend>(
            &self.instance,
            ui_rx,
            view_rx,
            media_cache,
        );
    }

    pub fn run(&self) -> Result<()> {
        self.instance.run()?;
        Ok(())
    }

    #[cfg(feature = "demo")]
    pub fn set_window_size(&self, width: f32, height: f32) {
        self.instance
            .window()
            .set_size(slint::LogicalSize::new(width, height));
    }

    #[cfg(feature = "demo")]
    pub fn prefer_silent_audio() {
        audio::prefer_silent();
    }

    #[cfg(feature = "demo")]
    pub fn enable_probe_introspection(&self) {
        self.instance.set_bool(BoolProp::ProbeEnabled, true);
    }
}

#[cfg(feature = "demo")]
pub fn install_timeline_dump(_ui: &SlintUiAdapter) {}

fn emoji_entry_to_value(e: &emoji::EmojiEntry) -> Value {
    let tones: Vec<Value> = e
        .tones
        .iter()
        .map(|t| Value::String(SharedString::from(t.as_str())))
        .collect();

    Value::Struct(Struct::from_iter([
        (
            emoji_entry::BASE.to_string(),
            Value::String(SharedString::from(&e.base)),
        ),
        (
            emoji_entry::TONES.to_string(),
            Value::Model(ModelRc::new(VecModel::from(tones))),
        ),
        (
            emoji_entry::NAME.to_string(),
            Value::String(SharedString::from(&e.name)),
        ),
    ]))
}

fn emoji_groups_to_value() -> Value {
    let groups: Vec<Value> = emoji::groups()
        .iter()
        .map(|items| {
            let entries: Vec<Value> = items.iter().map(emoji_entry_to_value).collect();
            Value::Struct(Struct::from_iter([(
                emoji_group::ITEMS.to_string(),
                Value::Model(ModelRc::new(VecModel::from(entries))),
            )]))
        })
        .collect();

    Value::Model(ModelRc::new(VecModel::from(groups)))
}

fn emoji_search_results_to_value(query: &str) -> Value {
    let results: Vec<Value> = emoji::search(query)
        .iter()
        .map(emoji_entry_to_value)
        .collect();
    Value::Model(ModelRc::new(VecModel::from(results)))
}

fn setup_emoji_store(inst: &ComponentInstance) -> Result<()> {
    set_global_prop(
        inst,
        emoji_store::NAME,
        emoji_store::GROUPS,
        emoji_groups_to_value(),
    )?;

    let weak = inst.as_weak();
    inst.set_global_callback(
        emoji_store::NAME,
        emoji_store::SEARCH,
        move |args: &[Value]| {
            if let Some(inst) = weak.upgrade()
                && let Err(e) = inst.set_global_property(
                    emoji_store::NAME,
                    emoji_store::RESULTS,
                    emoji_search_results_to_value(&string_arg(args, 0)),
                )
            {
                tracing::warn!("failed to set EmojiStore.results: {e:?}");
            }
            Value::Void
        },
    )
    .map_err(|e| AppError::Ui(format!("{e:?}")))?;

    inst.set_global_callback(
        emoji_store::NAME,
        emoji_store::INSERT,
        move |args: &[Value]| {
            let text = string_arg(args, 0);
            let offset = args
                .get(1)
                .and_then(|v| match v {
                    Value::Number(n)
                        if n.is_finite()
                            && n.fract() == 0.0
                            && *n >= f64::from(i32::MIN)
                            && *n <= f64::from(i32::MAX) =>
                    {
                        n.to_string().parse().ok()
                    }
                    _ => None,
                })
                .unwrap_or_default();
            let glyph = string_arg(args, 2);
            let (inserted, caret) = emoji::insert_at(&text, offset, &glyph);
            Value::Struct(Struct::from_iter([
                (
                    emoji_insert::TEXT.to_string(),
                    Value::String(SharedString::from(inserted)),
                ),
                (
                    emoji_insert::CARET.to_string(),
                    Value::Number(f64::from(caret)),
                ),
            ]))
        },
    )
    .map_err(|e| AppError::Ui(format!("{e:?}")))?;

    Ok(())
}

fn num(value: i32) -> Value {
    Value::Number(f64::from(value))
}

fn string_list(items: Vec<SharedString>) -> Value {
    let values: Vec<Value> = items.into_iter().map(Value::String).collect();
    Value::Model(ModelRc::new(VecModel::from(values)))
}

fn float_list(items: Vec<f32>) -> Value {
    let values: Vec<Value> = items
        .into_iter()
        .map(|item| Value::Number(item.into()))
        .collect();
    Value::Model(ModelRc::new(VecModel::from(values)))
}

fn struct_list<T>(items: Vec<T>) -> Value
where
    Value: From<T>,
{
    Value::Model(ModelRc::new(
        items.into_iter().map(Value::from).collect::<VecModel<_>>(),
    ))
}

macro_rules! field_value {
    ($s:ident, $lit:literal, $val:expr, text) => {
        $s.set_field($lit.to_string(), Value::String($val));
    };
    ($s:ident, $lit:literal, $val:expr, int) => {
        $s.set_field($lit.to_string(), num($val));
    };
    ($s:ident, $lit:literal, $val:expr, ratio) => {
        $s.set_field($lit.to_string(), Value::Number($val.into()));
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

macro_rules! impl_value_from {
    ($dto:ident; $($f:ident $c:ident $lit:literal $k:ident;)*) => {
        impl From<$dto> for Value {
            fn from(d: $dto) -> Self {
                let mut fields = Struct::default();
                $( field_value!(fields, $lit, d.$f, $k); )*
                Value::Struct(fields)
            }
        }
    };
}

message_fields!(impl_value_from MessageDto;);
reaction_fields!(impl_value_from ReactionDto;);
reactor_fields!(impl_value_from ReactorAvatarDto;);
room_fields!(impl_value_from RoomDto;);
space_fields!(impl_value_from SpaceDto;);
sticker_cell_fields!(impl_value_from StickerCellDto;);
sticker_pack_fields!(impl_value_from StickerPackDto;);
sticker_row_fields!(impl_value_from StickerRowDto;);

fn entry_id_from_value(val: &Value) -> Option<&SharedString> {
    message_text_field(val, message::UNIQUE_ID)
}

fn message_text_field<'a>(val: &'a Value, field: &str) -> Option<&'a SharedString> {
    if let Value::Struct(s) = val
        && let Some(Value::String(text)) = s.get_field(field)
    {
        Some(text)
    } else {
        None
    }
}

fn unclaimed_face(face: &Value, user_id: &str) -> bool {
    let Value::Struct(fields) = face else {
        return false;
    };
    let claimed = matches!(
        fields.get_field(reactor::HAS_AVATAR),
        Some(Value::Bool(true))
    );
    !claimed
        && matches!(
            fields.get_field(reactor::USER_ID),
            Some(Value::String(id)) if id == user_id
        )
}

fn cell_key_of(cell: &Value) -> Option<&SharedString> {
    if let Value::Struct(s) = cell
        && let Some(Value::String(key)) = s.get_field(sticker_cell::KEY)
    {
        Some(key)
    } else {
        None
    }
}

fn room_id_from_value(val: &Value) -> Option<&SharedString> {
    if let Value::Struct(s) = val
        && let Some(Value::String(id)) = s.get_field(room::ID)
    {
        Some(id)
    } else {
        None
    }
}
