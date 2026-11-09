use std::fmt;
use std::time::Duration;

use super::drafts::{CreateRoomOp, EmoteDto, message_edit, poll_draft, room_draft};
use super::targets::{
    Selection, ban, in_room, in_space, kick, leave_space, moderation, person, room, room_notify,
    setting_change, settings_page, target,
};
use super::{ProbeCommand, Rejected};
use crate::adapters::demo::attachments;
use crate::adapters::ui::dump::Poke;
use crate::adapters::ui::{link_command, matrix_link, room_address_entry};
use crate::commands::ui::{
    AccountCommand, AuthCommand, CallCommand, DepartureCommand, KeysCommand, MessageDraft,
    ReplyDraft, SettingsCommand, UiCommand,
};
use crate::domain::call::devices::DeviceKey;
use crate::domain::call::media::MediaChoice;
use crate::domain::capabilities::AccountPage;
use crate::domain::emote::ComposerText;
use crate::domain::encryption::{KeyPassphrase, RecoveryKey};
use crate::domain::image_pack::PackId;
use crate::domain::matrix_link::{RoomAddress, RoomLink};
use crate::domain::media::AttachmentPick;
use crate::domain::message::EditTarget;
use crate::domain::room::RoomId;
use crate::domain::sync::{ReauthReason, SessionLoss};
use crate::domain::uia::AccountPassword;
use crate::domain::user_info::UserId;
use crate::ports::handoff::Handoff;
use crate::ports::notifier::NoticeAction;

pub enum Driven {
    Command(UiCommand),
    SessionLoss(SessionLoss),
    Poke(Poke),
    NotificationClick {
        room_id: RoomId,
        token: Option<String>,
        action: NoticeAction,
    },
    HandOff(Handoff),
}

impl fmt::Display for Driven {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(cmd) => write!(f, "{cmd}"),
            Self::SessionLoss(SessionLoss::Expired) => f.write_str("SessionExpired"),
            Self::SessionLoss(SessionLoss::Suspended(ReauthReason::SoftLogout)) => {
                f.write_str("SoftLogout")
            }
            Self::SessionLoss(SessionLoss::Suspended(ReauthReason::Refused)) => {
                f.write_str("SessionRefused")
            }
            Self::Poke(Poke::ToggleAudio) => f.write_str("ToggleAudio"),
            Self::Poke(Poke::SeekAudio(position)) => {
                write!(f, "SeekAudio({}ms)", position.as_millis())
            }
            Self::Poke(Poke::WindowFocus(focused)) => write!(f, "WindowFocus({focused})"),
            Self::Poke(Poke::WindowSize(width, height)) => {
                write!(f, "WindowSize({width}x{height})")
            }
            Self::Poke(Poke::UserActivity(active)) => write!(f, "UserActivity({active})"),
            Self::Poke(Poke::SwipeTravel(px)) => write!(f, "SwipeTravel({px}px)"),
            Self::Poke(Poke::TouchpadPinch(permille)) => write!(f, "TouchpadPinch({permille}‰)"),
            Self::HandOff(handoff) => write!(f, "HandOff(link: {})", handoff.link.is_some()),
            Self::Poke(Poke::ContextPress) => f.write_str("ContextPress"),
            Self::Poke(Poke::CloseCallWindow) => f.write_str("CloseCallWindow"),
            Self::NotificationClick { room_id, .. } => write!(f, "ClickNotification({room_id})"),
        }
    }
}

#[allow(clippy::too_many_lines)]
pub fn to_driven(command: ProbeCommand, selected: Selection<'_>) -> Result<Driven, Rejected> {
    Ok(Driven::Command(match command {
        ProbeCommand::RetryRestore => UiCommand::RestoreSession,
        ProbeCommand::CheckServer { homeserver } => UiCommand::CheckServer(homeserver),
        ProbeCommand::LoginOauth => UiCommand::LoginOAuth,
        ProbeCommand::CancelOauth => UiCommand::CancelOAuth,
        ProbeCommand::BackToHomeserver => UiCommand::BackToHomeserver,
        ProbeCommand::ReauthOauth => UiCommand::ReauthOAuth,
        ProbeCommand::SelectSpace { space_id } => UiCommand::SelectSpace(space_id.map(RoomId::new)),
        ProbeCommand::SelectDirect => UiCommand::SelectDirect,
        ProbeCommand::SelectSubspace { subspace_id } => {
            UiCommand::SelectSubspace(subspace_id.map(RoomId::new))
        }
        ProbeCommand::MoveSpace { from, to } => UiCommand::MoveSpace { from, to },
        ProbeCommand::SelectRoom { room_id } => UiCommand::SelectRoom(RoomId::new(room_id)),
        ProbeCommand::FilterRooms { query } => UiCommand::FilterRooms(query),
        ProbeCommand::FoldInvites => UiCommand::FoldInvites,
        ProbeCommand::UnfoldInvites => UiCommand::UnfoldInvites,
        ProbeCommand::OpenSpace { space_id } => UiCommand::OpenSpace(RoomId::new(space_id)),
        ProbeCommand::OpenSpaceIndex => UiCommand::OpenSpaceIndex,
        ProbeCommand::CloseSpaceIndex => UiCommand::CloseSpaceIndex,
        ProbeCommand::PageSpaceIndex => UiCommand::PageSpaceIndex,
        ProbeCommand::RetrySpaceIndex => UiCommand::RetrySpaceIndex,
        ProbeCommand::JoinSpaceChild { room_id } => UiCommand::JoinSpaceChild(RoomId::new(room_id)),
        ProbeCommand::OpenSpaceChild { room_id } => UiCommand::OpenSpaceChild(RoomId::new(room_id)),
        ProbeCommand::OpenInvite { room_id } => UiCommand::OpenInvite(RoomId::new(room_id)),
        ProbeCommand::CloseInvite => UiCommand::CloseInvite,
        ProbeCommand::AcceptInvite { room_id } => UiCommand::AcceptInvite(RoomId::new(room_id)),
        ProbeCommand::DeclineInvite { room_id } => UiCommand::DeclineInvite(RoomId::new(room_id)),
        ProbeCommand::OpenRemoval { room_id } => {
            DepartureCommand::OpenRemoval(RoomId::new(room_id)).into()
        }
        ProbeCommand::CloseRemoval => DepartureCommand::CloseRemoval.into(),
        ProbeCommand::ForgetRoom { room_id } => {
            DepartureCommand::ForgetRoom(RoomId::new(room_id)).into()
        }
        ProbeCommand::OpenLeaveSpace { space_id } => {
            return in_space(space_id, DepartureCommand::OpenLeaveSpace);
        }
        ProbeCommand::CloseLeaveSpace => DepartureCommand::CloseLeaveSpace.into(),
        ProbeCommand::LeaveSpace {
            space_id,
            with_rooms,
        } => return in_space(space_id, leave_space(with_rooms)),
        ProbeCommand::DeclineCall { room_id } => {
            CallCommand::DeclineCall(RoomId::new(room_id)).into()
        }
        ProbeCommand::SilenceRing => CallCommand::SilenceRing.into(),
        ProbeCommand::OpenCallSheet { room_id, video } => {
            return in_room(room_id, selected, |room_id| CallCommand::OpenCallSheet {
                room_id,
                video,
            });
        }
        ProbeCommand::CloseCallSheet => CallCommand::CloseCallSheet.into(),
        ProbeCommand::StartCall {
            room_id,
            video,
            microphone,
            camera,
            leaving,
        } => {
            return in_room(room_id, selected, |room_id| CallCommand::StartCall {
                room_id,
                video,
                media: MediaChoice { microphone, camera },
                leaving: leaving.map(RoomId::new),
            });
        }
        ProbeCommand::JoinCall {
            room_id,
            microphone,
            camera,
            leaving,
        } => {
            return in_room(room_id, selected, |room_id| CallCommand::JoinCall {
                room_id,
                media: MediaChoice { microphone, camera },
                leaving: leaving.map(RoomId::new),
            });
        }
        ProbeCommand::AnswerCall {
            room_id,
            camera,
            leaving,
        } => CallCommand::AnswerCall {
            room_id: RoomId::new(room_id),
            camera,
            leaving: leaving.map(RoomId::new),
        }
        .into(),
        ProbeCommand::LeaveCall => CallCommand::LeaveCall.into(),
        ProbeCommand::RetryFailedCall => CallCommand::RetryFailedCall.into(),
        ProbeCommand::DismissFailedCall => CallCommand::DismissFailedCall.into(),
        ProbeCommand::AnnounceCallAgain => CallCommand::AnnounceCallAgain.into(),
        ProbeCommand::SetMicrophone { on } => CallCommand::SetMicrophone(on).into(),
        ProbeCommand::SetCamera { on } => CallCommand::SetCamera(on).into(),
        ProbeCommand::ShareScreen { on } => CallCommand::ShareScreen(on).into(),
        ProbeCommand::ChangeScreenShare => CallCommand::ChangeScreenShare.into(),
        ProbeCommand::UseCallDevice { kind, key } => CallCommand::UseCallDevice {
            kind,
            key: key.filter(|key| !key.is_empty()).map(DeviceKey::new),
        }
        .into(),
        ProbeCommand::RefreshCallDevices => CallCommand::RefreshCallDevices.into(),
        ProbeCommand::FocusCallTile { key } => CallCommand::FocusCallTile(key).into(),
        ProbeCommand::UnpinCallTile => CallCommand::UnpinCallTile.into(),
        ProbeCommand::RingDevices { open } => CallCommand::RingDevices(open).into(),
        ProbeCommand::MuteForMe { user_id, on } => CallCommand::MuteForMe { user_id, on }.into(),
        ProbeCommand::RaiseHand { on } => CallCommand::RaiseHand(on).into(),
        ProbeCommand::SendCallReaction { name } => CallCommand::SendCallReaction(name).into(),
        ProbeCommand::WatchMicrophone { on } => CallCommand::WatchMicrophone(on).into(),
        ProbeCommand::WatchConnections { on } => CallCommand::WatchConnections(on).into(),
        ProbeCommand::ShowCallGallery => CallCommand::ShowCallGallery.into(),
        ProbeCommand::ViewCallShare => CallCommand::ViewCallShare.into(),
        ProbeCommand::ShowSelfView { on } => CallCommand::ShowSelfView(on).into(),
        ProbeCommand::ShowSharePreview { on } => CallCommand::ShowSharePreview(on).into(),
        ProbeCommand::FollowSpeaker { on } => CallCommand::FollowSpeaker(on).into(),
        ProbeCommand::JoinSuccessor { room_id } => {
            return in_room(room_id, selected, UiCommand::JoinSuccessor);
        }
        ProbeCommand::OpenRoomLink {
            room_id,
            alias,
            event_id,
            via,
        } => {
            let room = match (room_id, alias) {
                (Some(room_id), None) => RoomAddress::Id(RoomId::new(room_id)),
                (None, Some(alias)) => RoomAddress::Alias(alias),
                _ => {
                    return Err(Rejected(
                        "open_room_link takes exactly one of room_id and alias".to_owned(),
                    ));
                }
            };
            UiCommand::OpenRoomLink(RoomLink {
                room,
                event: event_id,
                via,
            })
        }
        ProbeCommand::OpenRoomFinder => UiCommand::OpenRoomFinder,
        ProbeCommand::FindRoom { address } => UiCommand::FindRoom(room_address_entry(address)),
        ProbeCommand::JoinPreviewedRoom { address } => UiCommand::JoinPreviewedRoom(address),
        ProbeCommand::RetryRoomPreview => UiCommand::RetryRoomPreview,
        ProbeCommand::CloseRoomPreview => UiCommand::CloseRoomPreview,
        ProbeCommand::OpenCreateRoom => UiCommand::OpenCreateRoom,
        ProbeCommand::OpenCreateSpace => UiCommand::OpenCreateSpace,
        ProbeCommand::CloseCreateRoom => UiCommand::CloseCreateRoom,
        ProbeCommand::CreateRoom {
            name,
            topic,
            kind,
            access,
            address,
            encrypted,
            in_space,
        } => room_draft(CreateRoomOp {
            name,
            topic,
            kind,
            access,
            address,
            encrypted,
            in_space,
        })?,
        ProbeCommand::OpenInvitePeople { room_id } => {
            return in_room(room_id, selected, UiCommand::OpenInvitePeople);
        }
        ProbeCommand::CloseInvitePeople => UiCommand::CloseInvitePeople,
        ProbeCommand::SearchPeople { query } => UiCommand::SearchPeople(query),
        ProbeCommand::RetryPeopleSearch => UiCommand::RetryPeopleSearch,
        ProbeCommand::PickInvitee { user_id } => return person(user_id, UiCommand::PickInvitee),
        ProbeCommand::UnpickInvitee { user_id } => {
            return person(user_id, UiCommand::UnpickInvitee);
        }
        ProbeCommand::SendInvites { reason } => UiCommand::SendInvites(reason),
        ProbeCommand::JoinPredecessor { room_id } => {
            return in_room(room_id, selected, UiCommand::JoinPredecessor);
        }
        ProbeCommand::OpenRoomInfo { room_id } => {
            return in_room(room_id, selected, UiCommand::OpenRoomInfo);
        }
        ProbeCommand::CloseRoomInfo => UiCommand::CloseRoomInfo,
        ProbeCommand::ShowRoomInfoPane => UiCommand::ShowRoomInfoPane,
        ProbeCommand::HideRoomInfoPane => UiCommand::HideRoomInfoPane,
        ProbeCommand::PageRoomMembers => UiCommand::PageRoomMembers,
        ProbeCommand::RetryRoomMembers => UiCommand::RetryRoomMembers,
        ProbeCommand::FilterRoomMembers { query } => UiCommand::FilterRoomMembers(query),
        ProbeCommand::SetRoomNotify { room_id, mode } => room_notify(room_id, &mode, selected)?,
        ProbeCommand::LeaveRoom { room_id } => {
            return in_room(room_id, selected, UiCommand::LeaveRoom);
        }
        ProbeCommand::OpenRoomMenu { room_id } => {
            return in_room(room_id, selected, UiCommand::OpenRoomMenu);
        }
        ProbeCommand::CloseRoomMenu => UiCommand::CloseRoomMenu,
        ProbeCommand::MarkRoomRead { room_id } => {
            return in_room(room_id, selected, UiCommand::MarkRoomRead);
        }
        ProbeCommand::CopyRoomLink { room_id } => {
            return in_room(room_id, selected, UiCommand::CopyRoomLink);
        }
        ProbeCommand::OpenRoomLog { room_id } => {
            return in_room(room_id, selected, UiCommand::OpenRoomLog);
        }
        ProbeCommand::CloseRoomLog => UiCommand::CloseRoomLog,
        ProbeCommand::OpenUserInfo { user_id } => return person(user_id, UiCommand::OpenUserInfo),
        ProbeCommand::CloseUserInfo => UiCommand::CloseUserInfo,
        ProbeCommand::RetryUserInfo => UiCommand::RetryUserInfo,
        ProbeCommand::MessageUser { user_id } => return person(user_id, UiCommand::MessageUser),
        ProbeCommand::VerifyUser { user_id } => return person(user_id, UiCommand::VerifyUser),
        ProbeCommand::IgnoreUser { user_id } => return person(user_id, UiCommand::IgnoreUser),
        ProbeCommand::UnignoreUser { user_id } => return person(user_id, UiCommand::UnignoreUser),
        ProbeCommand::KickUser { user_id, reason } => return moderation(user_id, reason, kick),
        ProbeCommand::BanUser { user_id, reason } => return moderation(user_id, reason, ban),
        ProbeCommand::UnbanUser { user_id } => return person(user_id, UiCommand::UnbanUser),
        ProbeCommand::AcceptIdentity { user_id } => {
            return person(user_id, UiCommand::AcceptIdentity);
        }
        ProbeCommand::WithdrawVerification { user_id } => {
            return person(user_id, UiCommand::WithdrawVerification);
        }
        ProbeCommand::AcknowledgeIdentityReset => UiCommand::AcknowledgeIdentityReset,
        ProbeCommand::OpenEncryption => UiCommand::OpenEncryption,
        ProbeCommand::CloseEncryption => UiCommand::CloseEncryption,
        ProbeCommand::ShowEncryptionOverview => UiCommand::ShowEncryptionOverview,
        ProbeCommand::EnterRecoveryKey => UiCommand::EnterRecoveryKey,
        ProbeCommand::UseRecoveryKey { key } => UiCommand::UseRecoveryKey(RecoveryKey::new(key)),
        ProbeCommand::VerifyWithOtherSession => UiCommand::VerifyWithOtherSession,
        ProbeCommand::SetUpRecovery => UiCommand::SetUpRecovery,
        ProbeCommand::ChangeRecoveryKey => UiCommand::ChangeRecoveryKey,
        ProbeCommand::RetryEncryptionSetup => UiCommand::RetryEncryptionSetup,
        ProbeCommand::StartIdentityReset => KeysCommand::StartIdentityReset.into(),
        ProbeCommand::ResetIdentity => KeysCommand::ResetIdentity.into(),
        ProbeCommand::StartKeyExport => KeysCommand::StartKeyExport.into(),
        ProbeCommand::ExportRoomKeys { passphrase } => {
            KeysCommand::ExportRoomKeys(KeyPassphrase::new(passphrase)).into()
        }
        ProbeCommand::StartKeyImport => KeysCommand::StartKeyImport.into(),
        ProbeCommand::ImportRoomKeys { passphrase } => {
            KeysCommand::ImportRoomKeys(KeyPassphrase::new(passphrase)).into()
        }
        ProbeCommand::OpenSettings => SettingsCommand::OpenSettings.into(),
        ProbeCommand::CloseSettings => SettingsCommand::CloseSettings.into(),
        ProbeCommand::ClearMediaCache => SettingsCommand::ClearDownloadedMedia.into(),
        ProbeCommand::ShowSettingsPage { page } => return settings_page(&page),
        ProbeCommand::SetSetting { name, value } => return setting_change(&name, &value),
        ProbeCommand::OpenAccount => AccountCommand::OpenAccount.into(),
        ProbeCommand::CloseAccount => AccountCommand::CloseAccount.into(),
        ProbeCommand::ShowAccountOverview => AccountCommand::ShowAccountOverview.into(),
        ProbeCommand::RenameSession { device_id, name } => {
            AccountCommand::RenameSession { device_id, name }.into()
        }
        ProbeCommand::SignOutSession { device_id } => {
            AccountCommand::SignOutSession(device_id).into()
        }
        ProbeCommand::OpenAccountPage => {
            AccountCommand::OpenAccountPage(AccountPage::Account).into()
        }
        ProbeCommand::OpenSessionsPage => {
            AccountCommand::OpenAccountPage(AccountPage::Sessions).into()
        }
        ProbeCommand::StartPasswordChange => AccountCommand::StartPasswordChange.into(),
        ProbeCommand::ChangePassword {
            password,
            sign_out_others,
        } => AccountCommand::ChangePassword {
            password: AccountPassword::new(password),
            sign_out_others,
        }
        .into(),
        ProbeCommand::StartDeactivation => AccountCommand::StartDeactivation.into(),
        ProbeCommand::DeactivateAccount { erase } => {
            AccountCommand::DeactivateAccount { erase }.into()
        }
        ProbeCommand::AnswerAuthPassword { password } => {
            AuthCommand::AnswerAuthPassword(AccountPassword::new(password)).into()
        }
        ProbeCommand::ContinueAuth => AuthCommand::ContinueAuth.into(),
        ProbeCommand::ReopenAuthPage => AuthCommand::ReopenAuthPage.into(),
        ProbeCommand::CancelAuth => AuthCommand::CancelAuth.into(),
        ProbeCommand::ConfirmLogout => UiCommand::ConfirmLogout,
        ProbeCommand::CancelLogout => UiCommand::CancelLogout,
        ProbeCommand::SendMessage {
            room_id,
            body,
            emotes,
            reply_to,
            reply_sender,
            reply_preview,
        } => UiCommand::SendMessage {
            room_id: room(room_id, selected)?,
            draft: MessageDraft {
                body,
                reply: reply_to.map(|event_id| ReplyDraft {
                    event_id,
                    sender: reply_sender,
                    preview: reply_preview,
                }),
                emotes: emotes.into_iter().map(EmoteDto::into_emote).collect(),
            },
        },
        ProbeCommand::EditMessage {
            room_id,
            event_id,
            local_id,
            caption,
            body,
            original,
            emotes,
        } => UiCommand::EditMessage {
            room_id: room(room_id, selected)?,
            edit: message_edit(
                EditTarget::of(event_id, local_id),
                caption,
                body,
                original,
                emotes.into_iter().map(EmoteDto::into_emote).collect(),
            )?,
        },
        ProbeCommand::DismissUnsent { submission } => UiCommand::DismissUnsent { submission },
        ProbeCommand::SendPoll {
            room_id,
            question,
            answers,
            multiple,
            hide_results,
        } => UiCommand::SendPoll {
            room_id: room(room_id, selected)?,
            draft: poll_draft(question, answers, multiple, hide_results)?,
        },
        ProbeCommand::EditPoll {
            event_id,
            question,
            answers,
            multiple,
            hide_results,
        } => UiCommand::EditPoll {
            event_id,
            draft: poll_draft(question, answers, multiple, hide_results)?,
        },
        ProbeCommand::SendSticker {
            room_id,
            pack,
            shortcode,
            reply_to,
        } => UiCommand::SendSticker {
            room_id: room(room_id, selected)?,
            pack: PackId::new(pack),
            shortcode,
            reply_to,
        },
        ProbeCommand::PickAttachment { room_id, document } => {
            if attachments::scenario().preset_pick.is_none() {
                return Err(Rejected(
                    "PickAttachment opens a native file chooser, which cannot be driven and \
                     blocks the Slint event loop; set U2DM_DEMO_ATTACHMENTS=pick=<path> first"
                        .to_owned(),
                ));
            }
            UiCommand::PickAttachment {
                room_id: room(room_id, selected)?,
                pick: if document {
                    AttachmentPick::Document
                } else {
                    AttachmentPick::Media
                },
            }
        }
        ProbeCommand::SendAttachment {
            room_id,
            caption,
            caption_emotes,
            as_document,
            reply_to,
        } => UiCommand::SendAttachment {
            room_id: room(room_id, selected)?,
            caption: ComposerText {
                text: caption,
                emotes: caption_emotes
                    .into_iter()
                    .map(EmoteDto::into_emote)
                    .collect(),
            },
            as_document,
            reply_to,
        },
        ProbeCommand::CancelAttachment => UiCommand::CancelAttachment,
        ProbeCommand::PaginateBackwards {
            room_id,
            generation,
        } => {
            let at = target(room_id, generation, selected)?;
            UiCommand::PaginateBackwards {
                room_id: at.room_id,
                generation: at.generation,
            }
        }
        ProbeCommand::PaginateForwards {
            room_id,
            generation,
        } => {
            let at = target(room_id, generation, selected)?;
            UiCommand::PaginateForwards {
                room_id: at.room_id,
                generation: at.generation,
            }
        }
        ProbeCommand::JumpToLatest {
            room_id,
            generation,
        } => {
            let at = target(room_id, generation, selected)?;
            UiCommand::JumpToLatest {
                room_id: at.room_id,
                generation: at.generation,
            }
        }
        ProbeCommand::JumpToEvent { event_id } => UiCommand::JumpToEvent { event_id },
        ProbeCommand::OpenPinned { event_id } => UiCommand::OpenPinned { event_id },
        ProbeCommand::PreviousPinned { event_id } => UiCommand::PreviousPinned { event_id },
        ProbeCommand::NextPinned { event_id } => UiCommand::NextPinned { event_id },
        ProbeCommand::CopyMessageLink { event_id } => UiCommand::CopyMessageLink { event_id },
        ProbeCommand::LoadMedia { event_id } => UiCommand::LoadMedia { event_id },
        ProbeCommand::OpenEventSource { event_id } => UiCommand::OpenEventSource { event_id },
        ProbeCommand::CloseEventSource => UiCommand::CloseEventSource,
        ProbeCommand::OpenReaders { event_id } => UiCommand::OpenReaders { event_id },
        ProbeCommand::CloseReaders => UiCommand::CloseReaders,
        ProbeCommand::PageReaders => UiCommand::PageReaders,
        ProbeCommand::SuggestMentions {
            room_id,
            query,
            recent,
        } => UiCommand::SuggestMentions {
            room_id: room(room_id, selected)?,
            query,
            recent: recent.into_iter().map(UserId::new).collect(),
        },
        ProbeCommand::EndMentions => UiCommand::EndMentions,
        ProbeCommand::PinMessage { event_id } => UiCommand::PinMessage { event_id },
        ProbeCommand::UnpinMessage { event_id } => UiCommand::UnpinMessage { event_id },
        ProbeCommand::DeleteMessage { event_id } => UiCommand::DeleteMessage { event_id },
        ProbeCommand::ToggleReaction { event_id, key } => {
            UiCommand::ToggleReaction { event_id, key }
        }
        ProbeCommand::ReactWithEmote {
            event_id,
            pack,
            shortcode,
        } => UiCommand::ReactWithEmote {
            event_id,
            pack: PackId::new(pack),
            shortcode,
        },
        ProbeCommand::SetPackGlobal { pack, global } => UiCommand::SetPackGlobal {
            pack: PackId::new(pack),
            global,
        },
        ProbeCommand::VotePoll {
            event_id,
            answer_id,
        } => UiCommand::VotePoll {
            event_id,
            answer_id,
        },
        ProbeCommand::EndPoll { event_id } => UiCommand::EndPoll { event_id },
        ProbeCommand::RetrySend { local_id } => UiCommand::RetrySend { local_id },
        ProbeCommand::DiscardSend { local_id } => UiCommand::DiscardSend { local_id },
        ProbeCommand::RetryTimeline => UiCommand::RetryTimeline,
        ProbeCommand::OpenVideo { event_id } => {
            if !cfg!(feature = "video") {
                return Err(Rejected(
                    "this build has no video feature, so OpenVideo hands the file to the system \
                     player instead of opening the in-app overlay"
                        .to_owned(),
                ));
            }
            UiCommand::OpenVideo { event_id }
        }
        ProbeCommand::CloseVideo => UiCommand::CloseVideo,
        ProbeCommand::PlayAudio { event_id } => {
            if !cfg!(feature = "video") {
                return Err(Rejected(
                    "this build has no video feature, so PlayAudio hands the file to the system \
                     player instead of playing it in the app"
                        .to_owned(),
                ));
            }
            UiCommand::PlayAudio { event_id }
        }
        ProbeCommand::CloseAudio => UiCommand::CloseAudio,
        ProbeCommand::SaveFile {
            event_id,
            filename,
            mimetype,
        } => return scripted_save(event_id, filename, mimetype).map(Driven::Command),
        ProbeCommand::ToggleAudio => return Ok(Driven::Poke(Poke::ToggleAudio)),
        ProbeCommand::SeekAudio { ms } => {
            return Ok(Driven::Poke(Poke::SeekAudio(Duration::from_millis(ms))));
        }
        ProbeCommand::WindowFocus { focused } => {
            return Ok(Driven::Poke(Poke::WindowFocus(focused)));
        }
        ProbeCommand::ResizeWindow { width, height } => {
            return Ok(Driven::Poke(Poke::WindowSize(width, height)));
        }
        ProbeCommand::UserActivity { active } => {
            return Ok(Driven::Poke(Poke::UserActivity(active)));
        }
        ProbeCommand::SwipeTravel { px } => return Ok(Driven::Poke(Poke::SwipeTravel(px))),
        ProbeCommand::TouchpadPinch { permille } => {
            return Ok(Driven::Poke(Poke::TouchpadPinch(permille)));
        }
        ProbeCommand::ContextPress => return Ok(Driven::Poke(Poke::ContextPress)),
        ProbeCommand::CloseCallWindow => return Ok(Driven::Poke(Poke::CloseCallWindow)),
        ProbeCommand::ClickNotification {
            room_id,
            token,
            action,
        } => {
            let action = match action.as_deref() {
                None | Some("open") => NoticeAction::Open,
                Some("decline") => NoticeAction::Decline,
                Some("answer") => NoticeAction::Answer,
                Some(other) => {
                    return Err(Rejected(format!("{other} is not a notification action")));
                }
            };
            return Ok(Driven::NotificationClick {
                room_id: RoomId::new(room_id),
                token,
                action,
            });
        }
        ProbeCommand::OpenLink { url } => {
            link_command(&url).ok_or_else(|| Rejected(format!("{url} is not a link U2DM opens")))?
        }
        ProbeCommand::OpenExternalLink { url } => UiCommand::OpenExternalLink(
            matrix_link(&url).ok_or_else(|| Rejected(format!("{url} is not a Matrix link")))?,
        ),
        ProbeCommand::HandOff { link, token } => {
            return Ok(Driven::HandOff(Handoff { link, token }));
        }
        ProbeCommand::AcceptVerification => UiCommand::AcceptVerification,
        ProbeCommand::RejectVerification => UiCommand::RejectVerification,
        ProbeCommand::ConfirmVerification => UiCommand::ConfirmVerification,
        ProbeCommand::DismissVerification => UiCommand::DismissVerification,
        ProbeCommand::SessionExpired => return Ok(Driven::SessionLoss(SessionLoss::Expired)),
        ProbeCommand::SoftLogout => {
            return Ok(Driven::SessionLoss(SessionLoss::Suspended(
                ReauthReason::SoftLogout,
            )));
        }
        ProbeCommand::SessionRefused => {
            return Ok(Driven::SessionLoss(SessionLoss::Suspended(
                ReauthReason::Refused,
            )));
        }
        ProbeCommand::DismissToast => UiCommand::DismissToast,
        ProbeCommand::Logout => UiCommand::Logout,
        ProbeCommand::Quit => UiCommand::Quit,
    }))
}

fn scripted_save(
    event_id: String,
    filename: String,
    mimetype: Option<String>,
) -> Result<UiCommand, Rejected> {
    if attachments::scenario().save_dir.is_none() {
        return Err(Rejected(
            "SaveFile opens a native save dialog, which cannot be driven; set \
             U2DM_DEMO_ATTACHMENTS=save-to=<dir> first"
                .to_owned(),
        ));
    }
    Ok(UiCommand::SaveFile {
        event_id,
        filename,
        mimetype,
    })
}
