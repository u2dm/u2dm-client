use slint::{ComponentHandle, SharedString};

use super::run_change_handlers_next_frame;
use super::sheets::apply_joined_on;
use crate::adapters::ui::backend::{UiBackend, UiEventContext};
use crate::adapters::ui::clock::refresh_room_labels;
use crate::adapters::ui::decode::{forget_failed_decodes, set_animations_paused};
use crate::adapters::ui::dto::{ignored_person_to_dto, keyword_to_dto};
use crate::adapters::ui::fields::MessageFields;
use crate::adapters::ui::present::{message_sent_at_label, message_timestamp_label, use_clock};
use crate::adapters::ui::props::{BoolProp, IntProp, StringProp, UiProps};
use crate::adapters::ui::reconcile::{RowTime, restamp_rows};
use crate::commands::view::{
    AppViewState, DownloadedMedia, IgnoredUsersView, NotificationRulesView, SettingsPage,
    SettingsView,
};
use crate::domain::settings::{
    Alerts, Appearance, CallPreferences, MediaPreferences, Messaging, ReactionPreferences,
    RowFilter, Settings,
};
use crate::util::{format_bytes, saturating_i32};

pub(in crate::adapters::ui) fn seed_settings<W: UiProps>(w: &W, values: &Settings) {
    use_clock(values.appearance.clock);
    apply_values(w, values);
    w.apply_quick_reactions(&values.reactions.quick);
}

pub(super) fn apply_settings<W: UiProps + ComponentHandle>(
    w: &W,
    last: Option<&SettingsView>,
    view: &SettingsView,
) {
    if last == Some(view) {
        return;
    }
    let SettingsView {
        values,
        unsaved,
        dialog,
        media_cleared,
        rules: _,
        ignored: _,
    } = view;
    w.set_bool(BoolProp::SettingsVisible, dialog.is_some());
    w.set_bool(BoolProp::SettingsUnsaved, *unsaved);
    w.set_settings_page(dialog.map_or(SettingsPage::default(), |dialog| dialog.page));
    w.set_bool(
        BoolProp::SettingsListing,
        dialog.is_some_and(|dialog| dialog.listing),
    );
    w.set_bool(
        BoolProp::SettingsMediaClearFailed,
        dialog.is_some_and(|dialog| dialog.clear_failed),
    );
    apply_storage(
        w,
        dialog.map_or(DownloadedMedia::default(), |dialog| dialog.storage),
    );
    apply_values(w, values);
    if last.is_none_or(|last| last.values.reactions.quick.keys() != values.reactions.quick.keys()) {
        w.apply_quick_reactions(&values.reactions.quick);
    }
    if last.is_some_and(|last| last.media_cleared != *media_cleared) {
        forget_failed_decodes();
    }
    if last.is_none_or(|last| {
        last.values.appearance.theme != values.appearance.theme
            || last.values.media.play_animations != values.media.play_animations
    }) {
        run_change_handlers_next_frame(w);
    }
}

pub(super) fn follow_clock<B: UiBackend>(
    w: &B::Window,
    view: &AppViewState,
    ctx: &UiEventContext<'_, B>,
) {
    if !use_clock(view.settings.values.appearance.clock) {
        return;
    }
    tracing::debug!("times on screen follow the new clock");
    restamp_rows(
        &ctx.models.timeline,
        &|entry| entry.unique_id(),
        &restamp_message::<B>,
    );
    refresh_room_labels::<B>(&view.directory, ctx.media);
    apply_joined_on(w, view.room_info.about.as_ref());
}

fn restamp_message<B: UiBackend>(entry: &mut B::Message, time: RowTime) -> bool {
    let timestamp = SharedString::from(message_timestamp_label(time.at));
    let sent_at = if time.own {
        SharedString::from(message_sent_at_label(time.at))
    } else {
        SharedString::new()
    };
    if entry.timestamp() == timestamp.as_str() && entry.sent_at() == sent_at.as_str() {
        return false;
    }
    entry.set_timestamp(timestamp);
    entry.set_sent_at(sent_at);
    true
}

pub(super) fn apply_notification_rules<B: UiBackend>(
    w: &B::Window,
    last: Option<&NotificationRulesView>,
    rules: &NotificationRulesView,
    ctx: &UiEventContext<'_, B>,
) {
    if last == Some(rules) {
        return;
    }
    let NotificationRulesView {
        known,
        failed,
        account,
        direct,
        group,
        keywords,
    } = rules;
    w.set_bool(BoolProp::SettingsRulesKnown, *known);
    w.set_bool(BoolProp::SettingsRulesFailed, *failed);
    w.set_account_rule(*account);
    w.set_direct_default(*direct);
    w.set_group_default(*group);
    let rows = keywords
        .iter()
        .map(keyword_to_dto)
        .map(B::Keyword::from)
        .collect::<Vec<_>>();
    ctx.models.keywords.set_vec(rows);
}

pub(super) fn apply_ignored_users<B: UiBackend>(
    w: &B::Window,
    last: Option<&IgnoredUsersView>,
    ignored: &IgnoredUsersView,
    ctx: &UiEventContext<'_, B>,
) {
    if last == Some(ignored) {
        return;
    }
    let IgnoredUsersView {
        known,
        failed,
        people,
    } = ignored;
    w.set_bool(BoolProp::SettingsIgnoredKnown, *known);
    w.set_text(
        StringProp::SettingsUnignoreFailed,
        failed.clone().unwrap_or_default(),
    );
    let rows = people
        .iter()
        .map(ignored_person_to_dto)
        .map(B::IgnoredPerson::from)
        .collect::<Vec<_>>();
    ctx.models.ignored_people.set_vec(rows);
}

fn apply_storage<W: UiProps>(w: &W, storage: DownloadedMedia) {
    let usage = storage.usage().unwrap_or_default();
    w.set_downloaded_media(storage);
    w.set_text(
        StringProp::SettingsClearableSize,
        format_bytes(usage.clearable),
    );
    w.set_text(StringProp::SettingsKeptSize, format_bytes(usage.kept));
    w.set_bool(BoolProp::SettingsHasClearableMedia, usage.clearable > 0);
}

fn apply_values<W: UiProps>(w: &W, values: &Settings) {
    let Settings {
        appearance: Appearance {
            theme,
            motion,
            clock,
        },
        messaging:
            Messaging {
                send_key,
                receipts,
                typing_notices,
                share_presence,
            },
        timeline: RowFilter {
            joins_and_leaves,
            profile_changes,
        },
        reactions: ReactionPreferences {
            ref quick,
            skin_tone,
        },
        alerts:
            Alerts {
                desktop_notifications,
                hide_message_text,
                notification_sound,
                ringtone,
            },
        calls:
            CallPreferences {
                remember_devices,
                camera_on_join,
                layout,
                follow_speaker,
            },
        remembered_devices: _,
        media:
            MediaPreferences {
                auto_load,
                play_animations,
                voice_in_a_row,
            },
    } = *values;
    w.set_appearance_theme(theme);
    w.set_motion_choice(motion);
    w.set_clock_choice(clock);
    w.set_send_key(send_key);
    w.set_receipt_visibility(receipts);
    w.set_int(
        IntProp::OneClickReaction,
        saturating_i32(quick.one_click().index()),
    );
    w.set_bool(BoolProp::SettingsQuickReactionsCustom, !quick.is_default());
    w.set_int(IntProp::SkinTone, i32::from(skin_tone.index()));
    w.set_bool(BoolProp::SettingsTypingNotices, typing_notices);
    w.set_bool(BoolProp::SettingsSharePresence, share_presence);
    w.set_bool(BoolProp::SettingsJoinsAndLeaves, joins_and_leaves);
    w.set_bool(BoolProp::SettingsProfileChanges, profile_changes);
    w.set_bool(
        BoolProp::SettingsDesktopNotifications,
        desktop_notifications,
    );
    w.set_bool(BoolProp::SettingsHideMessageText, hide_message_text);
    w.set_bool(BoolProp::SettingsNotificationSound, notification_sound);
    w.set_bool(BoolProp::SettingsRingtone, ringtone);
    w.set_bool(BoolProp::SettingsRememberDevices, remember_devices);
    w.set_bool(BoolProp::SettingsCameraOnJoin, camera_on_join);
    w.set_call_layout(layout);
    w.set_bool(BoolProp::SettingsFollowSpeaker, follow_speaker);
    w.set_bool(BoolProp::SettingsAutoLoadMedia, auto_load);
    w.set_bool(BoolProp::SettingsPlayAnimations, play_animations);
    w.set_bool(BoolProp::SettingsVoiceInARow, voice_in_a_row);
    set_animations_paused(!play_animations);
}
