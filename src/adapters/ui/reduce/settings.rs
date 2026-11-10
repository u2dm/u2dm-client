use slint::ComponentHandle;

use super::run_change_handlers_next_frame;
use crate::adapters::ui::backend::{UiBackend, UiEventContext};
use crate::adapters::ui::decode::{forget_failed_decodes, set_animations_paused};
use crate::adapters::ui::dto::{ignored_person_to_dto, keyword_to_dto};
use crate::adapters::ui::props::{BoolProp, StringProp, UiProps};
use crate::commands::view::{
    DownloadedMedia, IgnoredUsersView, NotificationRulesView, SettingsPage, SettingsView,
};
use crate::domain::settings::{
    Alerts, Appearance, CallPreferences, MediaPreferences, Messaging, Settings,
};
use crate::util::format_bytes;

pub(in crate::adapters::ui) fn seed_settings<W: UiProps>(w: &W, values: &Settings) {
    apply_values(w, values);
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
        appearance: Appearance { theme, motion },
        messaging:
            Messaging {
                send_key,
                receipts,
                typing_notices,
                share_presence,
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
        media: MediaPreferences {
            auto_load,
            play_animations,
        },
    } = *values;
    w.set_appearance_theme(theme);
    w.set_motion_choice(motion);
    w.set_send_key(send_key);
    w.set_receipt_visibility(receipts);
    w.set_bool(BoolProp::SettingsTypingNotices, typing_notices);
    w.set_bool(BoolProp::SettingsSharePresence, share_presence);
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
    set_animations_paused(!play_animations);
}
