/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <string>
#include "LayoutGeometry.h"

namespace Core {
struct ComputedStyle;

class TextMeasurer {
public:
    virtual ~TextMeasurer() = default;
    virtual Layout::Vec2 measureText(const std::string& text, const Style::ComputedStyle& style) const = 0;
    virtual float usedLetterSpacing(const Style::ComputedStyle& style) const;
    virtual std::uint64_t generation() const noexcept = 0;
};

class FixedTextMeasurer final : public TextMeasurer {
public:
    explicit FixedTextMeasurer(float regularWidthFactor = .58f, float boldWidthFactor = .62f)
        : mRegularWidthFactor(regularWidthFactor)
        , mBoldWidthFactor(boldWidthFactor) {}

    Layout::Vec2 measureText(const std::string& text, const Style::ComputedStyle& style) const override;
    float usedLetterSpacing(const Style::ComputedStyle& style) const override;
    std::uint64_t generation() const noexcept override { return 1; }

private:
    float mRegularWidthFactor;
    float mBoldWidthFactor;
};

const TextMeasurer& fixedTextMeasurer();
} // namespace Core
