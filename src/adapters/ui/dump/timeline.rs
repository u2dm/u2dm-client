use serde::Serialize;
use slint::Model;

use crate::adapters::ui::backend::UiBackend;
use crate::adapters::ui::fields::{
    BodyLineFields, BodyPieceFields, MessageFields, PollAnswerFields, ReactionFields,
};

#[derive(Serialize)]
pub struct ReactionRowDump {
    pub key: String,
    pub label: String,
    pub art: String,
    pub count: i32,
    pub mine: bool,
    pub send: String,
    pub overflow: bool,
    pub reactors: String,
    pub hidden_reactors: i32,
}

#[derive(Serialize)]
pub struct BodyEmojiDump {
    pub line: usize,
    pub key: String,
    pub art: String,
}

#[derive(Serialize)]
pub struct PollAnswerRowDump {
    pub id: String,
    pub label: String,
    pub count: i32,
    pub share: f32,
    pub mine: bool,
    pub leading: bool,
    pub votable: bool,
    pub voters: String,
    pub hidden_voters: i32,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct TimelineRowDump {
    pub row: usize,
    pub unique_id: String,
    pub local_id: String,
    pub event_id: String,
    pub sender: String,
    pub sender_id: String,
    pub body: String,
    pub has_links: bool,
    pub mentions_you: bool,
    pub emote_only: bool,
    pub spoilers: String,
    pub preview_body: String,
    pub body_lines: Vec<usize>,
    pub body_emoji: Vec<BodyEmojiDump>,
    pub timestamp: String,
    pub sent_at: String,
    pub message_type: String,
    pub preview_kind: String,
    pub service_kind: String,
    pub service_target: String,
    pub service_reason: String,
    pub media_state: String,
    pub media_failure: String,
    pub send_state: String,
    pub send_progress: f32,
    pub delivery: String,
    pub shield: String,
    pub shield_reason: String,
    pub decryption_failure: String,
    pub readers: String,
    pub hidden_readers: i32,
    pub reader_count: i32,
    pub is_own: bool,
    pub edited: bool,
    pub text_editable: bool,
    pub edit_text: String,
    pub pinned: bool,
    pub first_unread: bool,
    pub needs_media: bool,
    pub has_avatar: bool,
    pub image_width: i32,
    pub image_height: i32,
    pub duration: String,
    pub filename: String,
    pub size: String,
    pub map_link: String,
    pub unsupported_kind: String,
    pub audio_kind: String,
    pub waveform: Vec<f32>,
    pub has_reply: bool,
    pub reply_event_id: String,
    pub reply_sender: String,
    pub reply_body: String,
    pub reactions: Vec<ReactionRowDump>,
    pub poll_phase: String,
    pub poll_editable: bool,
    pub poll_choices: i32,
    pub poll_voters: i32,
    pub poll_answers: Vec<PollAnswerRowDump>,
}

pub(super) fn timeline_row<B: UiBackend>(row: usize, entry: &B::Message) -> TimelineRowDump {
    TimelineRowDump {
        row,
        unique_id: entry.unique_id().to_owned(),
        local_id: entry.local_id().to_owned(),
        event_id: entry.event_id().to_owned(),
        sender: entry.sender().to_owned(),
        sender_id: entry.sender_id().to_owned(),
        body: entry.body().to_owned(),
        has_links: entry.has_links(),
        mentions_you: entry.mentions_you(),
        emote_only: entry.emote_only(),
        spoilers: entry.spoilers().to_owned(),
        preview_body: entry.preview_body().to_owned(),
        body_lines: entry
            .body_lines()
            .iter()
            .map(|line| line.pieces().row_count())
            .collect(),
        body_emoji: body_emoji::<B>(entry),
        timestamp: entry.timestamp().to_owned(),
        sent_at: entry.sent_at().to_owned(),
        message_type: entry.message_type().to_owned(),
        preview_kind: entry.preview_kind().to_owned(),
        service_kind: entry.service_kind().to_owned(),
        service_target: entry.service_target().to_owned(),
        service_reason: entry.service_reason().to_owned(),
        media_state: entry.media_state().to_owned(),
        media_failure: entry.media_failure().to_owned(),
        send_state: entry.send_state().to_owned(),
        send_progress: entry.send_progress(),
        delivery: entry.delivery().to_owned(),
        shield: entry.shield().to_owned(),
        shield_reason: entry.shield_reason().to_owned(),
        decryption_failure: entry.decryption_failure().to_owned(),
        readers: entry.readers().to_owned(),
        hidden_readers: entry.hidden_readers(),
        reader_count: entry.reader_count(),
        is_own: entry.is_own(),
        edited: entry.edited(),
        text_editable: entry.text_editable(),
        edit_text: entry.edit_text().to_owned(),
        pinned: entry.pinned(),
        first_unread: entry.first_unread(),
        needs_media: entry.needs_media(),
        has_avatar: entry.has_avatar(),
        image_width: entry.image_width(),
        image_height: entry.image_height(),
        duration: entry.duration().to_owned(),
        filename: entry.filename().to_owned(),
        size: entry.size().to_owned(),
        map_link: entry.map_link().to_owned(),
        unsupported_kind: entry.unsupported_kind().to_owned(),
        audio_kind: entry.audio_kind().to_owned(),
        waveform: entry.waveform(),
        has_reply: entry.has_reply(),
        reply_event_id: entry.reply_event_id().to_owned(),
        reply_sender: entry.reply_sender().to_owned(),
        reply_body: entry.reply_body().to_owned(),
        reactions: entry
            .reactions()
            .iter()
            .map(|reaction| reaction_row::<B>(&reaction))
            .collect(),
        poll_phase: entry.poll_phase().to_owned(),
        poll_editable: entry.poll_editable(),
        poll_choices: entry.poll_choices(),
        poll_voters: entry.poll_voters(),
        poll_answers: entry
            .poll_answers()
            .iter()
            .map(|answer| poll_answer_row::<B>(&answer))
            .collect(),
    }
}

fn body_emoji<B: UiBackend>(entry: &B::Message) -> Vec<BodyEmojiDump> {
    entry
        .body_lines()
        .iter()
        .enumerate()
        .flat_map(|(index, line)| {
            line.pieces()
                .iter()
                .filter(|piece| !piece.key().is_empty())
                .map(|piece| BodyEmojiDump {
                    line: index,
                    key: piece.key().to_owned(),
                    art: piece.art().to_owned(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn poll_answer_row<B: UiBackend>(entry: &B::PollAnswer) -> PollAnswerRowDump {
    PollAnswerRowDump {
        id: entry.id().to_owned(),
        label: entry.label().to_owned(),
        count: entry.count(),
        share: entry.share(),
        mine: entry.mine(),
        leading: entry.leading(),
        votable: entry.votable(),
        voters: entry.voters().to_owned(),
        hidden_voters: entry.hidden_voters(),
    }
}

fn reaction_row<B: UiBackend>(entry: &B::Reaction) -> ReactionRowDump {
    ReactionRowDump {
        key: entry.key().to_owned(),
        label: entry.label().to_owned(),
        art: entry.art().to_owned(),
        count: entry.count(),
        mine: entry.mine(),
        send: entry.send().to_owned(),
        overflow: entry.overflow(),
        reactors: entry.reactors().to_owned(),
        hidden_reactors: entry.hidden_reactors(),
    }
}
