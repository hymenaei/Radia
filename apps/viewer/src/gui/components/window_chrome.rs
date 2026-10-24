#[cfg(target_os = "linux")]
use gpui::{AnyElement, Decorations, IntoElement, ParentElement, Window};
#[cfg(target_os = "windows")]
use gpui::{IntoElement, WindowControlArea, div, prelude::*, px};
#[cfg(target_os = "windows")]
use gpui_component::ThemeColor;
#[cfg(target_os = "linux")]
use gpui_component::TitleBar;

#[cfg(target_os = "windows")]
pub(crate) fn windows_titlebar(
    maximized: bool,
    titlebar_content: gpui::AnyElement,
    titlebar_action: gpui::AnyElement,
    theme: ThemeColor,
) -> impl IntoElement {
    div()
        .w_full()
        .h(px(32.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .bg(theme.title_bar)
        .child(div().h_full().flex().items_center().px_3().child(titlebar_content))
        .child(div().flex_1().h_full().window_control_area(WindowControlArea::Drag))
        .child(div().h_full().px_2().flex().items_center().child(titlebar_action))
        .child(
            div()
                .flex()
                .h_full()
                .flex()
                .items_center()
                .font_family("Segoe Fluent Icons")
                .child(windows_caption_button("minimize", WindowControlArea::Min, "\u{e921}", false, theme))
                .child(windows_caption_button(
                    if maximized { "restore" } else { "maximize" },
                    WindowControlArea::Max,
                    if maximized { "\u{e923}" } else { "\u{e922}" },
                    false,
                    theme,
                ))
                .child(windows_caption_button("close", WindowControlArea::Close, "\u{e8bb}", true, theme)),
        )
}

#[cfg(target_os = "linux")]
pub(crate) fn linux_titlebar(window: &Window, titlebar_content: AnyElement, titlebar_action: AnyElement) -> Option<AnyElement> {
    needs_client_titlebar(window.window_decorations())
        .then(|| TitleBar::new().child(titlebar_content).child(titlebar_action).into_any_element())
}

#[cfg(target_os = "linux")]
fn needs_client_titlebar(decorations: Decorations) -> bool {
    matches!(decorations, Decorations::Client { .. })
}

#[cfg(target_os = "windows")]
fn windows_caption_button(
    id: &'static str,
    area: WindowControlArea,
    glyph: &'static str,
    is_close: bool,
    theme: ThemeColor,
) -> impl IntoElement {
    let (hover, active) =
        if is_close { (theme.danger_hover, theme.danger_active) } else { (theme.secondary_hover, theme.secondary_active) };

    div()
        .id(id)
        .w(px(46.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.))
        .text_color(theme.foreground)
        .hover(move |style| style.bg(hover))
        .active(move |style| style.bg(active))
        .occlude()
        .window_control_area(area)
        .child(glyph)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use gpui::{Decorations, Tiling};

    use super::needs_client_titlebar;

    #[test]
    fn titlebar_tracks_client_decorations() {
        assert!(needs_client_titlebar(Decorations::Client { tiling: Tiling::default() }));
        assert!(!needs_client_titlebar(Decorations::Server));
    }
}
