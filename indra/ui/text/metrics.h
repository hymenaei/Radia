/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <string>
#include "types.h"

namespace radia::ui {
struct Style;

class TextMetrics {
public:
    virtual ~TextMetrics() = default;
    virtual Vec2 measureText(const std::string& text, const Style& style) const = 0;
    virtual float usedLetterSpacing(const Style& style) const;
    virtual std::uint64_t generation() const { return 0; }
};

class FixedTextMetrics final : public TextMetrics {
public:
    explicit FixedTextMetrics(float regularWidthFactor = .58f, float boldWidthFactor = .62f)
        : mRegularWidthFactor(regularWidthFactor), mBoldWidthFactor(boldWidthFactor) {}

    Vec2 measureText(const std::string& text, const Style& style) const override;
    float usedLetterSpacing(const Style& style) const override;

private:
    float mRegularWidthFactor;
    float mBoldWidthFactor;
};

const TextMetrics& fixedTextMetrics();
} // namespace radia::ui
