/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <string>
#include "dom/element.h"

namespace radia::ui {
class TextLayout;

class Text : public Node {
public:
    explicit Text(std::string value);
    ~Text() override;

    Text* asText() noexcept override { return this; }
    const Text* asText() const noexcept override { return this; }

    const std::string& data() const { return mValue; }
    void setData(std::string value);

    Vec2 intrinsicSize(const StyleSheet& styleSheet, const Style& style, const TextMetrics& textMetrics,
                       const IntrinsicSizeConstraints& constraints = IntrinsicSizeConstraints()) const;
    void setRect(const Rect& rect) { mRect = rect; }
    const Rect& rect() const { return mRect; }
    void paint(PaintContext& context, const Style& style, const StyleSheet* styleSheet, const Element& owner) const;

private:
    friend class detail::NodeMutation;

    std::string mValue;
    std::unique_ptr<TextLayout> mLayout;
    Rect mRect;
};
} // namespace radia::ui
