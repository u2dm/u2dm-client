use super::timeline::{PaginationDirection, PaginationState, ScrollMode, TimelineFocus};

pub const PAGINATION_BATCH_SIZE: u16 = 50;

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct ViewportController {
    mode: ScrollMode,
    backwards_loading: bool,
    forwards_loading: bool,
    backwards_ended: bool,
    forwards_ended: bool,
}

impl ViewportController {
    pub fn new(focus: &TimelineFocus) -> Self {
        let mode = if focus.is_live() {
            ScrollMode::FollowLive
        } else {
            ScrollMode::PreserveAnchor
        };
        Self {
            mode,
            ..Self::default()
        }
    }

    pub fn update_scroll_position(&mut self, at_bottom: bool) {
        if at_bottom && (self.mode == ScrollMode::FollowLive || self.forwards_ended) {
            self.mode = ScrollMode::FollowLive;
        } else if !at_bottom {
            self.mode = ScrollMode::PreserveAnchor;
        }
    }

    pub fn jump_to_latest(&mut self) {
        self.mode = ScrollMode::FollowLive;
        self.forwards_ended = true;
    }

    pub fn should_paginate_backwards(&self) -> bool {
        !self.backwards_loading && !self.backwards_ended
    }

    pub fn should_paginate_forwards(&self) -> bool {
        !self.forwards_loading && !self.forwards_ended && self.mode == ScrollMode::PreserveAnchor
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

    pub fn fail_pagination(&mut self, direction: PaginationDirection) {
        match direction {
            PaginationDirection::Backwards => self.backwards_loading = false,
            PaginationDirection::Forwards => self.forwards_loading = false,
        }
    }

    pub fn state(&self) -> PaginationState {
        PaginationState {
            backwards_loading: self.backwards_loading,
            forwards_loading: self.forwards_loading,
        }
    }
}
