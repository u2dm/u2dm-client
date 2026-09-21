use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::{Image, Model, ModelRc, Rgb8Pixel, SharedPixelBuffer, StyledText, VecModel};
use slint_interpreter::{
    Compiler, ComponentHandle, ComponentInstance, SharedString, Struct, Value,
};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};

use names::{
    callback, emoji_entry, emoji_group, emoji_insert, emoji_store, user_message, verification_emoji,
};

use super::backend::{self, Models, UiBackend, reorder_spaces, selected_room_key, unread_below};
use super::decode::{AvatarSlot, request_avatar, request_media, request_sticker};
use super::dto::{
    MediaFailureKind, MediaState, MessageDto, ReactionDto, ReactorAvatarDto, RoomDto, SpaceDto,
    StickerCellDto, StickerPackDto, StickerRowDto,
};
#[cfg(feature = "demo")]
use super::dump;
use super::fields::{
    MessageFields, ReactionFields, ReactorFields, RoomFields, SpaceFields, StickerCellFields,
    StickerPackFields, StickerRowFields,
};
use super::present::{Delivery, MessageKind, ServiceKind, VerifyStep};
#[cfg(feature = "demo")]
use super::props::EnumProp;
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::schema::{
    attachment_kinds, audio_kinds, connection_states, deliveries, enum_props, login_activities,
    login_methods, login_phases, media_failures, media_states, message_fields, message_kinds,
    model_props, preview_kinds, reaction_fields, reaction_sends, reactor_fields, room_fields,
    room_scopes, send_states, service_kinds, simple_callbacks, space_fields, sticker_cell_fields,
    sticker_pack_fields, sticker_row_fields, timeline_states, user_message_kinds,
    verification_activities, verification_phases,
};
use super::session::active_models;
use super::video::{self, millis_to_duration};
use super::{audio, emoji, router};
use crate::app::input::CommandSender;
use crate::commands::effects::{Effect, VerificationActivity};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::{TimelineVisibility, ViewportChanged};
use crate::commands::view::{AppViewState, AttachmentKind, LoginActivity, LoginStep, RoomScope};
use crate::domain::auth::LoginMethod;
use crate::domain::media::AudioKind;
use crate::domain::message::{MessagePreviewKind, ReactionSend, SendState};
use crate::domain::sync::ConnectionStatus;
use crate::domain::timeline::TimelineStatus;
use crate::domain::verification::VerificationEmoji as DomainVerificationEmoji;
use crate::error::{AppError, Result};
use crate::ports::media::MediaCache;

#[allow(dead_code)]
mod names {
    pub mod callback {
        pub const GLOBAL: &str = "Actions";
        pub const MOVE_SPACE: &str = "move-space";
        pub const DISMISS_UNSENT: &str = "dismiss-unsent";
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
        pub const TIMELINE_VISIBILITY_CHANGED: &str = "timeline-visibility-changed";
    }

    pub mod emoji_store {
        pub const NAME: &str = "EmojiStore";
        pub const GROUPS: &str = "groups";
        pub const RESULTS: &str = "results";
        pub const SEARCH: &str = "search";
        pub const INSERT: &str = "insert";
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
deliveries!(impl_slint_enum Delivery "Delivery";);
media_failures!(impl_slint_enum MediaFailureKind "MediaFailure";);
message_kinds!(impl_slint_enum MessageKind "MessageKind";);
attachment_kinds!(impl_slint_enum AttachmentKind "AttachmentKind";);
room_scopes!(impl_slint_enum RoomScope "RoomScope";);
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

fn int_arg(args: &[Value], index: usize) -> Option<i32> {
    match args.get(index) {
        Some(Value::Number(n))
            if n.is_finite()
                && n.fract() == 0.0
                && *n >= i32::MIN.into()
                && *n <= i32::MAX.into() =>
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
    ($($v:ident $fn:ident($ty:ty) $g:ident $gname:literal $lit:literal $s:ident $get:ident;)*) => {
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
        let value = self.get_global_property(prop.global(), prop.as_str()).ok();
        int_of(value.as_ref())
    }

    fn get_bool(&self, prop: BoolProp) -> bool {
        let value = self.get_global_property(prop.global(), prop.as_str()).ok();
        flag_of(value.as_ref())
    }

    #[cfg(feature = "demo")]
    fn get_enum(&self, prop: EnumProp) -> SharedString {
        let value = self.get_global_property(prop.global(), prop.as_str()).ok();
        variant_of(value.as_ref()).into()
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
    type Reaction = Value;
    type Reactor = Value;
    type Room = Value;
    type Space = Value;
    type StickerRow = Value;
    type StickerCell = Value;
    type StickerPack = Value;

    fn models() -> Rc<Models<Self>> {
        active_models()
    }

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
        visibility_tx: &watch::Sender<TimelineVisibility>,
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
        bind_action(&self.instance, callback::DISMISS_UNSENT, move |args| {
            if let Some(submission) = int_arg(args, 0) {
                router::dismiss_unsent(&tx, submission);
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
                    unread_below::<InterpretedBackend>(int_arg(args, 1).unwrap_or_default()),
                );
                Value::Void
            },
        )?;

        let visibility_tx = visibility_tx.clone();
        bind_action(
            &self.instance,
            callback::TIMELINE_VISIBILITY_CHANGED,
            move |args| {
                router::timeline_visibility(&visibility_tx, bool_arg(args, 0));
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
pub fn install_timeline_dump(ui: &SlintUiAdapter) {
    dump::install_probe::<InterpretedBackend>(&ui.instance);
}

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

fn ratio(value: f32) -> Value {
    Value::Number(value.into())
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
reaction_fields!(impl_value ReactionDto ReactionFields;);
reactor_fields!(impl_value ReactorAvatarDto ReactorFields;);
room_fields!(impl_value RoomDto RoomFields;);
space_fields!(impl_value SpaceDto SpaceFields;);
sticker_cell_fields!(impl_value StickerCellDto StickerCellFields;);
sticker_pack_fields!(impl_value StickerPackDto StickerPackFields;);
sticker_row_fields!(impl_value StickerRowDto StickerRowFields;);

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

fn text_of(value: Option<&Value>) -> &str {
    match value {
        Some(Value::String(text)) => text.as_str(),
        _ => "",
    }
}

fn variant_of(value: Option<&Value>) -> &str {
    match value {
        Some(Value::EnumerationValue(_, variant)) => variant.as_str(),
        _ => "",
    }
}

fn int_of(value: Option<&Value>) -> i32 {
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

fn flag_of(value: Option<&Value>) -> bool {
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

fn model_of(value: Option<&Value>) -> ModelRc<Value> {
    match value {
        Some(Value::Model(model)) => model.clone(),
        _ => ModelRc::default(),
    }
}

fn strings_of(value: Option<&Value>) -> Vec<SharedString> {
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
