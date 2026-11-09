mod args;
mod callbacks;
mod emoji_rows;
mod names;
mod props;
mod values;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use names::{callback, notification_text, window_props};
use slint::ModelRc;
use slint_interpreter::{
    Compiler, ComponentDefinition, ComponentHandle, ComponentInstance, SharedString, Value,
};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};

use self::args::{int_arg, seen_args, string_arg};
use self::callbacks::{bind_hooks, bind_simple_callbacks, bind_void, warn_unbound};
use self::props::{mirror_bools, mirror_enums, mirror_ints, mirror_strings};
use self::values::{enum_value, text_value, variant_of};
#[cfg(feature = "demo")]
use super::audio;
use super::backend::{self, Models, UiBackend};
use super::call_window::{self, WindowHooks};
use super::dto::{PackSurface, pack_surface_named};
#[cfg(feature = "demo")]
use super::dump;
use super::notices::NoticeWords;
#[cfg(feature = "demo")]
use super::props::{BoolProp, UiProps};
use super::schema::model_props;
use super::session::active_models;
use super::{motion, reduce};
use crate::commands::effects::Effect;
use crate::commands::view::AppViewState;
use crate::domain::settings::Settings;
use crate::error::{AppError, Result};
use crate::ports::appearance::AppearancePort;
use crate::ports::call::CallVideoPort;
use crate::ports::handoff::HandoffPort;
use crate::ports::media::MediaCache;
use crate::ports::notifier::NotifierPort;

fn set_prop(inst: &ComponentInstance, name: &str, value: Value) {
    if let Err(e) = inst.set_property(name, value) {
        tracing::warn!("failed to set property '{name}': {e:?}");
    }
}

fn set_global(inst: &ComponentInstance, global: &str, name: &str, value: Value) {
    if let Err(e) = inst.set_global_property(global, name, value) {
        tracing::warn!("failed to set property '{global}.{name}': {e:?}");
    }
}

macro_rules! attach_interpreted_models {
    ($window:ident $models:ident;
        $($field:ident $row:ident $model:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        $( set_global($window, $gname, $lit, Value::Model(ModelRc::from(Rc::clone(&$models.$field)))); )*
    };
}

macro_rules! attach_mirrored_models {
    ($($field:ident $row:ident $model:ident $g:ident $gname:literal $lit:literal $s:ident;)*) => {
        fn attach_mirrored_models(window: &ComponentInstance, models: &Models<InterpretedBackend>) {
            $( if call_window::mirrors($gname) {
                set_global(window, $gname, $lit, Value::Model(ModelRc::from(Rc::clone(&models.$field))));
            } )*
        }
    };
}

model_props!(attach_mirrored_models);

thread_local! {
    static CALL_WINDOW: RefCell<Option<ComponentDefinition>> = const { RefCell::new(None) };
}

pub struct InterpretedBackend;

impl UiBackend for InterpretedBackend {
    type Window = ComponentInstance;
    type CallWindow = ComponentInstance;
    type Message = Value;
    type BodyLine = Value;
    type BodyPiece = Value;
    type Reaction = Value;
    type Reactor = Value;
    type PollAnswer = Value;
    type Room = Value;
    type Space = Value;
    type SpaceChild = Value;
    type MemberRow = Value;
    type Invitee = Value;
    type LogLine = Value;
    type CallTile = Value;
    type CallPerson = Value;
    type CallConnection = Value;
    type CallDevice = Value;
    type UserDevice = Value;
    type PackRow = Value;
    type PackCell = Value;
    type PackTab = Value;
    type Draft = Value;

    fn models() -> Rc<Models<Self>> {
        active_models()
    }

    fn attach_models(window: &ComponentInstance, models: &Models<Self>) {
        model_props!(attach_interpreted_models window models;);
    }

    fn bind_browse_packs(
        window: &ComponentInstance,
        browse: impl Fn(PackSurface, &str, i32) + 'static,
    ) {
        let bound = bind_void(window, callback::BROWSE_PACKS, move |args| {
            if let Some(surface) = pack_surface_named(variant_of(args.first())) {
                browse(
                    surface,
                    &string_arg(args, 1),
                    int_arg(args, 2).unwrap_or_default(),
                );
            }
        });
        warn_unbound(bound, "the pack browsing callback");
    }

    fn bind_reveal_spoilers(window: &ComponentInstance, reveal: impl Fn(&str) + 'static) {
        let bound = bind_void(window, callback::REVEAL_SPOILERS, move |args| {
            reveal(&string_arg(args, 0));
        });
        warn_unbound(bound, "the spoiler reveal callback");
    }

    fn bind_window_hooks(window: &ComponentInstance, hooks: WindowHooks) {
        warn_unbound(bind_hooks(window, hooks), "the window's call callbacks");
    }

    fn bind_call_preview_seen(
        window: &ComponentInstance,
        seen: impl Fn(&str, i32, i32, bool) + 'static,
    ) {
        let bound = bind_void(window, callback::CALL_PREVIEW_SEEN, move |args| {
            let (feed, width, height, shown) = seen_args(args);
            seen(&feed, width, height, shown);
        });
        warn_unbound(bound, "the camera preview callback");
    }

    fn new_call_window(main: &ComponentInstance, hooks: WindowHooks) -> Option<ComponentInstance> {
        let window = CALL_WINDOW
            .with_borrow(|definition| definition.as_ref().map(ComponentDefinition::create))?
            .inspect_err(|e| tracing::warn!("the call window could not be built: {e}"))
            .ok()?;
        mirror_strings(main, &window);
        mirror_bools(main, &window);
        mirror_ints(main, &window);
        mirror_enums(main, &window);
        attach_mirrored_models(&window, &Self::models());
        if let Some(tx) = call_window::commands() {
            let bound = bind_simple_callbacks(&window, main, &tx);
            warn_unbound(bound, "the call window's callbacks");
        }
        let bound = bind_hooks(&window, hooks);
        warn_unbound(bound, "the call window's call callbacks");
        set_prop(&window, window_props::READY, Value::Bool(true));
        Some(window)
    }

    fn call_window() -> Option<ComponentInstance> {
        call_window::opened()
    }

    fn keep_call_window(window: Option<ComponentInstance>) -> Option<ComponentInstance> {
        call_window::keep(window)
    }

    fn notification_text(window: &ComponentInstance, words: &NoticeWords<'_>) -> SharedString {
        let (function, args) = match *words {
            NoticeWords::Message { kind, body, sender } => (
                notification_text::MESSAGE,
                vec![enum_value(&kind), text_value(body), text_value(sender)],
            ),
            NoticeWords::Service {
                kind,
                actor,
                target,
            } => (
                notification_text::SERVICE,
                vec![enum_value(&kind), text_value(actor), text_value(target)],
            ),
            NoticeWords::Invite { inviter } => {
                (notification_text::INVITE, vec![text_value(inviter)])
            }
            NoticeWords::Ring { video, caller } => (
                notification_text::RING,
                vec![Value::Bool(video), text_value(caller)],
            ),
        };
        match window.invoke_global(notification_text::NAME, function, &args) {
            Ok(Value::String(text)) => text,
            Ok(other) => {
                tracing::warn!(function, "the notification words came back as {other:?}");
                SharedString::new()
            }
            Err(e) => {
                tracing::warn!(
                    function,
                    "the notification words could not be asked for: {e}"
                );
                SharedString::new()
            }
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
            CALL_WINDOW.set(result.component("CallWindow"));
            Ok::<_, AppError>(inst)
        })?;
        backend::identify_the_app();
        Ok(Self { instance })
    }

    pub fn spawn_event_handler(
        &self,
        ui_rx: mpsc::Receiver<Effect>,
        view_rx: watch::Receiver<Arc<AppViewState>>,
        media_cache: Arc<dyn MediaCache>,
        notifier: Arc<dyn NotifierPort>,
        handoff: &dyn HandoffPort,
        call_video: Option<Arc<dyn CallVideoPort>>,
    ) {
        backend::spawn_event_handler::<InterpretedBackend>(
            &self.instance,
            ui_rx,
            view_rx,
            media_cache,
            notifier,
            handoff,
            call_video,
        );
    }

    pub fn follow_motion(&self, appearance: &dyn AppearancePort) {
        motion::follow_motion::<InterpretedBackend>(&self.instance, appearance);
    }

    pub fn seed_settings(&self, settings: &Settings) {
        reduce::seed_settings(&self.instance, settings);
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
