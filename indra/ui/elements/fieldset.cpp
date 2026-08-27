/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include "elements/elementdefinition.h"
#include "layout/engine.h"
#include "render/paintcontext.h"

namespace radia::ui {
namespace {
class LegendElement final : public Element {
public:
    LegendElement() : Element("legend") {}

protected:
    void constrainResolvedStyle(Style& style) const override {
        if (style.alignSelf == AlignSelf::Auto) style.alignSelf = AlignSelf::Start;
    }
};

class FieldsetElement final : public Element {
public:
    FieldsetElement() : Element("fieldset") {}

    void paint(PaintContext& context, const Style& style, float) const override { context.paintBox(rect(), style, topBorderGap()); }

protected:
    bool hasLayoutGapBetween(const Element& first, const Element& second) const override { return !isDirectLegend(first) && !isDirectLegend(second); }

    float layoutOverlapBetween(const Element& first, const Element&, const Style& style) const override {
        if (!isDirectLegend(first)) return 0.f;

        return std::max(0.f, style.padding.top + first.desiredSize().y * 0.5f - style.borderWidth.top * 0.5f);
    }

    void onArranged(const Style& style) override {
        Element* legend = directLegend();
        if (!legend || !isLegendVisible(*legend)) return;

        const float legendCenter = (legend->rect().top() + legend->rect().bottom()) * 0.5f;
        const float borderCenter = rect().top() - style.borderWidth.top * 0.5f;
        const float topDelta = borderCenter - legendCenter;
        if (topDelta != 0.f) translateChild(*legend, {0.f, topDelta});
    }

private:
    static bool isDirectLegend(const Element& element) { return element.elementName() == "legend" && element.part().empty(); }

    Element* directLegend() {
        for (Element* child : children())
            if (isDirectLegend(*child)) return child;
        return nullptr;
    }

    const Element* directLegend() const {
        for (const Element* child : children())
            if (isDirectLegend(*child)) return child;
        return nullptr;
    }

    bool isLegendVisible(const Element& legend) const {
        if (const StyleSheet* sheet = styleSheet()) return legend.isVisible(resolveElementStyle(*sheet, legend));
        return legend.isVisible(Style{});
    }

    std::optional<TopBorderGap> topBorderGap() const {
        const Element* legend = directLegend();
        if (!legend || !isLegendVisible(*legend)) return std::nullopt;

        const TopBorderGap gap{legend->rect().left(), legend->rect().right()};
        if (gap.empty()) return std::nullopt;
        return gap;
    }
};
} // namespace

ElementDefinition detail::ElementDefinitionFactory::fieldset() {
    ElementDefinition definition;
    definition.elementName = "fieldset";
    definition.create = [] { return std::make_unique<FieldsetElement>(); };

    ScopedElementDefinition legend;
    legend.elementName = "legend";
    legend.acceptedElements = localizedInlineTags();
    legend.create = [](Element& fieldset, LayoutBuildResult& result, const std::string& source, std::size_t line, std::size_t column) -> Element* {
        for (Element* child : fieldset.children()) {
            if (child->elementName() != "legend" || !child->part().empty()) continue;
            result.error("layout.fieldset.legend_duplicate", "A fieldset accepts only one direct legend.", source, line, column);
            return child;
        }

        auto legend = std::make_unique<LegendElement>();
        Element* resultElement = legend.get();
        fieldset.append(std::move(legend));
        return resultElement;
    };
    definition.contentBehavior.scopedElements.emplace("legend", std::move(legend));
    return definition;
}

ElementDefinition detail::ElementDefinitionFactory::legend() {
    ElementDefinition definition;
    definition.elementName = "legend";
    definition.create = [] { return std::make_unique<LegendElement>(); };
    definition.scopedOnly = true;
    return definition;
}
} // namespace radia::ui
