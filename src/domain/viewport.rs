use super::models::{PaginationDirection, PaginationState, ScrollMode};

pub const PAGINATION_BATCH_SIZE: u16 = 50;

#[allow(clippy::struct_excessive_bools)]
pub struct ViewportController {
    mode: ScrollMode,
    backwards_loading: bool,
    forwards_loading: bool,
    backwards_ended: bool,
    forwards_ended: bool,
}

impl ViewportController {
    pub fn new() -> Self {
        Self {
            mode: ScrollMode::FollowLive,
            backwards_loading: false,
            forwards_loading: false,
            backwards_ended: false,
            forwards_ended: false,
        }
    }

    pub fn mode(&self) -> ScrollMode {
        self.mode
    }

    pub fn update_scroll_position(&mut self, _at_top: bool, at_bottom: bool) -> bool {
        let old_mode = self.mode;

        if at_bottom && (self.mode == ScrollMode::FollowLive || self.forwards_ended) {
            self.mode = ScrollMode::FollowLive;
        } else if !at_bottom {
            self.mode = ScrollMode::PreserveAnchor;
        }

        old_mode != self.mode
    }

    pub fn jump_to_latest(&mut self) {
        self.mode = ScrollMode::FollowLive;
        self.forwards_ended = true;
    }

    pub fn should_paginate_backwards(&self, at_top: bool) -> bool {
        at_top && !self.backwards_loading && !self.backwards_ended
    }

    pub fn should_paginate_forwards(&self, at_bottom: bool) -> bool {
        at_bottom
            && !self.forwards_loading
            && !self.forwards_ended
            && self.mode == ScrollMode::PreserveAnchor
    }

    pub fn set_backwards_loading(&mut self, loading: bool) {
        self.backwards_loading = loading;
    }

    pub fn set_forwards_loading(&mut self, loading: bool) {
        self.forwards_loading = loading;
    }

    pub fn complete_pagination(&mut self, direction: PaginationDirection, hit_end: bool) {
        match direction {
            PaginationDirection::Backwards => {
                self.backwards_loading = false;
                self.backwards_ended |= hit_end;
            }
            PaginationDirection::Forwards => {
                self.forwards_loading = false;
                self.forwards_ended |= hit_end;
                if hit_end {
                    self.mode = ScrollMode::FollowLive;
                } else {
                    self.mode = ScrollMode::PreserveAnchor;
                }
            }
        }
    }

    pub fn state(&self) -> PaginationState {
        PaginationState {
            backwards_ended: self.backwards_ended,
            forwards_ended: self.forwards_ended,
            backwards_loading: self.backwards_loading,
            forwards_loading: self.forwards_loading,
        }
    }
}
