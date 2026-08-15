/**
 * @file text.cpp
 * @brief Defines the static Text Host Widget.
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

#include "linden_common.h"
#include "widgets/text.h"
#include "render/paintcontext.h"
#include "system.h"
#include "widgets/widgetcontractbuilder.h"

namespace radia::ui {
Text::Text(std::string text) : Text(sElement, ElementTag{}) {
    setText(std::move(text));
}

Text::Text(const char* elementName, ElementTag) : Widget(elementName) {}

Text& Text::setText(std::string text) {
    return setContent(InlineContent::text(std::move(text)));
}

Text& Text::setContent(TextSource content) {
    mText.setContent(std::move(content));
    if (const System* system = attachedSystem()) onLocaleChanged(*system);
    else invalidateText();
    return *this;
}

Text& Text::setContent(InlineContent content) {
    return setContent(TextSource::literal(std::move(content)));
}

bool Text::setTextContent(TextSource content) {
    setContent(std::move(content));
    return true;
}

void Text::onLocaleChanged(const System& system) {
    mText.resolveLocalized([&system](const LocalizationRequest& request) { return system.resolveContent(request); });
    mText.resolveKeybindings([&system](const std::string& key) { return system.resolveKeybinding(key); });
    invalidateText();
}

bool Text::onKeybindingsChanged(const System& system) {
    const bool changed = mText.resolveKeybindings([&system](const std::string& key) { return system.resolveKeybinding(key); });
    if (changed) invalidateText();
    return changed;
}

Vec2 Text::intrinsicSize(const StyleSheet& styleSheet, const Style& style, const TextMetrics& textMetrics,
                         const IntrinsicSizeConstraints& constraints) const {
    return mText.measure(textMetrics, style, styleSheet, *this, constraints.width);
}

void Text::paint(PaintContext& context, const Style& style, float) const {
    context.paintBox(rect(), style);
    mText.paint(context, insetRect(rect(), style.padding), style, attachedStyleSheet(), *this);
}

WidgetContract detail::textContract() {
    return defineWidget<Text>(Text::sElement)
        .inlineContent({InlineContentKind::B, InlineContentKind::I, InlineContentKind::S, InlineContentKind::Kbd, InlineContentKind::Br},
                       [](TextSource content, Text& text) { text.setContent(std::move(content)); })
        .build();
}
} // namespace radia::ui
