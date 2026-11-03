pub(super) struct SessionRequests {
    issued: u64,
    first_of_session: u64,
}

impl Default for SessionRequests {
    fn default() -> Self {
        Self {
            issued: 0,
            first_of_session: 1,
        }
    }
}

impl SessionRequests {
    pub(super) fn issue(&mut self) -> u64 {
        self.issued = self.issued.wrapping_add(1);
        self.issued
    }

    pub(super) fn predates_session(&self, request: u64) -> bool {
        request < self.first_of_session
    }

    pub(super) fn end_session(&mut self) {
        self.first_of_session = self.issued.wrapping_add(1);
    }
}
