/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "Text.h"
#include <utility>
#include "LayoutGeometry.h"
#include "NodeMutation.h"
#include "TextLayout.h"

namespace Core {
using detail::NodeMutation;

Text::Text(std::string value)
    : Node(NodeType::Text)
    , mValue(std::move(value))
    , mLayout(std::make_unique<Layout::TextLayout>(mValue)) {}

Text::~Text() = default;

void Text::setData(std::string value) { NodeMutation::setTextData(*this, std::move(value)); }

Style::ComputedStyle Text::styleForParent(const Style::ComputedStyle& parentStyle) {
    Style::ComputedStyle result;
    inheritStyle(result, parentStyle);
    result.setTextDecoration(parentStyle.textDecorationPropagation);
    result.textDecorationPropagation = parentStyle.textDecorationPropagation;
    result.setTextOverflow(parentStyle.textOverflow());
    result.setOverflowX(parentStyle.overflowX());
    result.setDisplay(Style::Display::Inline);
    result.displaySet = true;
    result.setMargin(Layout::RectEdges<Style::MarginEdge> {});
    result.setPadding(Layout::RectEdges<Style::PaddingEdge> {});
    return result;
}

Layout::Vec2 Text::intrinsicSize(const CSS::StyleSheet& styleSheet, const Style::ComputedStyle& style, const TextMeasurer& textMetrics,
    const Layout::IntrinsicSizeConstraints& constraints) const {
    const Element* owner = parentNode() ? parentNode()->asElement() : nullptr;
    return owner ? mLayout->measure(textMetrics, style, styleSheet, *owner, constraints.width) : Layout::Vec2 {};
}

void Text::setLayoutStyle(Style::ComputedStyle style) { mLayoutStyle = std::move(style); }

void Text::preparePaint(const TextMeasurer& metrics, const CSS::StyleSheet& styleSheet) const {
    const Element* owner = parentNode() ? parentNode()->asElement() : nullptr;
    if (owner)
        mLayout->preparePaint(metrics, mLayoutStyle, styleSheet, *owner, mRect.w);
}

void Text::paint(PaintContext& context, const Style::ComputedStyle& parentStyle, const CSS::StyleSheet* styleSheet,
    const Element& owner) const {
    const Layout::TextPaintStyle paintStyle {parentStyle.color().resolvedColor(), parentStyle.textDecorationPropagation,
        parentStyle.textAlign(), parentStyle.direction};
    mLayout->paintPrepared(context, Layout::insetRect(mRect, Layout::paddingPixels(mLayoutStyle)), mLayoutStyle, paintStyle, styleSheet,
        owner);
}
} // namespace Core
