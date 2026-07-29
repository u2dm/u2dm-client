use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::{Image, ModelRc, VecModel};
use slint_interpreter::{
    Compiler, ComponentHandle, ComponentInstance, SharedString, Struct, Value,
};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};

thread_local! {
    static TIMELINE_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
    static ROOMS_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
    static SPACES_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
    static SUBSPACES_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
}

use names::{
    callback, emoji_entry, emoji_group, emoji_insert, emoji_store, login_request, message, room,
    save_file_request, send_message_request, space, user_message, verification_emoji,
};

use super::backend::{UiBackend, install_render_hooks, post_effect, selected_room_key};
use super::clock::install_clock_invalidation;
use super::decode::{AvatarSlot, request_avatar, request_media};
use super::dto::{
    MediaState, ThumbUpdate, enrich_to_update, message_to_dto, room_to_dto, space_to_dto,
};
use super::multiplex::spawn_event_multiplexer;
use super::present::{MessageKind, ServiceKind, VerifyStep};
use super::props::{BoolProp, IntProp, StringProp, UiProps};
use super::reconcile::reorder_rows;
use super::schema::{
    connection_states, login_activities, login_methods, login_phases, media_states, message_fields,
    message_kinds, preview_kinds, room_fields, service_kinds, simple_callbacks, space_fields,
    timeline_states, user_message_kinds, verification_activities, verification_phases,
};
use super::{emoji, router};
use crate::commands::effects::{Effect, VerificationActivity};
use crate::commands::messages::{UserMessage, UserMessageKind};
use crate::commands::ui::{UiCommand, ViewportChanged};
use crate::commands::view::{AppViewState, LoginActivity, LoginStep};
use crate::domain::models::{
    ConnectionStatus, EnrichmentDelta, LoginCredentials, LoginMethod, MessagePreviewKind, Room,
    Space, TimelineMessage, TimelineStatus, VerificationEmoji as DomainVerificationEmoji,
};
use crate::error::{AppError, Result};
use crate::ports::media::MediaCache;

#[allow(dead_code)]
mod names {
    pub mod callback {
        pub const GLOBAL: &str = "Actions";
        pub const LOGIN_PASSWORD: &str = "login-password";
        pub const MOVE_SPACE: &str = "move-space";
        pub const SEND_MESSAGE: &str = "send-message";
        pub const SAVE_FILE: &str = "save-file";
        pub const REQUEST_MEDIA: &str = "request-media";
        pub const REQUEST_ROOM_AVATAR: &str = "request-room-avatar";
        pub const SCROLL_POSITION_CHANGED: &str = "scroll-position-changed";
        pub const PAGINATE_BACKWARDS: &str = "paginate-backwards";
        pub const PAGINATE_FORWARDS: &str = "paginate-forwards";
        pub const JUMP_TO_LATEST: &str = "jump-to-latest";
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

    pub mod room {
        use crate::adapters::ui::schema::{gen_consts, room_fields};
        room_fields!(gen_consts);
    }

    pub mod space {
        use crate::adapters::ui::schema::{gen_consts, space_fields};
        space_fields!(gen_consts);
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

    pub mod login_request {
        pub const HOMESERVER: &str = "homeserver";
        pub const USERNAME: &str = "username";
        pub const PASSWORD: &str = "password";
    }

    pub mod send_message_request {
        pub const ROOM_ID: &str = "room-id";
        pub const BODY: &str = "body";
        pub const REPLY_TO: &str = "reply-to";
    }

    pub mod save_file_request {
        pub const EVENT_ID: &str = "event-id";
        pub const FILENAME: &str = "filename";
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
message_kinds!(impl_slint_enum MessageKind "MessageKind";);
preview_kinds!(impl_slint_enum MessagePreviewKind "PreviewKind";);
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
    ($inst:expr, $tx:ident; $($on:ident $lit:literal $fn:ident $kind:ident $cmd:ident;)*) => {
        $( bind_interpreted_callbacks!(@one $inst, $tx, $lit, $fn, $kind)?; )*
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

    fn set_login_phase(&self, step: LoginStep) {
        set_global(self, "LoginView", "step", enum_value(&step));
    }

    fn set_login_activity(&self, activity: LoginActivity) {
        set_global(self, "LoginView", "activity", enum_value(&activity));
    }

    fn set_login_method_kind(&self, method: LoginMethod) {
        set_global(self, "LoginView", "method", enum_value(&method));
    }

    fn set_toast_message(&self, kind: UserMessageKind) {
        set_global(self, "RoomView", "toast-message", enum_value(&kind));
    }

    fn set_verification_error(&self, kind: UserMessageKind) {
        set_global(self, "VerificationView", "error", enum_value(&kind));
    }

    fn set_connection_state(&self, status: &ConnectionStatus) {
        set_global(self, "SessionView", "connection-status", enum_value(status));
    }

    fn set_timeline_state(&self, status: TimelineStatus) {
        set_global(self, "RoomView", "timeline-status", enum_value(&status));
    }

    fn set_verification_phase(&self, phase: VerifyStep) {
        set_global(self, "VerificationView", "step", enum_value(&phase));
    }

    fn set_verification_activity(&self, activity: VerificationActivity) {
        set_global(self, "VerificationView", "activity", enum_value(&activity));
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
            "input-password",
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

pub struct InterpretedBackend;

impl UiBackend for InterpretedBackend {
    type Window = ComponentInstance;
    type Message = Value;
    type Room = Value;
    type Space = Value;

    fn convert_message(message: &TimelineMessage, media: &dyn MediaCache) -> Value {
        message_to_value(message, media)
    }

    fn enrich_message(entry: &mut Value, delta: &EnrichmentDelta, media: &dyn MediaCache) {
        enrich_value(entry, delta, media);
    }

    fn convert_room(room: &Room, media: &dyn MediaCache) -> Value {
        room_to_value(room, media)
    }

    fn convert_space(space: &Space, media: &dyn MediaCache) -> Value {
        space_to_value(space, media)
    }

    fn message_id(entry: &Value) -> &str {
        entry_id_from_value(entry).map_or("", SharedString::as_str)
    }

    fn message_is_first_unread(entry: &Value) -> bool {
        matches!(entry, Value::Struct(s)
            if matches!(s.get_field(message::IS_FIRST_UNREAD), Some(Value::Bool(true))))
    }

    fn room_id(entry: &Value) -> &str {
        room_id_from_value(entry).map_or("", SharedString::as_str)
    }

    fn space_id(entry: &Value) -> &str {
        room_id_from_value(entry).map_or("", SharedString::as_str)
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

    fn set_message_media_failed(entry: &mut Value) {
        if let Value::Struct(s) = entry {
            s.set_field(
                message::MEDIA_STATE.to_string(),
                enum_value(&MediaState::Failed),
            );
        }
    }

    fn set_message_frame(entry: &mut Value, image: Image) {
        if let Value::Struct(s) = entry {
            s.set_field(message::THUMBNAIL.to_string(), Value::Image(image));
        }
    }

    fn with_models<R>(
        f: impl FnOnce(&VecModel<Value>, &VecModel<Value>, &VecModel<Value>, &VecModel<Value>) -> R,
    ) -> Option<R> {
        let timeline = TIMELINE_MODEL.with(|cell| cell.borrow().clone())?;
        let rooms = ROOMS_MODEL.with(|cell| cell.borrow().clone())?;
        let spaces = SPACES_MODEL.with(|cell| cell.borrow().clone())?;
        let subspaces = SUBSPACES_MODEL.with(|cell| cell.borrow().clone())?;
        Some(f(&timeline, &rooms, &spaces, &subspaces))
    }

    fn with_timeline<R>(f: impl FnOnce(&VecModel<Value>) -> R) -> Option<R> {
        let timeline = TIMELINE_MODEL.with(|cell| cell.borrow().clone())?;
        Some(f(&timeline))
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

    pub fn register_callbacks(
        &self,
        cmd_tx: &mpsc::UnboundedSender<UiCommand>,
        scroll_tx: &watch::Sender<ViewportChanged>,
    ) -> Result<()> {
        setup_emoji_store(&self.instance)?;

        simple_callbacks!(bind_interpreted_callbacks &self.instance, cmd_tx;);

        let tx = cmd_tx.clone();
        bind_action(&self.instance, callback::LOGIN_PASSWORD, move |args| {
            let Some(s) = struct_arg(args, 0) else {
                return Value::Void;
            };
            router::login_password(
                &tx,
                LoginCredentials {
                    username: field(s, login_request::USERNAME),
                    password: field(s, login_request::PASSWORD),
                },
            );
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        bind_action(&self.instance, callback::MOVE_SPACE, move |args| {
            if let (Some(from), Some(to)) = (usize_arg(args, 0), usize_arg(args, 1)) {
                router::move_space(&tx, from, to, |from, to| {
                    SPACES_MODEL.with(|cell| {
                        if let Some(model) = cell.borrow().as_ref() {
                            reorder_rows(model, from, to);
                        }
                    });
                });
            }
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        bind_action(&self.instance, callback::SEND_MESSAGE, move |args| {
            let Some(s) = struct_arg(args, 0) else {
                return Value::Void;
            };
            router::send_message(
                &tx,
                field(s, send_message_request::ROOM_ID),
                field(s, send_message_request::BODY),
                field(s, send_message_request::REPLY_TO),
            );
            Value::Void
        })?;

        bind_action(&self.instance, callback::REQUEST_MEDIA, move |args| {
            request_media(&string_arg(args, 0));
            Value::Void
        })?;

        bind_action(&self.instance, callback::REQUEST_ROOM_AVATAR, move |args| {
            request_avatar(&AvatarSlot::Room(string_arg(args, 0)));
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        bind_action(&self.instance, callback::SAVE_FILE, move |args| {
            let Some(s) = struct_arg(args, 0) else {
                return Value::Void;
            };
            router::save_file(
                &tx,
                field(s, save_file_request::EVENT_ID),
                field(s, save_file_request::FILENAME),
            );
            Value::Void
        })?;

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
        )?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        bind_action(&self.instance, callback::PAGINATE_BACKWARDS, move |_args| {
            router::paginate_backwards(&tx, selected_room_key::<InterpretedBackend>(&weak));
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        bind_action(&self.instance, callback::PAGINATE_FORWARDS, move |_args| {
            router::paginate_forwards(&tx, selected_room_key::<InterpretedBackend>(&weak));
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        bind_action(&self.instance, callback::JUMP_TO_LATEST, move |_args| {
            router::jump_to_latest(&tx, selected_room_key::<InterpretedBackend>(&weak));
            Value::Void
        })?;

        Ok(())
    }

    pub fn spawn_event_handler(
        &self,
        ui_rx: mpsc::Receiver<Effect>,
        view_rx: watch::Receiver<Arc<AppViewState>>,
        media_cache: Arc<dyn MediaCache>,
    ) {
        let weak = self.instance.as_weak();
        let timeline_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());
        let rooms_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());
        let spaces_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());
        let subspaces_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());

        set_global(
            &self.instance,
            "RoomView",
            "timeline",
            Value::Model(ModelRc::from(Rc::clone(&timeline_model))),
        );
        set_global(
            &self.instance,
            "DirectoryView",
            "rooms",
            Value::Model(ModelRc::from(Rc::clone(&rooms_model))),
        );
        set_global(
            &self.instance,
            "DirectoryView",
            "spaces",
            Value::Model(ModelRc::from(Rc::clone(&spaces_model))),
        );
        set_global(
            &self.instance,
            "DirectoryView",
            "subspaces",
            Value::Model(ModelRc::from(Rc::clone(&subspaces_model))),
        );

        TIMELINE_MODEL.with(|cell| *cell.borrow_mut() = Some(timeline_model));
        ROOMS_MODEL.with(|cell| *cell.borrow_mut() = Some(rooms_model));
        SPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(spaces_model));
        SUBSPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(subspaces_model));

        install_render_hooks::<InterpretedBackend>(self.instance.as_weak());
        install_clock_invalidation::<InterpretedBackend>(Arc::clone(&media_cache));

        spawn_event_multiplexer(ui_rx, view_rx, media_cache, move |event, media, permit| {
            post_effect::<InterpretedBackend>(&weak, media, event, permit);
        });
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

fn string_list(items: Vec<SharedString>) -> Value {
    let values: Vec<Value> = items.into_iter().map(Value::String).collect();
    Value::Model(ModelRc::new(VecModel::from(values)))
}

macro_rules! field_value {
    ($s:ident, $lit:literal, $val:expr, text) => {
        $s.set_field($lit.to_string(), Value::String($val));
    };
    ($s:ident, $lit:literal, $val:expr, int) => {
        $s.set_field($lit.to_string(), num($val));
    };
    ($s:ident, $lit:literal, $val:expr, flag) => {
        $s.set_field($lit.to_string(), Value::Bool($val));
    };
    ($s:ident, $lit:literal, $val:expr, list) => {
        $s.set_field($lit.to_string(), string_list($val));
    };
    ($s:ident, $lit:literal, $val:expr, image) => {
        if let Some(img) = $val {
            $s.set_field($lit.to_string(), Value::Image(img));
        }
    };
    ($s:ident, $lit:literal, $val:expr, enumk) => {
        $s.set_field($lit.to_string(), enum_value(&$val));
    };
}

macro_rules! gen_to_value {
    ($fn:ident $ty:ident $dto:ident $($f:ident $c:ident $lit:literal $k:ident;)*) => {
        fn $fn(src: &$ty, media: &dyn MediaCache) -> Value {
            let d = $dto(src, media);
            let mut fields = Struct::default();
            $( field_value!(fields, $lit, d.$f, $k); )*
            Value::Struct(fields)
        }
    };
}

message_fields!(gen_to_value message_to_value TimelineMessage message_to_dto);

fn enrich_value(value: &mut Value, delta: &EnrichmentDelta, media: &dyn MediaCache) {
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
        ThumbUpdate::Failed => {
            entry.set_field(
                message::MEDIA_STATE.to_string(),
                enum_value(&MediaState::Failed),
            );
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

room_fields!(gen_to_value room_to_value Room room_to_dto);

space_fields!(gen_to_value space_to_value Space space_to_dto);

fn entry_id_from_value(val: &Value) -> Option<&SharedString> {
    if let Value::Struct(s) = val
        && let Some(Value::String(id)) = s.get_field(message::UNIQUE_ID)
    {
        Some(id)
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
