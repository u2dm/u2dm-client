use super::Tx;
use crate::adapters::ui::props::send_command;
use crate::adapters::ui::schema::{
    call_layouts, clock_choices, motion_choices, receipt_visibilities, room_classes, room_defaults,
    send_keys, setting_flags, settings_layouts, settings_pages, theme_choices, variant_named,
};
use crate::commands::ui::{RuleCommand, SettingsCommand};
use crate::commands::view::{SettingsLayout, SettingsPage};
use crate::domain::server_rules::{Keyword, RoomClass, RoomDefault};
use crate::domain::settings::quick_reactions::{QuickReaction, QuickReactionChange, ReactionSlot};
use crate::domain::settings::{
    CallLayout, ClockChoice, MotionChoice, ReceiptVisibility, SendKey, SettingChange, SettingFlag,
    SkinTone, ThemeChoice,
};

settings_pages!(variant_named settings_page_named SettingsPage;);
settings_layouts!(variant_named settings_layout_named SettingsLayout;);
theme_choices!(variant_named theme_choice_named ThemeChoice;);
motion_choices!(variant_named motion_choice_named MotionChoice;);
clock_choices!(variant_named clock_choice_named ClockChoice;);
send_keys!(variant_named send_key_named SendKey;);
receipt_visibilities!(variant_named receipt_visibility_named ReceiptVisibility;);
call_layouts!(variant_named call_layout_named CallLayout;);
setting_flags!(variant_named setting_flag_named SettingFlag;);
room_classes!(variant_named room_class_named RoomClass;);
room_defaults!(variant_named room_default_named RoomDefault;);

pub fn open_settings(tx: &Tx) {
    send_command(tx, SettingsCommand::Open(None));
}

pub fn open_settings_page(tx: &Tx, page: &str) {
    let Some(page) = settings_page_named(page) else {
        tracing::warn!(page, "ignoring an unknown settings page");
        return;
    };
    send_command(tx, SettingsCommand::Open(Some(page)));
}

pub fn settings_layout(tx: &Tx, layout: &str) {
    let Some(layout) = settings_layout_named(layout) else {
        tracing::warn!(layout, "ignoring an unknown settings layout");
        return;
    };
    send_command(tx, SettingsCommand::Layout(layout));
}

pub fn show_settings_page(tx: &Tx, page: &str) {
    let Some(page) = settings_page_named(page) else {
        tracing::warn!(page, "ignoring an unknown settings page");
        return;
    };
    send_command(tx, SettingsCommand::ShowPage(page));
}

pub fn set_theme(tx: &Tx, choice: &str) {
    let Some(theme) = theme_choice_named(choice) else {
        tracing::warn!(choice, "ignoring an unknown theme");
        return;
    };
    send_command(tx, SettingsCommand::Change(SettingChange::Theme(theme)));
}

pub fn set_motion(tx: &Tx, choice: &str) {
    let Some(motion) = motion_choice_named(choice) else {
        tracing::warn!(choice, "ignoring an unknown motion choice");
        return;
    };
    send_command(tx, SettingsCommand::Change(SettingChange::Motion(motion)));
}

pub fn set_clock(tx: &Tx, choice: &str) {
    let Some(clock) = clock_choice_named(choice) else {
        tracing::warn!(choice, "ignoring an unknown time format");
        return;
    };
    send_command(tx, SettingsCommand::Change(SettingChange::Clock(clock)));
}

pub fn set_send_key(tx: &Tx, choice: &str) {
    let Some(key) = send_key_named(choice) else {
        tracing::warn!(choice, "ignoring an unknown send key");
        return;
    };
    send_command(tx, SettingsCommand::Change(SettingChange::SendKey(key)));
}

pub fn set_receipts(tx: &Tx, choice: &str) {
    let Some(receipts) = receipt_visibility_named(choice) else {
        tracing::warn!(choice, "ignoring an unknown receipt visibility");
        return;
    };
    send_command(
        tx,
        SettingsCommand::Change(SettingChange::Receipts(receipts)),
    );
}

pub fn set_call_layout(tx: &Tx, choice: &str) {
    let Some(layout) = call_layout_named(choice) else {
        tracing::warn!(choice, "ignoring an unknown call layout");
        return;
    };
    send_command(
        tx,
        SettingsCommand::Change(SettingChange::CallLayout(layout)),
    );
}

pub fn set_setting_flag(tx: &Tx, setting: &str, on: bool) {
    let Some(flag) = setting_flag_named(setting) else {
        tracing::warn!(setting, "ignoring an unknown setting");
        return;
    };
    send_command(tx, SettingsCommand::Change(SettingChange::Flag(flag, on)));
}

pub fn set_skin_tone(tx: &Tx, tone: i32) {
    let Some(tone) = u8::try_from(tone).ok().and_then(SkinTone::new) else {
        tracing::warn!(tone, "ignoring an unknown skin tone");
        return;
    };
    send_command(tx, SettingsCommand::Change(SettingChange::SkinTone(tone)));
}

pub fn set_quick_reaction(tx: &Tx, slot: i32, key: &str) {
    let Some(slot) = reaction_slot(slot) else {
        tracing::warn!(
            slot,
            "ignoring a quick reaction for a slot that does not exist"
        );
        return;
    };
    let Some(key) = QuickReaction::parse(key) else {
        tracing::warn!(
            slot = slot.index(),
            "ignoring a quick reaction that is not an emoji"
        );
        return;
    };
    send_command(
        tx,
        SettingsCommand::QuickReactions(QuickReactionChange::Place(slot, key)),
    );
}

pub fn set_one_click_reaction(tx: &Tx, slot: i32) {
    let Some(slot) = reaction_slot(slot) else {
        tracing::warn!(
            slot,
            "ignoring a one-click reaction for a slot that does not exist"
        );
        return;
    };
    send_command(
        tx,
        SettingsCommand::QuickReactions(QuickReactionChange::OneClick(slot)),
    );
}

pub fn reset_quick_reactions(tx: &Tx) {
    send_command(
        tx,
        SettingsCommand::QuickReactions(QuickReactionChange::Reset),
    );
}

fn reaction_slot(slot: i32) -> Option<ReactionSlot> {
    usize::try_from(slot).ok().and_then(ReactionSlot::new)
}

pub fn set_room_default(tx: &Tx, rooms: &str, mode: &str) {
    let (Some(rooms), Some(mode)) = (room_class_named(rooms), room_default_named(mode)) else {
        tracing::warn!(rooms, mode, "ignoring an unknown room default");
        return;
    };
    send_command(tx, RuleCommand::SetRoomDefault(rooms, mode));
}

pub fn add_keyword(tx: &Tx, text: &str) -> bool {
    let Some(keyword) = Keyword::parse(text) else {
        return false;
    };
    send_command(tx, RuleCommand::AddKeyword(keyword));
    true
}
