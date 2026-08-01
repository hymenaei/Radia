/**
 * @file rdtext.h
 * @brief
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#ifndef LL_RDUI_TEXT_H
#define LL_RDUI_TEXT_H

#include "rduitexthost.h"
#include "rduiwidget.h"

namespace rdui {
struct WidgetContract;

namespace detail { WidgetContract textContract(); }

class Text : public Widget {
    friend WidgetContract detail::textContract();

public:
    static constexpr const char* ELEMENT = "text";

    explicit Text(std::string text = {});
    Text& setText(std::string text);
    Text& setContent(TextSource content);
    Text& setContent(InlineContent content);
    const std::string& text() const { return mText.plainText(); }
    const InlineContent& content() const { return mText.content(); }

    Vec2 intrinsicSize(const StyleSheet& theme, const Style& style, const TextMetrics& text_metrics) const override;
    void paint(PaintContext& context, const Style& style, float scale) const override;

protected:
    struct ElementTag {};
    Text(const char* element, ElementTag);

private:
    void onLocaleChanged(const System& system) override;
    bool onKeybindingsChanged(const System& system) override;

    TextHost mText;
};
} // namespace rdui
#endif // LL_RDUI_TEXT_H
