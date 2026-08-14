/**
 * @file label.h
 * @brief Defines Label targeting and activation for named controls.
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

#ifndef RD_WIDGETS_LABEL_H
#define RD_WIDGETS_LABEL_H

#include "text/host.h"
#include "widgets/widget.h"

namespace rdui {
struct WidgetContract;

namespace detail { WidgetContract labelContract(); }

class Label : public Widget {
    friend WidgetContract detail::labelContract();
    friend class detail::WidgetCompilerAccess;

public:
    static constexpr const char* sElement = "label";

    explicit Label(std::string text = {});

    Label& setText(std::string text);
    Label& setContent(TextSource content);
    Label& setContent(InlineContent content);
    bool setTextContent(TextSource content) override;
    const std::string& text() const { return mText.plainText(); }
    const InlineContent& content() const { return mText.content(); }

    Vec2 intrinsicSize(const StyleSheet& theme, const Style& style, const TextMetrics& textMetrics,
                       const IntrinsicSizeConstraints& constraints = IntrinsicSizeConstraints()) const override;
    void paint(PaintContext& context, const Style& style, float scale) const override;
    bool defaultPointerEvents() const override { return static_cast<bool>(mTarget); }

protected:
    Label(const char* elementName, std::string text);

private:
    Label& setTargetId(std::string id);
    void onActivate() override;
    void onLocaleChanged(const System& system) override;
    bool onKeybindingsChanged(const System& system) override;

    TextHost mText;
    std::string mTargetId;
    WidgetRef<Widget> mTarget;
};
} // namespace rdui
#endif // RD_WIDGETS_LABEL_H
