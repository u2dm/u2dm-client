use std::time::Duration;

use crate::commands::view::{CallTile, ShareNews, TileKey, TileKind};
use crate::domain::member_name::NamedUser;

const SPEAKER_SETTLE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Shown {
    Gallery,
    Focus(TileKey),
}

pub(super) struct Arranged {
    pub(super) stage: Option<CallTile>,
    pub(super) chosen: bool,
    pub(super) view_chosen: bool,
    pub(super) tiles: Vec<CallTile>,
    pub(super) compact_self: Option<CallTile>,
    pub(super) shares: Vec<NamedUser>,
    pub(super) offered_share: Option<NamedUser>,
}

#[derive(Default)]
struct Following {
    on: bool,
    speaker: Option<TileKey>,
    candidate: Option<(TileKey, Duration)>,
}

pub(super) struct Stage {
    chosen: Option<Shown>,
    before_share: Option<Shown>,
    shares: Vec<TileKey>,
    staged: Option<(TileKey, NamedUser)>,
    self_view: bool,
    share_preview: bool,
    following: Following,
    started: Option<ShareNews>,
    shares_started: u64,
    ended: Option<ShareNews>,
    shares_ended: u64,
}

impl Default for Stage {
    fn default() -> Self {
        Self {
            chosen: None,
            before_share: None,
            shares: Vec::new(),
            staged: None,
            self_view: true,
            share_preview: true,
            following: Following::default(),
            started: None,
            shares_started: 0,
            ended: None,
            shares_ended: 0,
        }
    }
}

impl Stage {
    pub(super) const fn self_view(&self) -> bool {
        self.self_view
    }

    pub(super) const fn share_preview(&self) -> bool {
        self.share_preview
    }

    pub(super) const fn follows_speaker(&self) -> bool {
        self.following.on
    }

    pub(super) fn self_focusable(&self, tiles: &[CallTile]) -> bool {
        self.self_view && !self.compacts_self(tiles)
    }

    pub(super) const fn started_share(&self) -> Option<&ShareNews> {
        self.started.as_ref()
    }

    pub(super) const fn ended_share(&self) -> Option<&ShareNews> {
        self.ended.as_ref()
    }

    pub(super) fn settle(
        &mut self,
        tiles: &[CallTile],
        loudest: Option<&TileKey>,
        now: Duration,
    ) -> Option<Duration> {
        self.follow_shares(tiles);
        let gone = match &self.chosen {
            Some(Shown::Focus(key)) if !self.shows(key, tiles) => Some(key.clone()),
            _ => None,
        };
        if let Some(key) = gone {
            tracing::debug!(tile = %key, "the focused call tile is gone");
            self.chosen = match key.kind {
                TileKind::Screen => self
                    .before_share
                    .take()
                    .filter(|before| self.applies(before, tiles)),
                TileKind::Person => None,
            };
        }
        let wake = self.track_speaker(tiles, loudest, now);
        self.follow_staged(tiles);
        wake
    }

    pub(super) fn focus(&mut self, key: &str, tiles: &[CallTile]) -> bool {
        let Some(tile) = tiles.iter().find(|tile| {
            self.seen(tile, tiles) && !tile.is_own_share() && tile.key().to_string() == key
        }) else {
            return false;
        };
        self.pick(tile.key(), tiles);
        true
    }

    pub(super) fn focus_any(&mut self, tiles: &[CallTile]) {
        if let Shown::Focus(key) = self.shown(tiles) {
            self.chosen = Some(Shown::Focus(key));
            return;
        }
        let others = || {
            tiles
                .iter()
                .filter(|tile| tile.kind == TileKind::Person && !tile.own)
        };
        let pick = self
            .shares
            .iter()
            .rev()
            .find(|key| self.shows(key, tiles))
            .cloned()
            .or_else(|| {
                others()
                    .find(|tile| tile.video.is_some())
                    .map(CallTile::key)
            })
            .or_else(|| others().next().map(CallTile::key))
            .or_else(|| {
                tiles
                    .iter()
                    .find(|tile| self.seen(tile, tiles) && !tile.is_own_share())
                    .map(CallTile::key)
            });
        if let Some(key) = pick {
            self.pick(key, tiles);
        }
    }

    pub(super) fn gallery(&mut self) {
        self.chosen = Some(Shown::Gallery);
        self.before_share = None;
    }

    pub(super) fn view_share(&mut self, tiles: &[CallTile]) {
        if let Some(key) = self.offered(tiles) {
            self.pick(key, tiles);
        }
    }

    pub(super) const fn show_self(&mut self, on: bool) {
        self.self_view = on;
    }

    pub(super) const fn show_share_preview(&mut self, on: bool) {
        self.share_preview = on;
    }

    pub(super) fn follow_speaker(&mut self, on: bool) {
        if on {
            self.chosen = None;
            self.before_share = None;
        }
        if self.following.on != on {
            self.following = Following {
                on,
                ..Following::default()
            };
        }
    }

    pub(super) fn arrange(&self, tiles: Vec<CallTile>) -> Arranged {
        let shown = self.shown(&tiles);
        let name_of = |key: &TileKey| {
            tiles
                .iter()
                .find(|tile| tile.key() == *key)
                .map(|tile| tile.name.clone())
        };
        let shares = self.shares.iter().filter_map(name_of).collect();
        let offered_share = self.offered(&tiles).as_ref().and_then(name_of);
        let view_chosen = self
            .chosen
            .as_ref()
            .is_some_and(|chosen| self.applies(chosen, &tiles));
        let compact = self.compacts_self(&tiles);
        let mut stage = None;
        let mut compact_self = None;
        let mut rest = Vec::with_capacity(tiles.len());
        for tile in tiles {
            if !self.visible(&tile) {
                continue;
            }
            if compact && tile.is_self_view() {
                compact_self = Some(tile);
                continue;
            }
            match &shown {
                Shown::Focus(key) if stage.is_none() && tile.key() == *key => stage = Some(tile),
                _ => rest.push(tile),
            }
        }
        let chosen = matches!(
            (&self.chosen, &stage),
            (Some(Shown::Focus(key)), Some(staged)) if staged.key() == *key
        );
        Arranged {
            stage,
            chosen,
            view_chosen,
            tiles: rest,
            compact_self,
            shares,
            offered_share,
        }
    }

    fn track_speaker(
        &mut self,
        tiles: &[CallTile],
        loudest: Option<&TileKey>,
        now: Duration,
    ) -> Option<Duration> {
        if !self.following.on {
            return None;
        }
        if self
            .following
            .speaker
            .as_ref()
            .is_some_and(|key| !self.shows(key, tiles))
        {
            self.following.speaker = None;
        }
        let Some(loudest) = loudest.filter(|key| self.shows(key, tiles)) else {
            self.following.candidate = None;
            return None;
        };
        if self.following.speaker.as_ref() == Some(loudest) {
            self.following.candidate = None;
            return None;
        }
        let since = match &self.following.candidate {
            Some((key, since)) if key == loudest => *since,
            _ => now,
        };
        let settled = since.saturating_add(SPEAKER_SETTLE);
        if now < settled {
            self.following.candidate = Some((loudest.clone(), since));
            return Some(settled);
        }
        tracing::debug!(tile = %loudest, "following the speaker");
        self.following.speaker = Some(loudest.clone());
        self.following.candidate = None;
        None
    }

    fn pick(&mut self, key: TileKey, tiles: &[CallTile]) {
        tracing::debug!(tile = %key, "focusing a call tile");
        match key.kind {
            TileKind::Screen => {
                if !matches!(self.shown(tiles), Shown::Focus(ref shown) if shown.kind == TileKind::Screen)
                {
                    self.before_share = self.chosen.take();
                }
            }
            TileKind::Person => self.before_share = None,
        }
        self.chosen = Some(Shown::Focus(key));
    }

    fn follow_shares(&mut self, tiles: &[CallTile]) {
        let present: Vec<(TileKey, &NamedUser)> = tiles
            .iter()
            .filter(|tile| tile.is_remote_share())
            .map(|tile| (tile.key(), &tile.name))
            .collect();
        self.shares
            .retain(|key| present.iter().any(|(shown, _)| shown == key));
        for (key, who) in present {
            if self.shares.contains(&key) {
                continue;
            }
            self.shares_started = self.shares_started.saturating_add(1);
            tracing::debug!(tile = %key, "a screen share started");
            self.started = Some(ShareNews {
                serial: self.shares_started,
                who: who.clone(),
            });
            self.shares.push(key);
        }
    }

    fn follow_staged(&mut self, tiles: &[CallTile]) {
        let staged = match self.shown(tiles) {
            Shown::Focus(key) => tiles
                .iter()
                .find(|tile| tile.key() == key)
                .map(|tile| (key, tile.name.clone())),
            Shown::Gallery => None,
        };
        if let Some((last, who)) = self.staged.take()
            && last.kind == TileKind::Screen
            && !tiles.iter().any(|tile| tile.key() == last)
        {
            self.shares_ended = self.shares_ended.saturating_add(1);
            tracing::debug!(tile = %last, "the screen share on the stage ended");
            self.ended = Some(ShareNews {
                serial: self.shares_ended,
                who,
            });
        }
        self.staged = staged;
    }

    fn shown(&self, tiles: &[CallTile]) -> Shown {
        self.chosen
            .clone()
            .filter(|chosen| self.applies(chosen, tiles))
            .unwrap_or_else(|| self.automatic(tiles))
    }

    fn automatic(&self, tiles: &[CallTile]) -> Shown {
        if let Some(share) = self.shares.iter().find(|key| self.shows(key, tiles)) {
            return Shown::Focus(share.clone());
        }
        if let Some(speaker) = self
            .following
            .speaker
            .as_ref()
            .filter(|key| self.following.on && self.shows(key, tiles))
        {
            return Shown::Focus(speaker.clone());
        }
        let mut others = tiles
            .iter()
            .filter(|tile| tile.kind == TileKind::Person && !tile.own);
        match (others.next(), others.next()) {
            (Some(other), None) => Shown::Focus(other.key()),
            _ => Shown::Gallery,
        }
    }

    fn offered(&self, tiles: &[CallTile]) -> Option<TileKey> {
        let staged = match self.shown(tiles) {
            Shown::Focus(key) => Some(key),
            Shown::Gallery => None,
        };
        self.shares
            .iter()
            .rev()
            .find(|key| staged.as_ref() != Some(*key) && self.shows(key, tiles))
            .cloned()
    }

    fn applies(&self, shown: &Shown, tiles: &[CallTile]) -> bool {
        match shown {
            Shown::Gallery => true,
            Shown::Focus(key) => self.shows(key, tiles),
        }
    }

    fn shows(&self, key: &TileKey, tiles: &[CallTile]) -> bool {
        tiles
            .iter()
            .any(|tile| self.seen(tile, tiles) && tile.key() == *key)
    }

    fn seen(&self, tile: &CallTile, tiles: &[CallTile]) -> bool {
        self.visible(tile) && !(tile.is_self_view() && self.compacts_self(tiles))
    }

    fn compacts_self(&self, tiles: &[CallTile]) -> bool {
        self.self_view
            && tiles
                .iter()
                .any(|tile| tile.is_self_view() && tile.video.is_none())
            && tiles
                .iter()
                .any(|tile| !tile.is_self_view() && self.visible(tile))
    }

    const fn visible(&self, tile: &CallTile) -> bool {
        (self.self_view || !tile.is_self_view()) && (self.share_preview || !tile.is_own_share())
    }
}
