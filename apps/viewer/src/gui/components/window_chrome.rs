#[cfg(target_os = "linux")]
mod platform {
    use gpui::{AnyElement, Decorations, IntoElement, ParentElement, Window, div, prelude::*, px};
    use gpui_component::{StyledExt, ThemeColor, TitleBar};

    pub(crate) fn appears_transparent() -> bool {
        false
    }

    pub(crate) fn titlebar(window: &Window, title: Option<&'static str>, theme: ThemeColor) -> Option<AnyElement> {
        let title = title.unwrap_or("Radia");
        let content = div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().text_sm().font_semibold().text_color(theme.foreground).child(title))
            .into_any_element();
        needs_client_titlebar(window.window_decorations())
            .then(|| TitleBar::new().child(content).child(div().into_any_element()).into_any_element())
    }

    fn needs_client_titlebar(decorations: Decorations) -> bool {
        matches!(decorations, Decorations::Client { .. })
    }

    #[cfg(test)]
    mod tests {
        use gpui::{Decorations, Tiling};

        use super::needs_client_titlebar;

        #[test]
        fn titlebar_tracks_client_decorations() {
            assert!(needs_client_titlebar(Decorations::Client { tiling: Tiling::default() }));
            assert!(!needs_client_titlebar(Decorations::Server));
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use gpui::{AnyElement, IntoElement, Window, WindowControlArea, div, prelude::*, px};
    use gpui_component::{StyledExt, ThemeColor};

    pub(crate) fn appears_transparent() -> bool {
        true
    }

    pub(crate) fn titlebar(window: &Window, title: Option<&'static str>, theme: ThemeColor) -> Option<AnyElement> {
        let title = title.map_or_else(
            || div().into_any_element(),
            |title| {
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().text_sm().font_semibold().text_color(theme.foreground).child(title))
                    .into_any_element()
            },
        );
        Some(windows_titlebar(window.is_maximized(), title, div().into_any_element(), theme).into_any_element())
    }

    fn windows_titlebar(maximized: bool, titlebar_content: AnyElement, titlebar_action: AnyElement, theme: ThemeColor) -> impl IntoElement {
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
}

#[cfg(target_os = "macos")]
mod platform {
    pub(crate) fn appears_transparent() -> bool {
        false
    }

    pub(crate) fn titlebar(_: &gpui::Window, _: Option<&'static str>, _: gpui_component::ThemeColor) -> Option<gpui::AnyElement> {
        None
    }
}

pub(crate) use platform::{appears_transparent, titlebar};
