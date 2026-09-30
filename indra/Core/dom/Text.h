/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <string>
#include "ComputedStyle.h"
#include "Element.h"

namespace Core {
namespace Layout {
class TextLayout;
}

class Text : public Node {
public:
    explicit Text(std::string value);
    ~Text() override;

    Text* asText() noexcept override { return this; }
    const Text* asText() const noexcept override { return this; }

    const std::string& data() const { return mValue; }
    void setData(std::string value);

    static Style::ComputedStyle styleForParent(const Style::ComputedStyle& parentStyle);
    Layout::Vec2 intrinsicSize(const CSS::StyleSheet& styleSheet, const Style::ComputedStyle& style, const TextMeasurer& textMetrics,
        const Layout::IntrinsicSizeConstraints& constraints = Layout::IntrinsicSizeConstraints()) const;
    void setRect(const Layout::Rect& rect) { mRect = rect; }
    const Layout::Rect& rect() const { return mRect; }
    void paint(PaintContext& context, const Style::ComputedStyle& style, const CSS::StyleSheet* styleSheet, const Element& owner) const;

private:
    friend class detail::NodeMutation;
    friend class Layout::Engine;
    friend class Layout::Pass;

    void setLayoutStyle(Style::ComputedStyle style);
    void preparePaint(const TextMeasurer& metrics, const CSS::StyleSheet& styleSheet) const;

    std::string mValue;
    std::unique_ptr<Layout::TextLayout> mLayout;
    Style::ComputedStyle mLayoutStyle;
    Layout::Rect mRect;
};
} // namespace Core
