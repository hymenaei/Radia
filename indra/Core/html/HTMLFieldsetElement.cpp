/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "HTMLFieldsetElement.h"
#include <algorithm>
#include "HTMLElementFactory.h"
#include "HTMLName.h"
#include "LayoutEngine.h"
#include "LayoutGeometry.h"
#include "PaintContext.h"
#include "ResourceElementDefinition.h"
#include "StylePass.h"
#include "Surface.h"
#include "TextMeasurer.h"

namespace Core {
using detail::HTMLElementFactory;

HTMLLegendElement::HTMLLegendElement()
    : HTMLElement(HTMLTagName(HTMLTag::Legend)) {}

void HTMLLegendElement::constrainResolvedStyle(Style::ComputedStyle& style) const {
    Style::SelfAlignmentData alignSelf = style.alignSelf();
    if (alignSelf.position == Style::ItemPosition::Auto) {
        alignSelf.position = Style::ItemPosition::Start;
        style.setAlignSelf(alignSelf);
    }
}

HTMLFieldsetElement::HTMLFieldsetElement()
    : HTMLElement(HTMLTagName(HTMLTag::Fieldset)) {}

void HTMLFieldsetElement::paint(PaintContext& context, const Style::ComputedStyle& style, float) const {
    context.paintBox(rect(), style, topBorderGap());
}

bool HTMLFieldsetElement::hasLayoutGapBetween(const Element& first, const Element& second) const {
    return !isDirectLegend(first) && !isDirectLegend(second);
}

float HTMLFieldsetElement::layoutOverlapBetween(const Element& first, const Element&, const Style::ComputedStyle& style) const {
    if (!isDirectLegend(first))
        return 0.f;

    return std::max(0.f, Layout::paddingPixels(style).top + first.desiredSize().y * 0.5f - Layout::borderWidths(style).top * 0.5f);
}

void HTMLFieldsetElement::onArranged(const Style::ComputedStyle& style) {
    Element* legend = directLegend();
    if (!legend || !isLegendVisible(*legend))
        return;

    const float legendCenter = (legend->rect().top() + legend->rect().bottom()) * 0.5f;
    const float borderCenter = rect().top() - Layout::borderWidths(style).top * 0.5f;
    const float topDelta = borderCenter - legendCenter;
    if (topDelta != 0.f)
        translateChild(*legend, {0.f, topDelta});
}

bool HTMLFieldsetElement::isDirectLegend(const Element& element) { return element.elementName() == HTMLTagName(HTMLTag::Legend); }

Element* HTMLFieldsetElement::directLegend() {
    for (Element* child : children())
        if (isDirectLegend(*child))
            return child;
    return nullptr;
}

const Element* HTMLFieldsetElement::directLegend() const {
    for (const Element* child : children())
        if (isDirectLegend(*child))
            return child;
    return nullptr;
}

bool HTMLFieldsetElement::isLegendVisible(const Element& legend) const {
    if (const CSS::StyleSheet* sheet = styleSheet()) {
        Style::Pass styles(*sheet, textMetrics(), surface() ? surface()->layoutDirection() : Layout::Direction::LeftToRight);
        return legend.isVisible(styles.style(legend));
    }
    return legend.isVisible(Style::ComputedStyle {});
}

std::optional<TopBorderGap> HTMLFieldsetElement::topBorderGap() const {
    const Element* legend = directLegend();
    if (!legend || !isLegendVisible(*legend))
        return std::nullopt;

    const TopBorderGap gap {legend->rect().left(), legend->rect().right()};
    if (gap.empty())
        return std::nullopt;
    return gap;
}

ResourceElementDefinition detail::ElementDefinitions::fieldset() {
    ResourceElementDefinition definition;
    definition.elementName = HTMLTagName(HTMLTag::Fieldset);

    ScopedElementDefinition legend;
    legend.elementName = HTMLTagName(HTMLTag::Legend);
    legend.acceptedTags = {
        HTMLTag::Abbr,
        HTMLTag::B,
        HTMLTag::Br,
        HTMLTag::Button,
        HTMLTag::Cite,
        HTMLTag::Code,
        HTMLTag::Dfn,
        HTMLTag::Del,
        HTMLTag::Em,
        HTMLTag::I,
        HTMLTag::Input,
        HTMLTag::Ins,
        HTMLTag::Kbd,
        HTMLTag::Label,
        HTMLTag::Mark,
        HTMLTag::Q,
        HTMLTag::S,
        HTMLTag::Small,
        HTMLTag::Strong,
        HTMLTag::U,
    };
    legend.create = [](Element& fieldset, ElementBuildContext& context, const std::string& sourceName, std::size_t line,
                        std::size_t column) -> Element* {
        for (Element* child : fieldset.children()) {
            if (child->elementName() != HTMLTagName(HTMLTag::Legend))
                continue;
            context.error("layout.fieldset.legend_duplicate", "A fieldset accepts only one direct legend.", sourceName, line, column);
            return child;
        }

        auto legend = HTMLElementFactory::create(HTMLTagName(HTMLTag::Legend));
        Element* resultElement = legend.get();
        fieldset.append(std::move(legend));
        return resultElement;
    };
    definition.contentBehavior.scopedElements.emplace(HTMLTagName(HTMLTag::Legend), std::move(legend));
    return definition;
}

ResourceElementDefinition detail::ElementDefinitions::legend() {
    ResourceElementDefinition definition;
    definition.elementName = HTMLTagName(HTMLTag::Legend);
    definition.scopedOnly = true;
    return definition;
}
} // namespace Core
