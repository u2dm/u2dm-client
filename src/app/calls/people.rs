use crate::commands::view::{
    CallFace, CallPerson, CallTile, CantDecrypt, TileKey, TileKind, TileState,
};
use crate::domain::call::{CallMember, Capabilities, Capability, MemberKey, RoomCall};

const FACES_SHOWN: usize = 3;

pub(super) fn faces(call: &RoomCall, me: Option<&str>) -> Vec<CallFace> {
    let face = |member: &CallMember| CallFace {
        user: member.user.clone(),
        avatar_mxc: member.avatar_mxc.clone(),
        you: Some(member.user.user_id.as_str()) == me,
    };
    let others: Vec<CallFace> = call
        .first_memberships()
        .filter(|member| Some(member.user.user_id.as_str()) != me)
        .take(FACES_SHOWN)
        .map(face)
        .collect();
    if others.is_empty() {
        return call.first_memberships().take(1).map(face).collect();
    }
    others
}

pub(super) fn of_tiles(
    tiles: &[CallTile],
    members: &[CallMember],
    shared_by_you: bool,
    self_focusable: bool,
) -> Vec<CallPerson> {
    let mut people: Vec<CallPerson> = Vec::new();
    for tile in tiles.iter().filter(|tile| tile.kind == TileKind::Person) {
        let declared = declared_by(tile, members);
        match people
            .iter_mut()
            .find(|person| person.user.user_id == tile.member.user)
        {
            Some(person) => add_device(person, tile, declared, self_focusable),
            None => people.push(first_device(tile, declared, self_focusable)),
        }
    }
    for tile in tiles.iter().filter(|tile| tile.kind == TileKind::Screen) {
        if let Some(person) = people
            .iter_mut()
            .find(|person| person.user.user_id == tile.member.user)
        {
            person.sharing = true;
        }
    }
    if let Some(index) = people.iter().position(|person| person.you) {
        let mut you = people.remove(index);
        you.sharing |= shared_by_you;
        people.insert(0, you);
    }
    people.sort_by_key(|person| (!person.you, person.hand.is_none(), person.hand));
    people
}

fn declared_by<'a>(tile: &CallTile, members: &'a [CallMember]) -> Option<&'a Capabilities> {
    if tile.own {
        return None;
    }
    members
        .iter()
        .find(|member| MemberKey::of(member) == tile.member)
        .map(|member| &member.capabilities)
}

fn first_device(
    tile: &CallTile,
    declared: Option<&Capabilities>,
    self_focusable: bool,
) -> CallPerson {
    CallPerson {
        user: tile.name.clone(),
        avatar_mxc: tile.avatar_mxc.clone(),
        you: tile.own,
        devices: 1,
        muted: tile.muted,
        muted_for_you: tile.muted_for_you,
        hand: tile.hand,
        reaction: tile.reaction.clone(),
        sharing: false,
        sees_no_video: sees_no_video(declared),
        exposes_e2ee: exposes_e2ee(declared),
        state: tile.state,
        tile: focusable(tile, self_focusable),
    }
}

fn add_device(
    person: &mut CallPerson,
    tile: &CallTile,
    declared: Option<&Capabilities>,
    self_focusable: bool,
) {
    person.devices += 1;
    person.muted &= tile.muted;
    person.muted_for_you |= tile.muted_for_you;
    if person.reaction.is_none() {
        person.reaction.clone_from(&tile.reaction);
    }
    person.hand = match (person.hand, tile.hand) {
        (Some(first), Some(other)) => Some(first.min(other)),
        (first, other) => first.or(other),
    };
    person.sees_no_video &= sees_no_video(declared);
    person.exposes_e2ee |= exposes_e2ee(declared);
    if person.avatar_mxc.is_none() {
        person.avatar_mxc.clone_from(&tile.avatar_mxc);
    }
    if telling(tile.state) > telling(person.state) {
        person.state = tile.state;
    }
    if tile.own {
        person.you = true;
        person.tile = focusable(tile, self_focusable);
    }
}

fn sees_no_video(declared: Option<&Capabilities>) -> bool {
    declared.and_then(|declared| declared.declares(Capability::RenderVideo)) == Some(false)
}

fn exposes_e2ee(declared: Option<&Capabilities>) -> bool {
    declared.and_then(|declared| declared.declares(Capability::ExposesE2ee)) == Some(true)
}

fn focusable(tile: &CallTile, self_focusable: bool) -> Option<TileKey> {
    (!tile.own || self_focusable).then(|| tile.key())
}

const fn telling(state: TileState) -> u8 {
    match state {
        TileState::Joining => 0,
        TileState::WaitingForKeys => 1,
        TileState::Live => 2,
        TileState::PoorConnection => 3,
        TileState::VideoStalled => 4,
        TileState::NotReceived(_) => 5,
        TileState::ConnectionLost => 6,
        TileState::Unreachable => 7,
        TileState::CantDecrypt(CantDecrypt::NoKey) => 8,
        TileState::CantDecrypt(CantDecrypt::NotCrossSigned) => 9,
    }
}
