use slint::Model;
use slint_interpreter::{ComponentHandle, ComponentInstance, SharedString, Value};
use tokio::sync::watch;

use super::args::{
    bool_arg, field, flag, int_arg, number, seen_args, string_arg, strings, struct_arg, usize_arg,
};
use super::emoji_rows::setup_emoji_store;
use super::names::{callback, composer_draft};
use super::values::{
    draft_verdict_value, mention_word_value, model_of, model_value, no_draft_value, num,
    text_splice_value, text_value, variant_of, word_value,
};
use super::{InterpretedBackend, SlintUiAdapter};
use crate::adapters::ui::backend::{
    RECENT_SPEAKERS, adopted_room_key, current_edited_row, last_editable_row, on_main,
    recent_speakers, reorder_spaces, row_of_message, selected_room_key, shown_room_log_text,
    unread_below,
};
use crate::adapters::ui::call_window::{self, WindowHooks};
use crate::adapters::ui::decode::{
    AvatarSlot, request_avatar, request_custom_emoji, request_media, request_pack_cell,
};
use crate::adapters::ui::router::{DraftVerdict, EditedRow, EditedText};
use crate::adapters::ui::schema::simple_callbacks;
use crate::adapters::ui::video::{self, millis_to_duration};
use crate::adapters::ui::{audio, drafts, emote, list_keys, mention, router, settings_search};
use crate::app::input::CommandSender;
use crate::commands::ui::{ComposerActivity, TimelineVisibility, ViewportChanged, WindowAttention};
use crate::error::{AppError, Result};

pub(super) fn bind(
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

pub(super) fn bind_void(
    inst: &ComponentInstance,
    name: &str,
    handler: impl Fn(&[Value]) + 'static,
) -> Result<()> {
    bind_action(inst, name, move |args| {
        handler(args);
        Value::Void
    })
}

macro_rules! bind_interpreted_callbacks {
    ($inst:expr, $main:expr, $tx:ident;
        $($on:ident $lit:literal $fn:ident $kind:ident $(($($arg:tt)*))?
            $cmd:ident $(:: $variant:ident)?;)*) => {
        $( bind_interpreted_callbacks!(@one $inst, $main, $tx, $lit, $fn, $kind $(($($arg)*))?)?; )*
    };
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident, plain) => {
        bind_interpreted_callbacks!(@unit $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident, pass) => {
        bind_interpreted_callbacks!(@string $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident, room) => {
        bind_interpreted_callbacks!(@string $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident, opt_room) => {
        bind_interpreted_callbacks!(@string $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident, user) => {
        bind_interpreted_callbacks!(@string $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident, manual) => {
        bind_interpreted_callbacks!(@unit $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident, manual_string) => {
        bind_interpreted_callbacks!(@string $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident, event) => {
        bind_interpreted_callbacks!(@string $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident, local) => {
        bind_interpreted_callbacks!(@string $inst, $tx, $lit, $fn)
    };
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident,
        flag($field:ident $name:literal)) => {
        bind_interpreted_callbacks!(@one $inst, $main, $tx, $lit, $fn, request($field $name flag))
    };
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident, room_key) => {{
        let tx = $tx.clone();
        let weak = $main.as_weak();
        bind_void($inst, $lit, move |_args| router::$fn(&tx, selected_room_key::<InterpretedBackend>(&weak)))
    }};
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident,
        request($($field:ident $name:literal $field_kind:ident),*)) => {{
        let tx = $tx.clone();
        bind_action($inst, $lit, move |args| {
            let Some(s) = struct_arg(args, 0) else {
                return Value::Void;
            };
            Value::from(router::$fn(&tx, $(bind_interpreted_callbacks!(@field s $name $field_kind)),*))
        })
    }};
    (@one $inst:expr, $main:expr, $tx:ident, $lit:literal, $fn:ident,
        verdict($($field:ident $name:literal $field_kind:ident),*)) => {{
        let tx = $tx.clone();
        bind_action($inst, $lit, move |args| {
            let Some(s) = struct_arg(args, 0) else {
                return draft_verdict_value(&DraftVerdict::Incomplete);
            };
            draft_verdict_value(&router::$fn(
                &tx,
                $(bind_interpreted_callbacks!(@field s $name $field_kind)),*
            ))
        })
    }};
    (@unit $inst:expr, $tx:ident, $lit:literal, $fn:ident) => {{
        let tx = $tx.clone();
        bind_void($inst, $lit, move |_args| router::$fn(&tx))
    }};
    (@string $inst:expr, $tx:ident, $lit:literal, $fn:ident) => {{
        let tx = $tx.clone();
        bind_void($inst, $lit, move |args| router::$fn(&tx, string_arg(args, 0)))
    }};
    (@field $s:ident $name:literal text) => { field($s, $name) };
    (@field $s:ident $name:literal str) => { &field($s, $name) };
    (@field $s:ident $name:literal int) => { number($s, $name) };
    (@field $s:ident $name:literal flag) => { flag($s, $name) };
    (@field $s:ident $name:literal list) => { strings($s, $name) };
    (@field $s:ident $name:literal variant) => { variant_of($s.get_field($name)) };
}

pub(super) fn bind_simple_callbacks(
    window: &ComponentInstance,
    main: &ComponentInstance,
    cmd_tx: &CommandSender,
) -> Result<()> {
    simple_callbacks!(bind_interpreted_callbacks window, main, cmd_tx;);
    Ok(())
}

pub(super) fn bind_hooks(window: &ComponentInstance, hooks: WindowHooks) -> Result<()> {
    let WindowHooks {
        call_tile_seen,
        call_media_shown,
        watch_microphone,
        watch_connections,
        show_call_window,
        close_call_window,
        show_call_chat,
        show_main_window,
    } = hooks;
    bind_void(window, callback::CALL_TILE_SEEN, move |args| {
        let (feed, width, height, shown) = seen_args(args);
        call_tile_seen(&feed, width, height, shown);
    })?;
    bind_void(window, callback::CALL_MEDIA_SHOWN, move |args| {
        call_media_shown(bool_arg(args, 0));
    })?;
    bind_void(window, callback::WATCH_MICROPHONE, move |args| {
        watch_microphone(struct_arg(args, 0).is_some_and(|s| flag(s, "on")));
    })?;
    bind_void(window, callback::WATCH_CONNECTIONS, move |args| {
        watch_connections(struct_arg(args, 0).is_some_and(|s| flag(s, "on")));
    })?;
    bind_void(window, callback::SHOW_CALL_WINDOW, move |_| {
        show_call_window();
    })?;
    bind_void(window, callback::CLOSE_CALL_WINDOW, move |_| {
        close_call_window();
    })?;
    bind_void(window, callback::SHOW_CALL_CHAT, move |_| show_call_chat())?;
    bind_void(window, callback::SHOW_MAIN_WINDOW, move |_| {
        show_main_window();
    })
}

impl SlintUiAdapter {
    fn bind_audio_callbacks(&self, cmd_tx: &CommandSender) -> Result<()> {
        audio::install_commands(cmd_tx);

        let toggle = on_main::<InterpretedBackend>(&self.instance, audio::toggle);
        bind_void(&self.instance, callback::TOGGLE_AUDIO, move |_| toggle())?;

        let weak = self.instance.as_weak();
        bind_void(&self.instance, callback::SEEK_AUDIO, move |args| {
            if let Some(window) = weak.upgrade() {
                audio::seek(&window, millis_to_duration(usize_arg(args, 0)));
            }
        })
    }

    fn bind_edit_callbacks(&self, cmd_tx: &CommandSender) -> Result<()> {
        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        bind_action(&self.instance, callback::EDIT_MESSAGE, move |args| {
            let Some(req) = struct_arg(args, 0) else {
                return draft_verdict_value(&DraftVerdict::Incomplete);
            };
            let row = current_edited_row::<InterpretedBackend>(
                &weak,
                &field(req, "unique-id"),
                number(req, "timeline-token"),
            )
            .unwrap_or_else(|| EditedRow {
                event_id: field(req, "event-id"),
                local_id: field(req, "local-id"),
            });
            draft_verdict_value(&router::edit_message(
                &tx,
                field(req, "room-id"),
                row,
                EditedText {
                    caption: flag(req, "caption"),
                    restored: flag(req, "restored"),
                    original: field(req, "original"),
                    body: field(req, "body"),
                },
            ))
        })?;

        bind_action(&self.instance, callback::LAST_EDITABLE_ROW, |_| {
            num(last_editable_row::<InterpretedBackend>())
        })?;

        bind_action(&self.instance, callback::ROW_OF_MESSAGE, |args| {
            num(row_of_message::<InterpretedBackend>(
                &string_arg(args, 0),
                &string_arg(args, 1),
            ))
        })?;

        bind_action(&self.instance, callback::SETTINGS_MATCHES, |args| {
            model_value(
                settings_search::matching_entries(
                    &string_arg(args, 0),
                    model_of(args.get(1)).iter().map(|entry| match entry {
                        Value::Struct(entry) => [
                            field(&entry, "label"),
                            field(&entry, "page-title"),
                            field(&entry, "terms"),
                        ],
                        _ => [String::new(), String::new(), String::new()],
                    }),
                )
                .into_iter()
                .map(num)
                .collect(),
            )
        })?;

        bind_action(&self.instance, callback::POLL_ANSWER_TEXTS, |args| {
            model_value(
                model_of(args.first())
                    .iter()
                    .map(|answer| match answer {
                        Value::Struct(entry) => text_value(field(&entry, "label")),
                        _ => Value::String(SharedString::new()),
                    })
                    .collect(),
            )
        })
    }

    fn bind_draft_callbacks(&self) -> Result<()> {
        bind_void(&self.instance, callback::KEEP_DRAFT, |args| {
            if let (Some(fields), Some(draft)) = (struct_arg(args, 0), args.first()) {
                drafts::keep(field(fields, composer_draft::ROOM_ID), draft.clone());
            }
        })?;

        bind_action(&self.instance, callback::TAKE_DRAFT, |args| {
            drafts::take(&string_arg(args, 0)).unwrap_or_else(no_draft_value)
        })
    }

    fn bind_mention_callbacks(&self, cmd_tx: &CommandSender) -> Result<()> {
        let tx = cmd_tx.clone();
        let weak = self.instance.as_weak();
        bind_void(&self.instance, callback::SUGGEST_MENTIONS, move |args| {
            router::suggest_mentions(
                &tx,
                selected_room_key::<InterpretedBackend>(&weak),
                string_arg(args, 0),
                recent_speakers::<InterpretedBackend>(RECENT_SPEAKERS),
            );
        })?;

        bind_action(&self.instance, callback::MENTION_AT, |args| {
            mention_word_value(&string_arg(args, 0), int_arg(args, 1).unwrap_or_default())
        })?;

        bind_action(&self.instance, callback::COMPLETE_MENTION, |args| {
            let (text, caret) = mention::complete(
                &string_arg(args, 0),
                int_arg(args, 1).unwrap_or_default(),
                &string_arg(args, 2),
            );
            text_splice_value(text, caret)
        })?;

        bind_action(&self.instance, callback::EMOTE_AT, |args| {
            let (start, query) =
                emote::word_at(&string_arg(args, 0), int_arg(args, 1).unwrap_or_default());
            word_value(start, query)
        })?;

        bind_action(&self.instance, callback::PLACE_EMOTE, |args| {
            let (text, caret) = emote::place_picked(
                &string_arg(args, 0),
                int_arg(args, 1).unwrap_or_default(),
                &string_arg(args, 2),
                &string_arg(args, 3),
                bool_arg(args, 4),
            );
            text_splice_value(text, caret)
        })
    }

    fn bind_decode_requests(&self) -> Result<()> {
        bind_void(&self.instance, callback::REQUEST_MEDIA, move |args| {
            request_media(&string_arg(args, 0));
        })?;

        bind_void(&self.instance, callback::REQUEST_ROOM_AVATAR, move |args| {
            request_avatar(&AvatarSlot::Room(string_arg(args, 0)));
        })?;

        bind_void(&self.instance, callback::REQUEST_PACK_CELL, move |args| {
            request_pack_cell(&string_arg(args, 0));
        })?;

        bind_void(
            &self.instance,
            callback::REQUEST_CUSTOM_EMOJI,
            move |args| request_custom_emoji(&string_arg(args, 0)),
        )
    }

    fn bind_video_callbacks(&self) -> Result<()> {
        let toggle = on_main::<InterpretedBackend>(&self.instance, video::toggle);
        bind_void(&self.instance, callback::TOGGLE_VIDEO, move |_| toggle())?;

        let toggle_muted = on_main::<InterpretedBackend>(&self.instance, video::toggle_muted);
        bind_void(&self.instance, callback::TOGGLE_VIDEO_MUTED, move |_| {
            toggle_muted();
        })?;

        let weak = self.instance.as_weak();
        bind_void(&self.instance, callback::SEEK_VIDEO, move |args| {
            if let Some(window) = weak.upgrade() {
                video::seek(&window, millis_to_duration(usize_arg(args, 0)));
            }
        })
    }

    pub fn register_callbacks(
        &self,
        cmd_tx: &CommandSender,
        scroll_tx: &watch::Sender<ViewportChanged>,
        visibility_tx: &watch::Sender<TimelineVisibility>,
        composer_tx: &watch::Sender<ComposerActivity>,
        attention_tx: &watch::Sender<WindowAttention>,
    ) -> Result<()> {
        setup_emoji_store(&self.instance)?;
        call_window::install_commands(cmd_tx);
        bind_simple_callbacks(&self.instance, &self.instance, cmd_tx)?;
        self.bind_room_callbacks(cmd_tx)?;
        self.bind_reaction_callbacks(cmd_tx)?;
        self.bind_edit_callbacks(cmd_tx)?;
        self.bind_draft_callbacks()?;
        self.bind_mention_callbacks(cmd_tx)?;
        self.bind_decode_requests()?;
        self.bind_video_callbacks()?;
        self.bind_audio_callbacks(cmd_tx)?;
        self.bind_window_watches(scroll_tx, visibility_tx, composer_tx, attention_tx)
    }

    fn bind_room_callbacks(&self, cmd_tx: &CommandSender) -> Result<()> {
        let tx = cmd_tx.clone();
        bind_void(&self.instance, callback::MOVE_SPACE, move |args| {
            if let (Some(from), Some(to)) = (usize_arg(args, 0), usize_arg(args, 1)) {
                router::move_space(&tx, from, to, reorder_spaces::<InterpretedBackend>);
            }
        })?;

        let tx = cmd_tx.clone();
        bind_void(&self.instance, callback::DISMISS_UNSENT, move |args| {
            if let Some(submission) = int_arg(args, 0) {
                router::dismiss_unsent(&tx, submission);
            }
        })?;

        let tx = cmd_tx.clone();
        bind_void(&self.instance, callback::SET_ROOM_NOTIFY, move |args| {
            router::set_room_notify(&tx, string_arg(args, 0), variant_of(args.get(1)));
        })?;

        let tx = cmd_tx.clone();
        bind_action(&self.instance, callback::PASTE_ATTACHMENT, move |args| {
            Value::Bool(router::paste_attachment(&tx, string_arg(args, 0)))
        })?;

        let tx = cmd_tx.clone();
        bind_action(&self.instance, callback::ADD_KEYWORD, move |args| {
            Value::Bool(router::add_keyword(&tx, &string_arg(args, 0)))
        })?;

        bind_action(&self.instance, callback::ROOM_LOG_TEXT, |_| {
            text_value(shown_room_log_text())
        })?;

        bind_action(&self.instance, callback::ROOM_ROW_OF, |args| {
            num(list_keys::row_with_id(
                room_fields(args.get(1), "id"),
                &string_arg(args, 0),
            ))
        })?;

        bind_action(&self.instance, callback::ROOM_ROW_STARTING_WITH, |args| {
            num(list_keys::row_starting_with(
                &room_fields(args.get(1), "name"),
                &string_arg(args, 0),
                int_arg(args, 2).unwrap_or(-1),
            ))
        })
    }

    fn bind_reaction_callbacks(&self, cmd_tx: &CommandSender) -> Result<()> {
        let tx = cmd_tx.clone();
        bind_void(&self.instance, callback::TOGGLE_REACTION, move |args| {
            router::toggle_reaction(&tx, string_arg(args, 0), string_arg(args, 1));
        })?;

        let tx = cmd_tx.clone();
        bind_void(&self.instance, callback::REACT_WITH_EMOTE, move |args| {
            router::react_with_emote(
                &tx,
                string_arg(args, 0),
                string_arg(args, 1),
                string_arg(args, 2),
            );
        })?;

        let tx = cmd_tx.clone();
        bind_void(&self.instance, callback::SET_PACK_GLOBAL, move |args| {
            router::set_pack_global(&tx, string_arg(args, 0), bool_arg(args, 1));
        })
    }

    fn bind_window_watches(
        &self,
        scroll_tx: &watch::Sender<ViewportChanged>,
        visibility_tx: &watch::Sender<TimelineVisibility>,
        composer_tx: &watch::Sender<ComposerActivity>,
        attention_tx: &watch::Sender<WindowAttention>,
    ) -> Result<()> {
        let scroll_tx = scroll_tx.clone();
        let weak = self.instance.as_weak();
        bind_void(
            &self.instance,
            callback::SCROLL_POSITION_CHANGED,
            move |args| {
                router::scroll_position(
                    &scroll_tx,
                    int_arg(args, 2).and_then(|seen_timeline_token| {
                        adopted_room_key::<InterpretedBackend>(&weak, seen_timeline_token)
                    }),
                    bool_arg(args, 0),
                    unread_below::<InterpretedBackend>(int_arg(args, 1).unwrap_or_default()),
                );
            },
        )?;

        let visibility_tx = visibility_tx.clone();
        bind_void(
            &self.instance,
            callback::TIMELINE_VISIBILITY_CHANGED,
            move |args| router::timeline_visibility(&visibility_tx, bool_arg(args, 0)),
        )?;

        let composer_tx = composer_tx.clone();
        bind_void(&self.instance, callback::COMPOSER_CHANGED, move |args| {
            router::composer_changed(
                &composer_tx,
                string_arg(args, 0),
                bool_arg(args, 1),
                int_arg(args, 2).unwrap_or_default(),
            );
        })?;

        let focus_tx = attention_tx.clone();
        bind_void(
            &self.instance,
            callback::WINDOW_FOCUS_CHANGED,
            move |args| router::window_focus(&focus_tx, bool_arg(args, 0)),
        )?;

        let activity_tx = attention_tx.clone();
        bind_void(
            &self.instance,
            callback::USER_ACTIVITY_CHANGED,
            move |args| router::user_activity(&activity_tx, bool_arg(args, 0)),
        )
    }
}

pub(super) fn warn_unbound(bound: Result<()>, what: &str) {
    if let Err(e) = bound {
        tracing::warn!("failed to bind {what}: {e}");
    }
}

fn room_fields(rows: Option<&Value>, name: &str) -> Vec<String> {
    model_of(rows)
        .iter()
        .map(|row| match row {
            Value::Struct(row) => field(&row, name),
            _ => String::new(),
        })
        .collect()
}
