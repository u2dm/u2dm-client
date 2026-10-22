use crate::commands::view::{CallTile, EndedShare, TileKey, TileKind};
use crate::domain::member_name::NamedUser;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Shown {
    Gallery,
    Focus(TileKey),
}

pub(super) struct Arranged {
    pub(super) stage: Option<CallTile>,
    pub(super) chosen: bool,
    pub(super) tiles: Vec<CallTile>,
    pub(super) offered_share: Option<NamedUser>,
}

pub(super) struct Stage {
    chosen: Option<Shown>,
    before_share: Option<Shown>,
    shares: Vec<TileKey>,
    staged: Option<(TileKey, NamedUser)>,
    self_view: bool,
    ended: Option<EndedShare>,
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
            ended: None,
            shares_ended: 0,
        }
    }
}

impl Stage {
    pub(super) const fn self_view(&self) -> bool {
        self.self_view
    }

    pub(super) const fn ended_share(&self) -> Option<&EndedShare> {
        self.ended.as_ref()
    }

    pub(super) fn settle(&mut self, tiles: &[CallTile]) {
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
        self.follow_staged(tiles);
    }

    pub(super) fn focus(&mut self, key: &str, tiles: &[CallTile]) -> bool {
        let Some(tile) = tiles
            .iter()
            .find(|tile| self.visible(tile) && tile.key().to_string() == key)
        else {
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
                    .find(|tile| self.visible(tile))
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

    pub(super) fn arrange(&self, tiles: Vec<CallTile>) -> Arranged {
        let shown = self.shown(&tiles);
        let offered_share = self.offered(&tiles).and_then(|key| {
            tiles
                .iter()
                .find(|tile| tile.key() == key)
                .map(|tile| tile.name.clone())
        });
        let mut stage = None;
        let mut rest = Vec::with_capacity(tiles.len());
        for tile in tiles {
            if !self.visible(&tile) {
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
            tiles: rest,
            offered_share,
        }
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
        let present: Vec<TileKey> = tiles
            .iter()
            .filter(|tile| tile.kind == TileKind::Screen)
            .map(CallTile::key)
            .collect();
        self.shares.retain(|key| present.contains(key));
        for key in present {
            if !self.shares.contains(&key) {
                self.shares.push(key);
            }
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
            self.ended = Some(EndedShare {
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
            .any(|tile| self.visible(tile) && tile.key() == *key)
    }

    const fn visible(&self, tile: &CallTile) -> bool {
        self.self_view || !tile.is_self_view()
    }
}
