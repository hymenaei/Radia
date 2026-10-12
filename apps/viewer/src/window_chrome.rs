use gpui::{IntoElement, WindowControlArea, div, prelude::*, px};
use gpui_component::ThemeColor;

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
        .child(titlebar_content)
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
