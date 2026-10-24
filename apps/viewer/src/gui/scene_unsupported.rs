use futures::future::BoxFuture;
use gpui::{AnyElement, App, BackgroundExecutor, WindowId};
use render::scene::Camera;
use tokio::sync::watch;
use uuid::Uuid;

use crate::{asset_coordinator::AssetHandle, session::EnvironmentInbox, world_reader::WorldReader};

pub(super) struct State;

impl State {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) fn window_closed(&mut self, _: WindowId) -> bool {
        false
    }

    pub(super) fn shutdown(&mut self, _: BackgroundExecutor) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

pub(crate) fn start_session(
    _: u64,
    _: &WorldReader,
    _: EnvironmentInbox,
    _: &AssetHandle,
    _: Uuid,
    _: watch::Sender<Option<Camera>>,
    _: &mut App,
) {
}

pub(crate) fn end_session(_: u64, _: &mut App) {}

pub(crate) fn login_content(session_ready: bool, login_body: AnyElement, chat_body: AnyElement) -> AnyElement {
    if session_ready { chat_body } else { login_body }
}
