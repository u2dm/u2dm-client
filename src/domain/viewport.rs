use super::models::ScrollMode;

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

        if at_bottom {
            self.mode = ScrollMode::FollowLive;
        } else {
            self.mode = ScrollMode::PreserveAnchor;
        }

        self.backwards_loading = false;
        self.forwards_loading = false;

        old_mode != self.mode
    }

    pub fn jump_to_latest(&mut self) {
        self.mode = ScrollMode::FollowLive;
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
}
