/**
 * @file metrics.h
 * @brief
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

#include <string>
#include "types.h"

namespace rdui {
struct Style;

class TextMetrics {
public:
    virtual ~TextMetrics() = default;
    virtual Vec2 measureText(const std::string& text, const Style& style) const = 0;
    virtual float usedLetterSpacing(const Style& style) const;
};

class FixedTextMetrics final : public TextMetrics {
public:
    explicit FixedTextMetrics(float regular_width_factor = .58f, float bold_width_factor = .62f)
        : mRegularWidthFactor(regular_width_factor), mBoldWidthFactor(bold_width_factor) {}

    Vec2 measureText(const std::string& text, const Style& style) const override;
    float usedLetterSpacing(const Style& style) const override;

private:
    float mRegularWidthFactor;
    float mBoldWidthFactor;
};

const TextMetrics& fixedTextMetrics();
} // namespace rdui
#endif // RD_TEXT_METRICS_H
