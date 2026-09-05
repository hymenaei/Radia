/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/fieldset.h"
#include <algorithm>
#include "html/elementfactory.h"
#include "html/elementnames.h"
#include "layout/engine.h"
#include "render/paintcontext.h"
#include "resource/elementdefinition.h"
#include "style/stylepass.h"
#include "surface/surface.h"
#include "text/metrics.h"

namespace radia::ui {
using detail::HTMLElementFactory;

HTMLLegendElement::HTMLLegendElement() : HTMLElement(kLegendTag.localName) {}

void HTMLLegendElement::constrainResolvedStyle(ComputedStyle& style) const {
    if (style.alignSelf == AlignSelf::Auto) style.alignSelf = AlignSelf::Start;
}

HTMLFieldsetElement::HTMLFieldsetElement() : HTMLElement(kFieldsetTag.localName) {}

void HTMLFieldsetElement::paint(PaintContext& context, const ComputedStyle& style, float) const {
    context.paintBox(rect(), style, topBorderGap());
}

bool HTMLFieldsetElement::hasLayoutGapBetween(const Element& first, const Element& second) const {
    return !isDirectLegend(first) && !isDirectLegend(second);
}

float HTMLFieldsetElement::layoutOverlapBetween(const Element& first, const Element&, const ComputedStyle& style) const {
    if (!isDirectLegend(first)) return 0.f;

    return std::max(0.f, style.padding.top + first.desiredSize().y * 0.5f - style.borderWidth.top * 0.5f);
}

void HTMLFieldsetElement::onArranged(const ComputedStyle& style) {
    Element* legend = directLegend();
    if (!legend || !isLegendVisible(*legend)) return;

    const float legendCenter = (legend->rect().top() + legend->rect().bottom()) * 0.5f;
    const float borderCenter = rect().top() - style.borderWidth.top * 0.5f;
    const float topDelta = borderCenter - legendCenter;
    if (topDelta != 0.f) translateChild(*legend, {0.f, topDelta});
}

bool HTMLFieldsetElement::isDirectLegend(const Element& element) {
    return element.elementName() == kLegendTag.localName;
}

Element* HTMLFieldsetElement::directLegend() {
    for (Element* child : children())
        if (isDirectLegend(*child)) return child;
    return nullptr;
}

const Element* HTMLFieldsetElement::directLegend() const {
    for (const Element* child : children())
        if (isDirectLegend(*child)) return child;
    return nullptr;
}

bool HTMLFieldsetElement::isLegendVisible(const Element& legend) const {
    if (const StyleSheet* sheet = styleSheet()) {
        StylePass styles(*sheet, textMetrics(), surface() ? surface()->layoutDirection() : LayoutDirection::LeftToRight);
        return legend.isVisible(styles.style(legend));
    }
    return legend.isVisible(ComputedStyle{});
}

std::optional<TopBorderGap> HTMLFieldsetElement::topBorderGap() const {
    const Element* legend = directLegend();
    if (!legend || !isLegendVisible(*legend)) return std::nullopt;

    const TopBorderGap gap{legend->rect().left(), legend->rect().right()};
    if (gap.empty()) return std::nullopt;
    return gap;
}

ResourceElementDefinition detail::ElementDefinitions::fieldset() {
    ResourceElementDefinition definition;
    definition.elementName = kFieldsetTag.localName;

    ScopedElementDefinition legend;
    legend.elementName = kLegendTag.localName;
    legend.acceptedTags = {
        HTMLTag::Abbr, HTMLTag::B,    HTMLTag::Br, HTMLTag::Button, HTMLTag::Cite,  HTMLTag::Code,   HTMLTag::Dfn,
        HTMLTag::Del,  HTMLTag::Em,   HTMLTag::I,  HTMLTag::Input,  HTMLTag::Ins,   HTMLTag::Kbd,    HTMLTag::Label,
        HTMLTag::Link, HTMLTag::Mark, HTMLTag::Q,  HTMLTag::S,      HTMLTag::Small, HTMLTag::Strong, HTMLTag::U,
    };
    legend.create = [](Element& fieldset, ElementBuildContext& context, const std::string& sourceName, std::size_t line,
                       std::size_t column) -> Element* {
        for (Element* child : fieldset.children()) {
            if (child->elementName() != kLegendTag.localName) continue;
            context.error("layout.fieldset.legend_duplicate", "A fieldset accepts only one direct legend.", sourceName, line, column);
            return child;
        }

        auto legend = HTMLElementFactory::Create(kLegendTag.localName);
        Element* resultElement = legend.get();
        fieldset.append(std::move(legend));
        return resultElement;
    };
    definition.contentBehavior.scopedElements.emplace(kLegendTag.localName, std::move(legend));
    return definition;
}

ResourceElementDefinition detail::ElementDefinitions::legend() {
    ResourceElementDefinition definition;
    definition.elementName = kLegendTag.localName;
    definition.scopedOnly = true;
    return definition;
}
} // namespace radia::ui
