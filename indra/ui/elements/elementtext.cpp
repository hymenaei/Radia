/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "elements/elementtext.h"
#include <utility>
#include "text/host.h"

namespace radia::ui {
Text::Text(std::string value) : Node(NodeType::Text), mValue(std::move(value)), mLayout(std::make_unique<TextLayout>(mValue)) {}

Text::~Text() = default;

void Text::setData(std::string value) {
    if (mValue == value) return;
    mValue = std::move(value);
    mLayout->setText(mValue);
    if (Element* owner = parentElement()) owner->invalidateText();
}

Vec2 Text::intrinsicSize(const StyleSheet& styleSheet, const Style& style, const TextMetrics& textMetrics,
                         const IntrinsicSizeConstraints& constraints) const {
    const Element* owner = parentNode() ? parentNode()->asElement() : nullptr;
    return owner ? mLayout->measure(textMetrics, style, styleSheet, *owner, constraints.width) : Vec2{};
}

void Text::paint(PaintContext& context, const Style& style, const StyleSheet* styleSheet, const Element& owner) const {
    mLayout->paint(context, insetRect(mRect, style.padding), style, styleSheet, owner);
}
} // namespace radia::ui
