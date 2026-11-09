use serde::{Deserialize, Deserializer, de};

use super::Rejected;
use super::driven::Driven;
use crate::adapters::demo::probe::names;
use crate::commands::ui::{DepartureCommand, SettingsCommand, UiCommand};
use crate::domain::call::devices::DeviceKind;
use crate::domain::departure::membership_reason;
use crate::domain::room::RoomId;
use crate::domain::settings::SettingChange;
use crate::domain::user_info::UserId;

pub type Selection<'a> = Option<&'a (String, i32)>;

pub(super) fn settings_page(page: &str) -> Result<Driven, Rejected> {
    names::settings_page_named(page)
        .map(|page| Driven::Command(SettingsCommand::ShowPage(page).into()))
        .ok_or_else(|| Rejected(format!("{page} is not a settings page")))
}

pub(super) fn setting_change(name: &str, value: &str) -> Result<Driven, Rejected> {
    let change = match name {
        "theme" => names::theme_choice_named(value).map(SettingChange::Theme),
        "motion" => names::motion_choice_named(value).map(SettingChange::Motion),
        "send_key" => names::send_key_named(value).map(SettingChange::SendKey),
        "receipts" => names::receipt_visibility_named(value).map(SettingChange::Receipts),
        "call_layout" => names::call_layout_named(value).map(SettingChange::CallLayout),
        flag => setting_flag_change(flag, value)?,
    };
    change
        .map(|change| Driven::Command(SettingsCommand::Change(change).into()))
        .ok_or_else(|| Rejected(format!("{value} is not a value of {name}")))
}

fn setting_flag_change(name: &str, value: &str) -> Result<Option<SettingChange>, Rejected> {
    let flag = names::setting_flag_named(name)
        .ok_or_else(|| Rejected(format!("{name} is not a setting")))?;
    Ok(value.parse().ok().map(|on| SettingChange::Flag(flag, on)))
}

pub(super) struct Target {
    pub(super) room_id: RoomId,
    pub(super) generation: i32,
}

pub(super) fn target(
    explicit_room: Option<String>,
    explicit_generation: Option<i32>,
    selected: Selection<'_>,
) -> Result<Target, Rejected> {
    let room_id = explicit_room
        .or_else(|| selected.map(|(id, _)| id.clone()))
        .map(RoomId::new)
        .ok_or_else(|| Rejected("no room_id was given and no room is selected".to_owned()))?;
    let generation = explicit_generation
        .or_else(|| selected.map(|(_, current)| *current))
        .unwrap_or_default();
    Ok(Target {
        room_id,
        generation,
    })
}

pub(super) fn room(explicit: Option<String>, selected: Selection<'_>) -> Result<RoomId, Rejected> {
    Ok(target(explicit, None, selected)?.room_id)
}

pub(super) fn in_room<C: Into<UiCommand>>(
    explicit: Option<String>,
    selected: Selection<'_>,
    command: impl FnOnce(RoomId) -> C,
) -> Result<Driven, Rejected> {
    Ok(Driven::Command(command(room(explicit, selected)?).into()))
}

pub(super) fn in_space<C: Into<UiCommand>>(
    space_id: String,
    command: impl FnOnce(RoomId) -> C,
) -> Result<Driven, Rejected> {
    if space_id.is_empty() {
        return Err(Rejected("an empty space_id names no space".to_owned()));
    }
    Ok(Driven::Command(command(RoomId::new(space_id)).into()))
}

pub(super) fn leave_space(with_rooms: bool) -> impl FnOnce(RoomId) -> DepartureCommand {
    move |space_id| DepartureCommand::LeaveSpace {
        space_id,
        with_rooms,
    }
}

pub(super) fn moderation(
    user_id: String,
    reason: String,
    command: fn(UserId, Option<String>) -> UiCommand,
) -> Result<Driven, Rejected> {
    person(user_id, |user_id| {
        command(user_id, membership_reason(reason))
    })
}

pub(super) fn kick(user_id: UserId, reason: Option<String>) -> UiCommand {
    UiCommand::KickUser { user_id, reason }
}

pub(super) fn ban(user_id: UserId, reason: Option<String>) -> UiCommand {
    UiCommand::BanUser { user_id, reason }
}

pub(super) fn person(
    user_id: String,
    command: impl FnOnce(UserId) -> UiCommand,
) -> Result<Driven, Rejected> {
    if user_id.is_empty() {
        return Err(Rejected("an empty user_id names nobody".to_owned()));
    }
    Ok(Driven::Command(command(UserId::new(user_id))))
}

pub(super) fn device_kind<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<DeviceKind, D::Error> {
    let name = String::deserialize(deserializer)?;
    names::call_device_kind_named(&name).ok_or_else(|| {
        de::Error::custom(format!(
            "{name} is not a call device kind; use microphone, speaker or camera"
        ))
    })
}

pub(super) fn room_notify(
    explicit: Option<String>,
    mode: &str,
    selected: Selection<'_>,
) -> Result<UiCommand, Rejected> {
    let mode = names::notify_mode_named(mode).ok_or_else(|| {
        Rejected(format!(
            "{mode} is not a notification mode; use all-messages, mentions-only or muted"
        ))
    })?;
    Ok(UiCommand::SetRoomNotify {
        room_id: room(explicit, selected)?,
        mode,
    })
}
