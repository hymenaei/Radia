/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "dom/text.h"
#include <utility>
#include "dom/mutation.h"
#include "text/host.h"

namespace radia::ui {
using detail::NodeMutation;

Text::Text(std::string value) : Node(NodeType::Text), mValue(std::move(value)), mLayout(std::make_unique<TextLayout>(mValue)) {}

Text::~Text() = default;

void Text::setData(std::string value) {
    NodeMutation::setTextData(*this, std::move(value));
}

ComputedStyle Text::styleForParent(const ComputedStyle& parentStyle) {
    ComputedStyle result;
    inheritStyle(result, parentStyle);
    result.textOverflow = parentStyle.textOverflow;
    result.overflowX = parentStyle.overflowX;
    result.display = DisplayMode::Inline;
    result.displaySet = true;
    result.margin = {};
    result.padding = {};
    return result;
}

Vec2 Text::intrinsicSize(const StyleSheet& styleSheet, const ComputedStyle& style, const TextMetrics& textMetrics,
                         const IntrinsicSizeConstraints& constraints) const {
    const Element* owner = parentNode() ? parentNode()->asElement() : nullptr;
    return owner ? mLayout->measure(textMetrics, style, styleSheet, *owner, constraints.width) : Vec2{};
}

void Text::paint(PaintContext& context, const ComputedStyle& style, const StyleSheet* styleSheet, const Element& owner) const {
    mLayout->paint(context, insetRect(mRect, style.padding), style, styleSheet, owner);
}
} // namespace radia::ui
