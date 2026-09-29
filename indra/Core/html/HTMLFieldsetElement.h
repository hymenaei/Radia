/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include "HTMLElement.h"
#include "PaintContext.h"

namespace Core {
class HTMLLegendElement final : public HTMLElement {
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;

protected:
    void constrainResolvedStyle(Style::ComputedStyle& style) const override;

private:
    HTMLLegendElement();
};

class HTMLFieldsetElement final : public HTMLElement {
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;

public:
    void paint(PaintContext& context, const Style::ComputedStyle& style, float scale) const override;

protected:
    bool hasLayoutGapBetween(const Element& first, const Element& second) const override;
    float layoutOverlapBetween(const Element& first, const Element& second, const Style::ComputedStyle& style) const override;
    void onArranged(const Style::ComputedStyle& style) override;

private:
    static bool isDirectLegend(const Element& element);
    Element* directLegend();
    const Element* directLegend() const;
    bool isLegendVisible(const Element& legend) const;
    std::optional<TopBorderGap> topBorderGap() const;
    HTMLFieldsetElement();
};
} // namespace Core
