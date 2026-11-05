use super::Calls;
use super::joined::Joined;
use crate::app::room_directory::RoomDirectory;
use crate::commands::ui::CallCommand;

impl Calls {
    pub(in crate::app) fn handle(&mut self, command: CallCommand, directory: &RoomDirectory) {
        match command {
            CallCommand::DeclineCall(room_id) => self.decline(&room_id, directory),
            CallCommand::SilenceRing => self.silence(directory),
            CallCommand::OpenCallSheet { room_id, video } => {
                self.open_sheet(&room_id, video, directory);
            }
            CallCommand::CloseCallSheet => self.close_sheet(directory),
            CallCommand::SheetCamera(on) => self.sheet_camera(on, directory),
            CallCommand::TestCallDevice { kind, on } => self.test_device(kind, on, directory),
            CallCommand::StartCall {
                room_id,
                video,
                media,
                leaving,
            } => self.start_call(room_id, video, media, leaving, directory),
            CallCommand::JoinCall {
                room_id,
                media,
                leaving,
            } => self.join_call(room_id, media, leaving, directory),
            CallCommand::AnswerCall {
                room_id,
                camera,
                leaving,
            } => self.answer(room_id, camera, leaving, directory),
            CallCommand::LeaveCall => self.end_call(None, directory),
            CallCommand::RetryFailedCall => self.retry_failed(directory),
            CallCommand::DismissFailedCall => self.dismiss_failed(directory),
            CallCommand::AnnounceCallAgain => self.announce_again(directory),
            CallCommand::SetMicrophone(on) => {
                self.change_joined(directory, |joined| joined.set_microphone(on));
            }
            CallCommand::SetCamera(on) => {
                self.change_joined(directory, |joined| joined.set_camera(on));
            }
            CallCommand::ShareScreen(on) => {
                self.change_joined(directory, |joined| joined.set_screen_share(on));
            }
            CallCommand::ChangeScreenShare => {
                self.change_joined(directory, Joined::change_screen_share);
            }
            CallCommand::UseCallDevice { kind, key } => {
                self.use_device(kind, key.as_ref(), directory);
            }
            CallCommand::RefreshCallDevices => self.refresh_devices(),
            CallCommand::FocusCallTile(key) => {
                self.change_joined(directory, |joined| joined.focus_tile(&key));
            }
            CallCommand::UnpinCallTile => self.change_joined(directory, Joined::unpin_tile),
            CallCommand::RingDevices(open) => self.ring_devices(open, directory),
            CallCommand::ShowCallGallery => self.change_joined(directory, Joined::show_gallery),
            CallCommand::ViewCallShare => self.change_joined(directory, Joined::view_share),
            CallCommand::ShowSelfView(on) => {
                self.change_joined(directory, |joined| joined.show_self_view(on));
            }
            CallCommand::ShowSharePreview(on) => {
                self.change_joined(directory, |joined| joined.show_share_preview(on));
            }
            CallCommand::FollowSpeaker(on) => {
                self.change_joined(directory, |joined| joined.follow_speaker(on));
            }
            CallCommand::MuteForMe { user_id, on } => {
                self.change_joined(directory, |joined| joined.mute_for_me(&user_id, on));
            }
            CallCommand::WatchMicrophone(on) => {
                self.change_joined(directory, |joined| joined.watch_microphone(on));
            }
            CallCommand::WatchConnections(on) => {
                self.change_joined(directory, |joined| joined.watch_connections(on));
            }
            CallCommand::RaiseHand(on) => {
                self.change_joined_effects(directory, |joined| joined.raise_hand(on));
            }
            CallCommand::SendCallReaction(name) => {
                self.change_joined_effects(directory, |joined| joined.send_reaction(&name));
            }
        }
    }
}
