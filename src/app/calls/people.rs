use crate::commands::view::{CallPerson, CallTile, CantDecrypt, TileKey, TileKind, TileState};

pub(super) fn of_tiles(
    tiles: &[CallTile],
    shared_by_you: bool,
    self_focusable: bool,
) -> Vec<CallPerson> {
    let mut people: Vec<CallPerson> = Vec::new();
    for tile in tiles.iter().filter(|tile| tile.kind == TileKind::Person) {
        match people
            .iter_mut()
            .find(|person| person.user.user_id == tile.member.user)
        {
            Some(person) => add_device(person, tile, self_focusable),
            None => people.push(first_device(tile, self_focusable)),
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
    people
}

fn first_device(tile: &CallTile, self_focusable: bool) -> CallPerson {
    CallPerson {
        user: tile.name.clone(),
        avatar_mxc: tile.avatar_mxc.clone(),
        you: tile.own,
        devices: 1,
        muted: tile.muted,
        sharing: false,
        state: tile.state,
        tile: focusable(tile, self_focusable),
    }
}

fn add_device(person: &mut CallPerson, tile: &CallTile, self_focusable: bool) {
    person.devices += 1;
    person.muted &= tile.muted;
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

fn focusable(tile: &CallTile, self_focusable: bool) -> Option<TileKey> {
    (!tile.own || self_focusable).then(|| tile.key())
}

const fn telling(state: TileState) -> u8 {
    match state {
        TileState::Joining => 0,
        TileState::WaitingForKeys => 1,
        TileState::Live => 2,
        TileState::PoorConnection => 3,
        TileState::NotReceived(_) => 4,
        TileState::ConnectionLost => 5,
        TileState::Unreachable => 6,
        TileState::CantDecrypt(CantDecrypt::NoKey) => 7,
        TileState::CantDecrypt(CantDecrypt::NotCrossSigned) => 8,
    }
}
