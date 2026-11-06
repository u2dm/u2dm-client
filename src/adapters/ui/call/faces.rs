use std::collections::HashSet;

use slint::Image;

use super::shown_name;
use crate::adapters::ui::backend::UiBackend;
use crate::adapters::ui::decode::{AvatarSlot, load_avatar_async, peek_avatar};
use crate::adapters::ui::dto::{call_face_to_dto, call_person_to_dto};
use crate::adapters::ui::fields::{CallPersonFields, CallTileFields};
use crate::adapters::ui::props::{IntProp, StringProp, UiProps};
use crate::adapters::ui::rows::patch_rows_where;
use crate::commands::view::{ActiveCall, CallFace, CallPerson};
use crate::domain::member_name::NamedUser;
use crate::ports::media::MediaCache;
use crate::util::saturating_i32;

pub(super) fn apply_people<B: UiBackend>(
    last: Option<&[CallPerson]>,
    people: &[CallPerson],
    media: &dyn MediaCache,
) {
    if last == Some(people) {
        return;
    }
    let rows: Vec<B::CallPerson> = people
        .iter()
        .map(|person| {
            let mut row = B::CallPerson::from(call_person_to_dto(person));
            if let Some(face) = face_of(person.avatar_mxc.as_deref(), media) {
                row.set_avatar(face);
                row.set_has_avatar(true);
            }
            row
        })
        .collect();
    B::models().call_people.set_vec(rows);
}

pub(super) fn apply_here_faces<B: UiBackend>(faces: &[CallFace], media: &dyn MediaCache) {
    let rows: Vec<B::CallPerson> = faces
        .iter()
        .map(|face| {
            let mut row = B::CallPerson::from(call_face_to_dto(face));
            if let Some(image) = face_of(face.avatar_mxc.as_deref(), media) {
                row.set_avatar(image);
                row.set_has_avatar(true);
            }
            row
        })
        .collect();
    B::models().call_here_faces.set_vec(rows);
    let faces = faces
        .iter()
        .map(|face| (face.user.user_id.as_str(), face.avatar_mxc.as_deref()));
    request_faces::<B>(faces, media);
}

pub(super) fn face_of(mxc: Option<&str>, media: &dyn MediaCache) -> Option<Image> {
    let path = media.user_avatar_path(mxc?)?;
    peek_avatar(&path)
}

pub(super) fn faces_in_call(active: &ActiveCall) -> impl Iterator<Item = (&str, Option<&str>)> {
    let tiles = active
        .stage
        .iter()
        .chain(active.tiles.iter())
        .chain(active.compact_self.iter())
        .map(|tile| (tile.name.user_id.as_str(), tile.avatar_mxc.as_deref()));
    let people = active
        .people
        .iter()
        .map(|person| (person.user.user_id.as_str(), person.avatar_mxc.as_deref()));
    tiles.chain(people)
}

pub(super) fn request_faces<'a, B: UiBackend>(
    faces: impl Iterator<Item = (&'a str, Option<&'a str>)>,
    media: &dyn MediaCache,
) {
    let mut asked: HashSet<&str> = HashSet::new();
    for (user_id, mxc) in faces {
        let Some(mxc) = mxc else {
            continue;
        };
        if !asked.insert(user_id) {
            continue;
        }
        let path = media.user_avatar_path(mxc);
        let slot = AvatarSlot::CallFace(user_id.to_owned());
        if let Some(face) = load_avatar_async(path.as_deref(), slot) {
            show_faces::<B>(&HashSet::from([user_id]), &face);
        }
    }
}

pub fn show_faces<B: UiBackend>(users: &HashSet<&str>, face: &Image) {
    if users.is_empty() {
        return;
    }
    let models = B::models();
    for rows in [&models.call_stage, &models.call_tiles, &models.call_self] {
        patch_rows_where(
            &**rows,
            |entry| !entry.screen() && !entry.has_avatar() && users.contains(entry.user_id()),
            |entry| {
                entry.set_avatar(face.clone());
                entry.set_has_avatar(true);
            },
        );
    }
    for people in [&models.call_people, &models.call_here_faces] {
        patch_rows_where(
            &**people,
            |entry| !entry.has_avatar() && users.contains(entry.user_id()),
            |entry| {
                entry.set_avatar(face.clone());
                entry.set_has_avatar(true);
            },
        );
    }
}

pub(super) fn apply_names(
    w: &impl UiProps,
    names: &[NamedUser],
    [first, second]: [StringProp; 2],
    more: IntProp,
) {
    w.set_text(first, shown_name(names.first()));
    w.set_text(second, shown_name(names.get(1)));
    w.set_int(more, saturating_i32(names.len().saturating_sub(2)));
}
