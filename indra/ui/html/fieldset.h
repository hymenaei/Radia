/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include "html/element.h"
#include "render/paintcontext.h"

namespace radia::ui {
class HTMLLegendElement final : public HTMLElement {
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;

protected:
    void constrainResolvedStyle(ComputedStyle& style) const override;

private:
    HTMLLegendElement();
};

class HTMLFieldsetElement final : public HTMLElement {
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;

public:
    void paint(PaintContext& context, const ComputedStyle& style, float scale) const override;

protected:
    bool hasLayoutGapBetween(const Element& first, const Element& second) const override;
    float layoutOverlapBetween(const Element& first, const Element& second, const ComputedStyle& style) const override;
    void onArranged(const ComputedStyle& style) override;

private:
    static bool isDirectLegend(const Element& element);
    Element* directLegend();
    const Element* directLegend() const;
    bool isLegendVisible(const Element& legend) const;
    std::optional<TopBorderGap> topBorderGap() const;
    HTMLFieldsetElement();
};
} // namespace radia::ui
