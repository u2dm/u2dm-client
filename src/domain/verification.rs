#[derive(Debug, Clone)]
pub struct VerificationEmoji {
    pub symbol: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub enum VerificationEvent {
    Requested { sender: String, is_self: bool },
    Emojis(Vec<VerificationEmoji>),
    Confirming,
    Done,
    Cancelled(VerificationCancellation),
}

#[derive(Debug, Clone)]
pub enum VerificationCancellation {
    TimedOut,
    AcceptFailed,
    Declined,
    Mismatch,
    AcceptedElsewhere,
    Remote,
    Failed,
}
