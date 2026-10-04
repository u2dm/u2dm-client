#[derive(Default)]
pub(super) struct LinkRequests {
    issued: i32,
    awaited: Option<i32>,
}

impl LinkRequests {
    pub(super) fn issue(&mut self) -> i32 {
        self.issued = self.issued.wrapping_add(1);
        self.awaited = Some(self.issued);
        self.issued
    }

    pub(super) fn settle(&mut self, request: i32) -> bool {
        if self.awaited != Some(request) {
            return false;
        }
        self.awaited = None;
        true
    }

    pub(super) fn forget(&mut self) {
        self.awaited = None;
    }
}
