use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ModelRc, VecModel};
use slint_interpreter::{
    Compiler, ComponentHandle, ComponentInstance, SharedString, Struct, Value,
};
use tokio::runtime::Runtime;
use tokio::sync::{OwnedSemaphorePermit, mpsc, watch};

thread_local! {
    static TIMELINE_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
    static ROOMS_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
    static SPACES_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
    static SUBSPACES_MODEL: RefCell<Option<Rc<VecModel<Value>>>> = const { RefCell::new(None) };
}

use super::common::{BoolProp, IntProp, StringProp, UiProps, dispatch_ui_event, reorder_rows};
use super::decode::{
    AvatarSlot, advance_animations, patch_rows, set_animation_tick, set_avatar_ready,
    set_image_ready,
};
use super::dto::{ThumbUpdate, enrich_to_update, message_to_dto, room_to_dto, space_to_dto};
use super::multiplex::spawn_event_multiplexer;
use super::schema::{
    callback, emoji_entry, emoji_group, emoji_insert, emoji_store, login_request, message, prop,
    room, save_file_request, send_message_request, space, verification_emoji,
};
use super::{emoji, router};
use crate::commands::{UiCommand, UiEvent, ViewportChanged};
use crate::domain::models::{
    ConnectionStatus, EnrichmentDelta, LoginCredentials, Room, RoomId, Space, TimelineMessage,
    VerificationEmoji as DomainVerificationEmoji,
};
use crate::error::{AppError, Result};
use crate::ports::media::MediaCache;

fn set_prop(inst: &ComponentInstance, name: &str, value: Value) {
    if let Err(e) = inst.set_property(name, value) {
        tracing::warn!("failed to set property '{name}': {e:?}");
    }
}

fn set_global_prop(inst: &ComponentInstance, global: &str, name: &str, value: Value) -> Result<()> {
    inst.set_global_property(global, name, value)
        .map_err(|e| AppError::Ui(format!("{e:?}")))
}

fn selected_room_key(weak: &slint::Weak<ComponentInstance>) -> Option<(RoomId, i32)> {
    let inst = weak.upgrade()?;
    let room_id = inst.get_string(StringProp::SelectedRoomId).to_string();
    if room_id.is_empty() {
        return None;
    }
    let generation = match inst.get_property(prop::SELECTED_GENERATION) {
        Ok(Value::Number(n)) if n.is_finite() && n.fract() == 0.0 => {
            n.to_string().parse().unwrap_or_default()
        }
        _ => 0,
    };
    Some((RoomId::new(room_id), generation))
}

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

fn props(inst: Option<&ComponentInstance>) -> Option<&dyn UiProps> {
    inst.map(|inst| inst as &dyn UiProps)
}

fn bind(
    inst: &ComponentInstance,
    name: &str,
    callback: impl Fn(&[Value]) -> Value + 'static,
) -> Result<()> {
    inst.set_callback(name, callback)
        .map_err(|e| AppError::Ui(format!("{e:?}")))
}

impl UiProps for ComponentInstance {
    fn set_string(&self, prop: StringProp, value: SharedString) {
        set_prop(self, prop.as_str(), Value::String(value));
    }

    fn set_bool(&self, prop: BoolProp, value: bool) {
        set_prop(self, prop.as_str(), Value::Bool(value));
    }

    fn set_int(&self, prop: IntProp, value: i32) {
        set_prop(self, prop.as_str(), Value::Number(value.into()));
    }

    fn get_string(&self, prop: StringProp) -> SharedString {
        self.get_property(prop.as_str())
            .ok()
            .and_then(|v| match v {
                Value::String(s) => Some(s),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn apply_user_avatar(&self, avatar: Option<slint::Image>) {
        match avatar {
            Some(img) => {
                set_prop(self, prop::USER_AVATAR, Value::Image(img));
                set_prop(self, prop::USER_HAS_AVATAR, Value::Bool(true));
            }
            None => set_prop(self, prop::USER_HAS_AVATAR, Value::Bool(false)),
        }
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
        set_prop(
            self,
            prop::VERIFICATION_EMOJIS,
            Value::Model(ModelRc::new(VecModel::from(entries))),
        );
    }

    fn clear_emoji_model(&self) {
        set_prop(
            self,
            prop::VERIFICATION_EMOJIS,
            Value::Model(ModelRc::new(VecModel::<Value>::default())),
        );
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

    #[allow(clippy::too_many_lines)]
    pub fn register_callbacks(
        &self,
        cmd_tx: &mpsc::UnboundedSender<UiCommand>,
        scroll_tx: &watch::Sender<ViewportChanged>,
    ) -> Result<()> {
        setup_emoji_store(&self.instance)?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        bind(&self.instance, callback::CHECK_SERVER, move |args| {
            let inst = weak.upgrade();
            router::check_server(props(inst.as_ref()), &tx, string_arg(args, 0));
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        bind(&self.instance, callback::LOGIN_PASSWORD, move |args| {
            let Some(s) = struct_arg(args, 0) else {
                return Value::Void;
            };
            let creds = LoginCredentials {
                homeserver: field(s, login_request::HOMESERVER),
                username: field(s, login_request::USERNAME),
                password: field(s, login_request::PASSWORD),
            };
            let inst = weak.upgrade();
            router::login_password(props(inst.as_ref()), &tx, creds);
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        bind(&self.instance, callback::LOGIN_OAUTH, move |_args| {
            let inst = weak.upgrade();
            router::login_oauth(props(inst.as_ref()), &tx);
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        bind(&self.instance, callback::CANCEL_OAUTH, move |_args| {
            router::cancel_oauth(&tx);
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        bind(&self.instance, callback::SELECT_ROOM, move |args| {
            router::select_room(&tx, string_arg(args, 0));
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        bind(&self.instance, callback::SELECT_SPACE, move |args| {
            router::select_space(&tx, string_arg(args, 0));
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        bind(&self.instance, callback::SELECT_SUBSPACE, move |args| {
            router::select_subspace(&tx, string_arg(args, 0));
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        bind(&self.instance, callback::MOVE_SPACE, move |args| {
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
        bind(&self.instance, callback::LOGOUT, move |_args| {
            router::logout(&tx);
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        bind(&self.instance, callback::SEND_MESSAGE, move |args| {
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

        let tx = cmd_tx.clone();
        bind(
            &self.instance,
            callback::ACCEPT_VERIFICATION,
            move |_args| {
                router::accept_verification(&tx);
                Value::Void
            },
        )?;

        let tx = cmd_tx.clone();
        bind(
            &self.instance,
            callback::CONFIRM_VERIFICATION,
            move |_args| {
                router::confirm_verification(&tx);
                Value::Void
            },
        )?;

        let tx = cmd_tx.clone();
        bind(
            &self.instance,
            callback::REJECT_VERIFICATION,
            move |_args| {
                router::reject_verification(&tx);
                Value::Void
            },
        )?;

        let tx = cmd_tx.clone();
        bind(&self.instance, callback::OPEN_MEDIA, move |args| {
            router::open_media(&tx, string_arg(args, 0));
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        bind(&self.instance, callback::SAVE_FILE, move |args| {
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
        bind(
            &self.instance,
            callback::SCROLL_POSITION_CHANGED,
            move |args| {
                router::scroll_position(
                    &scroll_tx,
                    selected_room_key(&weak),
                    bool_arg(args, 0),
                    bool_arg(args, 1),
                );
                Value::Void
            },
        )?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        bind(&self.instance, callback::PAGINATE_BACKWARDS, move |_args| {
            router::paginate_backwards(&tx, selected_room_key(&weak));
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        bind(&self.instance, callback::PAGINATE_FORWARDS, move |_args| {
            router::paginate_forwards(&tx, selected_room_key(&weak));
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        bind(&self.instance, callback::JUMP_TO_LATEST, move |_args| {
            router::jump_to_latest(&tx, selected_room_key(&weak));
            Value::Void
        })?;

        let tx = cmd_tx.clone();
        bind(&self.instance, callback::RETRY_TIMELINE, move |_args| {
            router::retry_timeline(&tx);
            Value::Void
        })?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn spawn_event_handler(
        &self,
        ui_rx: mpsc::Receiver<UiEvent>,
        rooms_rx: watch::Receiver<Arc<[Room]>>,
        spaces_rx: watch::Receiver<Arc<[Space]>>,
        subspaces_rx: watch::Receiver<Arc<[Space]>>,
        connection_rx: watch::Receiver<ConnectionStatus>,
        status_rx: watch::Receiver<String>,
        media_cache: Arc<dyn MediaCache>,
    ) {
        let weak = self.instance.as_weak();
        let timeline_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());
        let rooms_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());
        let spaces_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());
        let subspaces_model: Rc<VecModel<Value>> = Rc::new(VecModel::default());

        set_prop(
            &self.instance,
            prop::TIMELINE,
            Value::Model(ModelRc::from(Rc::clone(&timeline_model))),
        );
        set_prop(
            &self.instance,
            prop::ROOMS,
            Value::Model(ModelRc::from(Rc::clone(&rooms_model))),
        );
        set_prop(
            &self.instance,
            prop::SPACES,
            Value::Model(ModelRc::from(Rc::clone(&spaces_model))),
        );
        set_prop(
            &self.instance,
            prop::SUBSPACES,
            Value::Model(ModelRc::from(Rc::clone(&subspaces_model))),
        );

        TIMELINE_MODEL.with(|cell| *cell.borrow_mut() = Some(timeline_model));
        ROOMS_MODEL.with(|cell| *cell.borrow_mut() = Some(rooms_model));
        SPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(spaces_model));
        SUBSPACES_MODEL.with(|cell| *cell.borrow_mut() = Some(subspaces_model));

        set_animation_tick(|| {
            if let Some(timeline) = TIMELINE_MODEL.with(|cell| cell.borrow().clone()) {
                advance_animations(&timeline, &entry_id_from_value, &|value, frame| {
                    if let Value::Struct(entry) = value {
                        entry.set_field(message::THUMBNAIL.to_string(), Value::Image(frame));
                    }
                });
            }
        });

        set_image_ready({
            let weak = self.instance.as_weak();
            move |unique_id, image| {
                apply_thumbnail_ready(unique_id, image);
                if let Some(instance) = weak.upgrade() {
                    instance.window().request_redraw();
                }
            }
        });

        set_avatar_ready({
            let weak = self.instance.as_weak();
            move |slots, image| {
                apply_avatar_ready(&weak, slots, image);
                if let Some(instance) = weak.upgrade() {
                    instance.window().request_redraw();
                }
            }
        });

        spawn_event_multiplexer(
            ui_rx,
            rooms_rx,
            spaces_rx,
            subspaces_rx,
            connection_rx,
            status_rx,
            media_cache,
            move |event, media, permit| post_ui_event(&weak, media, event, permit),
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
}

fn post_ui_event(
    weak: &slint::Weak<ComponentInstance>,
    media_cache: Arc<dyn MediaCache>,
    event: UiEvent,
    permit: OwnedSemaphorePermit,
) {
    weak.upgrade_in_event_loop(move |inst| {
        let timeline = TIMELINE_MODEL.with(|cell| cell.borrow().clone());
        let rooms = ROOMS_MODEL.with(|cell| cell.borrow().clone());
        let spaces = SPACES_MODEL.with(|cell| cell.borrow().clone());
        let subspaces = SUBSPACES_MODEL.with(|cell| cell.borrow().clone());
        if let (Some(tl), Some(rm), Some(sm), Some(ssm)) = (timeline, rooms, spaces, subspaces) {
            dispatch_ui_event(
                &inst,
                event,
                &tl,
                &rm,
                &sm,
                &ssm,
                &|m| message_to_value(m, media_cache.as_ref()),
                &|v, d| enrich_value(v, d, media_cache.as_ref()),
                &|r| room_to_value(r, media_cache.as_ref()),
                &|s| space_to_value(s, media_cache.as_ref()),
                &|v| room_id_from_value(v).map_or("", SharedString::as_str),
                &|v| room_id_from_value(v).map_or("", SharedString::as_str),
                &entry_id_from_value,
            );
        }
        drop(permit);
    })
    .ok();
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

fn message_to_value(m: &TimelineMessage, media: &dyn MediaCache) -> Value {
    let d = message_to_dto(m, media);
    let mut fields = vec![
        (message::UNIQUE_ID.to_string(), Value::String(d.unique_id)),
        (message::SENDER.to_string(), Value::String(d.sender)),
        (message::PRONOUNS.to_string(), string_list(d.pronouns)),
        (message::BODY.to_string(), Value::String(d.body)),
        (message::TIMESTAMP.to_string(), Value::String(d.timestamp)),
        (
            message::MESSAGE_TYPE.to_string(),
            Value::String(d.message_type),
        ),
        (
            message::PREVIEW_KIND.to_string(),
            Value::String(d.preview_kind),
        ),
        (
            message::UNSUPPORTED_KIND.to_string(),
            Value::String(d.unsupported_kind),
        ),
        (message::EVENT_ID.to_string(), Value::String(d.event_id)),
        (
            message::SENDER_INITIAL.to_string(),
            Value::String(d.sender_initial),
        ),
        (message::COLOR_INDEX.to_string(), num(d.color_index)),
        (message::IS_OWN.to_string(), Value::Bool(d.is_own)),
        (message::EDITED.to_string(), Value::Bool(d.edited)),
        (message::HAS_REPLY.to_string(), Value::Bool(d.has_reply)),
        (
            message::REPLY_SENDER.to_string(),
            Value::String(d.reply_sender),
        ),
        (message::REPLY_KIND.to_string(), Value::String(d.reply_kind)),
        (message::REPLY_BODY.to_string(), Value::String(d.reply_body)),
        (
            message::SERVICE_KIND.to_string(),
            Value::String(d.service_kind),
        ),
        (
            message::SERVICE_TARGET.to_string(),
            Value::String(d.service_target),
        ),
        (
            message::HAS_THUMBNAIL.to_string(),
            Value::Bool(d.has_thumbnail),
        ),
        (
            message::MEDIA_FAILED.to_string(),
            Value::Bool(d.media_failed),
        ),
        (message::IMAGE_WIDTH.to_string(), num(d.image_width)),
        (message::IMAGE_HEIGHT.to_string(), num(d.image_height)),
        (message::HAS_AVATAR.to_string(), Value::Bool(d.has_avatar)),
    ];
    if let Some(img) = d.thumbnail {
        fields.push((message::THUMBNAIL.to_string(), Value::Image(img)));
    }
    if let Some(img) = d.avatar {
        fields.push((message::AVATAR.to_string(), Value::Image(img)));
    }
    Value::Struct(Struct::from_iter(fields))
}

fn apply_thumbnail_ready(unique_id: &str, image: Option<&slint::Image>) {
    let Some(timeline) = TIMELINE_MODEL.with(|cell| cell.borrow().clone()) else {
        return;
    };
    patch_rows(
        &timeline,
        |value: &Value| entry_id_from_value(value) == unique_id,
        |value: &mut Value| {
            if let Value::Struct(entry) = value {
                match image {
                    Some(img) => {
                        entry.set_field(message::THUMBNAIL.to_string(), Value::Image(img.clone()));
                        entry.set_field(message::HAS_THUMBNAIL.to_string(), Value::Bool(true));
                        entry.set_field(message::MEDIA_FAILED.to_string(), Value::Bool(false));
                    }
                    None => {
                        entry.set_field(message::MEDIA_FAILED.to_string(), Value::Bool(true));
                    }
                }
            }
        },
    );
}

fn set_avatar_on_rows(
    model: &VecModel<Value>,
    avatar_field: &str,
    has_avatar_field: &str,
    matches: impl Fn(&Value) -> bool,
    image: &slint::Image,
) {
    patch_rows(model, matches, |value: &mut Value| {
        if let Value::Struct(entry) = value {
            entry.set_field(avatar_field.to_string(), Value::Image(image.clone()));
            entry.set_field(has_avatar_field.to_string(), Value::Bool(true));
        }
    });
}

fn apply_avatar_ready(
    weak: &slint::Weak<ComponentInstance>,
    slots: &[AvatarSlot],
    image: Option<&slint::Image>,
) {
    let Some(image) = image else {
        return;
    };
    for slot in slots {
        match slot {
            AvatarSlot::Message(unique_id) => {
                if let Some(model) = TIMELINE_MODEL.with(|cell| cell.borrow().clone()) {
                    set_avatar_on_rows(
                        &model,
                        message::AVATAR,
                        message::HAS_AVATAR,
                        |v| entry_id_from_value(v) == unique_id.as_str(),
                        image,
                    );
                }
            }
            AvatarSlot::Room(id) => {
                if let Some(model) = ROOMS_MODEL.with(|cell| cell.borrow().clone()) {
                    set_avatar_on_rows(
                        &model,
                        room::AVATAR,
                        room::HAS_AVATAR,
                        |v| room_id_from_value(v).is_some_and(|rid| rid.as_str() == id.as_str()),
                        image,
                    );
                }
            }
            AvatarSlot::Space(id) => {
                for cell in [&SPACES_MODEL, &SUBSPACES_MODEL] {
                    if let Some(model) = cell.with(|slot| slot.borrow().clone()) {
                        set_avatar_on_rows(
                            &model,
                            space::AVATAR,
                            space::HAS_AVATAR,
                            |v| {
                                room_id_from_value(v).is_some_and(|rid| rid.as_str() == id.as_str())
                            },
                            image,
                        );
                    }
                }
            }
            AvatarSlot::User => {
                if let Some(inst) = weak.upgrade() {
                    set_prop(&inst, prop::USER_AVATAR, Value::Image(image.clone()));
                    set_prop(&inst, prop::USER_HAS_AVATAR, Value::Bool(true));
                }
            }
        }
    }
}

fn enrich_value(value: &mut Value, delta: &EnrichmentDelta, media: &dyn MediaCache) {
    let Value::Struct(entry) = value else {
        return;
    };
    let update = enrich_to_update(delta, media);
    match update.thumbnail {
        ThumbUpdate::Ready(img) => {
            entry.set_field(message::THUMBNAIL.to_string(), Value::Image(img));
            entry.set_field(message::HAS_THUMBNAIL.to_string(), Value::Bool(true));
            entry.set_field(message::MEDIA_FAILED.to_string(), Value::Bool(false));
        }
        ThumbUpdate::Failed => {
            entry.set_field(message::MEDIA_FAILED.to_string(), Value::Bool(true));
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

fn room_to_value(r: &Room, media: &dyn MediaCache) -> Value {
    let d = room_to_dto(r, media);
    let mut fields = vec![
        (room::ID.to_string(), Value::String(d.id)),
        (room::NAME.to_string(), Value::String(d.name)),
        (room::INITIAL.to_string(), Value::String(d.initial)),
        (room::COLOR_INDEX.to_string(), num(d.color_index)),
        (room::MEMBERS.to_string(), num(d.members)),
        (room::UNREAD.to_string(), num(d.unread)),
        (room::MENTIONS.to_string(), num(d.mentions)),
        (
            room::LAST_MESSAGE_SENDER.to_string(),
            Value::String(d.last_message_sender),
        ),
        (
            room::LAST_MESSAGE_KIND.to_string(),
            Value::String(d.last_message_kind),
        ),
        (
            room::LAST_MESSAGE_BODY.to_string(),
            Value::String(d.last_message_body),
        ),
        (
            room::LAST_MESSAGE_SERVICE_KIND.to_string(),
            Value::String(d.last_message_service_kind),
        ),
        (
            room::LAST_MESSAGE_SERVICE_TARGET.to_string(),
            Value::String(d.last_message_service_target),
        ),
        (
            room::LAST_MESSAGE_IS_OWN.to_string(),
            Value::Bool(d.last_message_is_own),
        ),
        (
            room::LAST_MESSAGE_EDITED.to_string(),
            Value::Bool(d.last_message_edited),
        ),
        (
            room::LAST_MESSAGE_TIME.to_string(),
            Value::String(d.last_message_time),
        ),
        (room::HAS_AVATAR.to_string(), Value::Bool(d.has_avatar)),
    ];
    if let Some(img) = d.avatar {
        fields.push((room::AVATAR.to_string(), Value::Image(img)));
    }
    Value::Struct(Struct::from_iter(fields))
}

fn space_to_value(s: &Space, media: &dyn MediaCache) -> Value {
    let d = space_to_dto(s, media);
    let mut fields = vec![
        (space::ID.to_string(), Value::String(d.id)),
        (space::NAME.to_string(), Value::String(d.name)),
        (space::UNREAD.to_string(), num(d.unread)),
        (space::MENTIONS.to_string(), num(d.mentions)),
        (space::INITIAL.to_string(), Value::String(d.initial)),
        (space::HAS_AVATAR.to_string(), Value::Bool(d.has_avatar)),
    ];
    if let Some(img) = d.avatar {
        fields.push((space::AVATAR.to_string(), Value::Image(img)));
    }
    Value::Struct(Struct::from_iter(fields))
}

fn entry_id_from_value(val: &Value) -> String {
    if let Value::Struct(s) = val
        && let Some(Value::String(id)) = s.get_field(message::UNIQUE_ID)
    {
        id.to_string()
    } else {
        String::new()
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
