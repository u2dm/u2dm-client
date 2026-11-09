use slint::ComponentHandle;

use super::run_change_handlers_next_frame;
use crate::adapters::ui::decode::set_animations_paused;
use crate::adapters::ui::props::{BoolProp, UiProps};
use crate::commands::view::{SettingsPage, SettingsView};
use crate::domain::settings::{
    Alerts, Appearance, CallPreferences, MediaPreferences, Messaging, Settings,
};

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
    let SettingsView { values, dialog } = view;
    w.set_bool(BoolProp::SettingsVisible, dialog.is_some());
    w.set_settings_page(dialog.map_or(SettingsPage::default(), |dialog| dialog.page));
    apply_values(w, values);
    if last.is_none_or(|last| {
        last.values.appearance.theme != values.appearance.theme
            || last.values.media.play_animations != values.media.play_animations
    }) {
        run_change_handlers_next_frame(w);
    }
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
