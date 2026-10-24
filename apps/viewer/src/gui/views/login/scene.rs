use gpui::{Context, Window};

use crate::gui::window_geometry::observe;

use super::LoginScreen;

impl LoginScreen {
    pub(crate) fn attach_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._window_geometry_subscription = observe("login", window, cx);
    }

    pub(crate) fn chat_view(&self) -> gpui::Entity<crate::gui::views::chat::ChatScreen> {
        self.chat_screen.clone()
    }
}
