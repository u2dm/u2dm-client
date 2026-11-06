use std::collections::{HashMap, HashSet};

use slint::{Image, Model};

use super::UiBackend;
use super::art::apply_pack_avatars;
use crate::adapters::ui::call;
use crate::adapters::ui::decode::{AvatarSlot, DecodeOutcome};
use crate::adapters::ui::fields::{
    InviteeFields, MemberRowFields, MessageFields, ReactionFields, ReactorFields, RoomFields,
    SpaceChildFields, SpaceFields,
};
use crate::adapters::ui::props::{ImageProp, UiProps};
use crate::adapters::ui::reconcile::timeline_row_of;
use crate::adapters::ui::rows::{locate_row, patch_first, patch_rows_by_id};
use crate::adapters::ui::splice_model::SpliceModel;

#[derive(Default)]
struct AvatarTargets<'a> {
    messages: HashSet<&'a str>,
    reactors_by_message: HashMap<&'a str, HashSet<&'a str>>,
    rooms: HashSet<&'a str>,
    spaces: HashSet<&'a str>,
    space_children: HashSet<&'a str>,
    members: HashSet<&'a str>,
    readers: HashSet<&'a str>,
    mentions: HashSet<&'a str>,
    invitees: HashSet<&'a str>,
    call_faces: HashSet<&'a str>,
    singles: HashSet<ImageProp>,
}

fn group_slots(slots: &[AvatarSlot]) -> AvatarTargets<'_> {
    let mut targets = AvatarTargets::default();
    for slot in slots {
        match slot {
            AvatarSlot::Message(item) => targets.messages.extend([item.unique_id()]),
            AvatarSlot::Reactor { item, user_id } => targets
                .reactors_by_message
                .entry(item.unique_id())
                .or_default()
                .extend([user_id.as_str()]),
            AvatarSlot::Room(id) => targets.rooms.extend([id.as_str()]),
            AvatarSlot::Space(id) => targets.spaces.extend([id.as_str()]),
            AvatarSlot::SpaceChild(id) => targets.space_children.extend([id.as_str()]),
            AvatarSlot::Member(user_id) => targets.members.extend([user_id.as_str()]),
            AvatarSlot::Reader(user_id) => targets.readers.extend([user_id.as_str()]),
            AvatarSlot::Mention(user_id) => targets.mentions.extend([user_id.as_str()]),
            AvatarSlot::Invitee(user_id) => targets.invitees.extend([user_id.as_str()]),
            AvatarSlot::SelectedRoom => targets.singles.extend([ImageProp::SelectedRoomAvatar]),
            AvatarSlot::RoomInfo => targets.singles.extend([ImageProp::RoomInfoAvatar]),
            AvatarSlot::UserInfo => targets.singles.extend([ImageProp::UserInfoAvatar]),
            AvatarSlot::Invite => targets.singles.extend([ImageProp::InviteAvatar]),
            AvatarSlot::Removal => targets.singles.extend([ImageProp::RemovalAvatar]),
            AvatarSlot::RoomPreview => targets.singles.extend([ImageProp::RoomPreviewAvatar]),
            AvatarSlot::Ring => targets.singles.extend([ImageProp::RingAvatar]),
            AvatarSlot::CallFace(user_id) => targets.call_faces.extend([user_id.as_str()]),
            AvatarSlot::User => targets.singles.extend([ImageProp::UserAvatar]),
            AvatarSlot::AttachmentPreview { .. } => {
                targets.singles.extend([ImageProp::AttachmentPreview]);
            }
            AvatarSlot::Pack(_) => {}
        }
    }
    targets
}

pub(super) fn indexed_timeline_row<B: UiBackend>(
    timeline: &SpliceModel<B::Message>,
    unique_id: &str,
) -> Option<usize> {
    locate_row(
        timeline,
        &B::Message::unique_id,
        unique_id,
        timeline_row_of(unique_id)?,
    )
}

fn patch_message_avatars<B: UiBackend>(
    timeline: &SpliceModel<B::Message>,
    unique_ids: &HashSet<&str>,
    image: &Image,
) {
    for unique_id in unique_ids {
        let Some(row) = indexed_timeline_row::<B>(timeline, unique_id) else {
            continue;
        };
        let Some(mut entry) = timeline.row_data(row) else {
            continue;
        };
        entry.set_avatar(image.clone());
        entry.set_has_avatar(true);
        timeline.set_row_data(row, entry);
    }
}

fn patch_reactor_avatars<B: UiBackend>(
    timeline: &SpliceModel<B::Message>,
    reactors_by_message: &HashMap<&str, HashSet<&str>>,
    image: &Image,
) {
    for (unique_id, user_ids) in reactors_by_message {
        let Some(entry) =
            indexed_timeline_row::<B>(timeline, unique_id).and_then(|row| timeline.row_data(row))
        else {
            continue;
        };
        for user_id in user_ids {
            patch_reactor_avatar::<B>(&entry, user_id, image);
        }
    }
}

fn patch_reactor_avatar<B: UiBackend>(entry: &B::Message, user_id: &str, image: &Image) {
    for reaction in entry.reactions().iter() {
        patch_first(
            &reaction.avatars(),
            |face| face.user_id() == user_id && !face.has_avatar(),
            |face| {
                face.set_avatar(image.clone());
                face.set_has_avatar(true);
            },
        );
    }
}

pub(super) fn apply_avatar_ready<B: UiBackend>(
    weak: &slint::Weak<B::Window>,
    slots: &[AvatarSlot],
    outcome: DecodeOutcome<'_>,
) {
    let image = match outcome {
        DecodeOutcome::Ready(image) => image,
        DecodeOutcome::Failed(_) => {
            apply_pack_avatars::<B>(slots, None);
            return;
        }
        DecodeOutcome::Deferred => return,
    };
    apply_pack_avatars::<B>(slots, Some(image));
    let targets = group_slots(slots);
    if !targets.singles.is_empty()
        && let Some(w) = weak.upgrade()
    {
        for single in &targets.singles {
            w.set_image(*single, Some(image.clone()));
        }
    }
    let models = B::models();
    patch_message_avatars::<B>(&models.timeline, &targets.messages, image);
    patch_reactor_avatars::<B>(&models.timeline, &targets.reactors_by_message, image);
    let show_room = |entry: &mut B::Room| {
        entry.set_avatar(image.clone());
        entry.set_has_avatar(true);
    };
    patch_rows_by_id(&*models.rooms, &targets.rooms, &B::Room::id, show_room);
    patch_rows_by_id(&*models.rooms, &targets.spaces, &B::Room::id, show_room);
    for spaces in [&models.spaces, &models.subspaces] {
        patch_rows_by_id(&**spaces, &targets.spaces, &B::Space::id, |entry| {
            entry.set_avatar(image.clone());
            entry.set_has_avatar(true);
        });
    }
    if !targets.space_children.is_empty() {
        patch_rows_by_id(
            &*models.space_children,
            &targets.space_children,
            &B::SpaceChild::id,
            |entry| {
                entry.set_avatar(image.clone());
                entry.set_has_avatar(true);
            },
        );
    }
    patch_person_avatars::<B>(&targets, image);
    call::show_faces::<B>(&targets.call_faces, image);
}

fn patch_person_avatars<B: UiBackend>(targets: &AvatarTargets<'_>, image: &Image) {
    let models = B::models();
    for (rows, ids) in [
        (&models.room_members, &targets.members),
        (&models.readers, &targets.readers),
        (&models.mentions, &targets.mentions),
    ] {
        if !ids.is_empty() {
            patch_rows_by_id(&**rows, ids, &B::MemberRow::user_id, |entry| {
                entry.set_avatar(image.clone());
                entry.set_has_avatar(true);
            });
        }
    }
    if !targets.invitees.is_empty() {
        for rows in [&models.invitee_results, &models.invitee_picks] {
            patch_rows_by_id(&**rows, &targets.invitees, &B::Invitee::user_id, |entry| {
                entry.set_avatar(image.clone());
                entry.set_has_avatar(true);
            });
        }
    }
}
