/**
 * @file metrics.h
 * @brief Implements fixed and adapter-backed text measurement behavior.
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

#ifndef RD_TEXT_METRICS_H
#define RD_TEXT_METRICS_H

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
#endif // RD_TEXT_METRICS_H
