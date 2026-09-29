/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <cmath>
#include <utility>
#include <variant>
#include "Element.h"
#include "ElementInternal.h"
#include "HTMLName.h"
#include "LayoutGeometry.h"
#include "LayoutPass.h"
#include "StyleSheet.h"
#include "Text.h"

namespace Core::Layout {
using Core::detail::ElementInternalAccess;
using detail::AdjacentLayout;
using detail::adjacentLayout;
using detail::alignSelfOffset;
using detail::allocateMainAxis;
using detail::applyCrossAxisSizing;
using detail::ChildLayout;
using detail::crossAlignment;
using detail::crossAlignmentSafety;
using detail::flexLines;
using detail::gridTrackSizes;
using detail::isDisplayed;
using detail::isInlineLevel;
using detail::isWhitespaceOnlyText;
using detail::justifySelfOffset;
using detail::MainAxisAllocation;
using detail::normalLines;
using detail::outOfFlowRect;
using detail::positionedRect;
using detail::prepareMainAxis;
using detail::relativeRect;
using detail::removeChildrenExcludedFromLayout;
using detail::setArrangedRect;
using detail::styledBoxDimension;
using detail::textAlignmentOffset;
using detail::translatedRect;
using detail::verticalAlignmentOffset;

namespace {
constexpr float kScrollEpsilon = 1.0e-4f;
constexpr std::size_t kMaxScrollLayoutIterations = 4;

void includeOverflow(Rect& bounds, const Rect& candidate, bool includeX, bool includeY) {
    const float left = includeX ? std::min(bounds.left(), candidate.left()) : bounds.left();
    const float right = includeX ? std::max(bounds.right(), candidate.right()) : bounds.right();
    const float bottom = includeY ? std::min(bounds.bottom(), candidate.bottom()) : bounds.bottom();
    const float top = includeY ? std::max(bounds.top(), candidate.top()) : bounds.top();
    bounds = {left, bottom, std::max(0.f, right - left), std::max(0.f, top - bottom)};
}

Rect paddingBox(const Rect& rect, const Style::ComputedStyle& style) {
    const RectEdges<float> borderInsets = borderWidths(style);
    return {rect.x + borderInsets.left, rect.y + borderInsets.bottom, std::max(0.f, rect.w - borderInsets.horizontal()),
        std::max(0.f, rect.h - borderInsets.vertical())};
}

Rect paddingBox(const Element& node, const Style::ComputedStyle& style) { return paddingBox(node.rect(), style); }

Rect containingBlockFor(const Element& parent, Style::Position position, Pass& pass) {
    if (position == Style::Position::Fixed)
        return pass.viewport();
    for (const Element* current = &parent; current; current = current->parentElement()) {
        const Style::ComputedStyle& style = pass.style(*current);
        if (style.position() != Style::Position::Static)
            return paddingBox(*current, style);
    }
    return pass.viewport();
}

Rect containingBlockFor(Style::PseudoElement& parent, Style::Position position, Pass& pass) {
    if (position == Style::Position::Fixed)
        return pass.viewport();
    const Style::ComputedStyle parentStyle = pass.style(parent);
    if (parentStyle.position() != Style::Position::Static)
        return paddingBox(parent.rect(), parentStyle);
    if (parent.parentPseudoElement())
        return containingBlockFor(*parent.parentPseudoElement(), position, pass);
    return containingBlockFor(parent.originatingElement(), position, pass);
}

bool isScrollContainer(Style::Overflow overflow) {
    return overflow == Style::Overflow::Hidden || overflow == Style::Overflow::Scroll || overflow == Style::Overflow::Auto;
}

Rect stickyRect(const Element& node, const Style::ComputedStyle& style, const Rect& rect, Pass& pass) {
    const Element* scrollContainer = node.parentElement();
    while (scrollContainer) {
        const Style::ComputedStyle& scrollStyle = pass.style(*scrollContainer);
        if (isScrollContainer(scrollStyle.overflowX()) || isScrollContainer(scrollStyle.overflowY()))
            break;
        scrollContainer = scrollContainer->parentElement();
    }
    Rect scrollport = scrollContainer ? ElementInternalAccess::scrollport(*scrollContainer) : pass.viewport();
    if (scrollContainer && scrollport.empty())
        scrollport = paddingBox(*scrollContainer, pass.style(*scrollContainer));
    const float scrollLeft = scrollContainer ? scrollContainer->scrollLeft() : 0.f;
    const float scrollTop = scrollContainer ? scrollContainer->scrollTop() : 0.f;
    const float scrollTranslationX = pass.direction() == Direction::RightToLeft ? scrollLeft : -scrollLeft;
    const Rect containingBlock =
        node.parentElement() ? paddingBox(*node.parentElement(), pass.style(*node.parentElement())) : pass.viewport();
    float offsetX = 0.f;
    float offsetY = 0.f;

    const Style::InsetEdge topInset = style.top();
    const Style::InsetEdge bottomInset = style.bottom();
    const Style::InsetEdge leftInset = style.left();
    const Style::InsetEdge rightInset = style.right();
    if (topInset || bottomInset) {
        const float visibleTop = rect.top() + scrollTop;
        const float visibleBottom = rect.bottom() + scrollTop;
        if (topInset) {
            const float inset = topInset->resolve(scrollport.h);
            const float target = std::min(scrollport.top() - inset, containingBlock.top());
            offsetY = std::min(0.f, target - visibleTop);
        }
        if (bottomInset) {
            const float inset = bottomInset->resolve(scrollport.h);
            const float target = std::max(scrollport.bottom() + inset, containingBlock.bottom());
            offsetY = std::max(offsetY, target - visibleBottom);
        }
    }
    if (leftInset || rightInset) {
        const float visibleLeft = rect.left() + scrollTranslationX;
        const float visibleRight = rect.right() + scrollTranslationX;
        if (leftInset) {
            const float inset = leftInset->resolve(scrollport.w);
            const float target = std::max(scrollport.left() + inset, containingBlock.left());
            offsetX = std::max(0.f, target - visibleLeft);
        }
        if (rightInset) {
            const float inset = rightInset->resolve(scrollport.w);
            const float target = std::min(scrollport.right() - inset, containingBlock.right());
            offsetX = std::min(offsetX, target - visibleRight);
        }
    }
    return {rect.x + offsetX, rect.y + offsetY, rect.w, rect.h};
}

float flexBaseline(const ChildLayout& child, Style::ItemPosition preference) {
    const bool inlineLevel = isInlineLevel(child.style.display());
    if (!inlineLevel)
        return preference == Style::ItemPosition::LastBaseline ? 0.f : child.measured.y;
    const auto& lineHeightValue = child.style.lineHeight().mValue;
    const auto* length = std::get_if<Style::LineHeight::Length>(&lineHeightValue);
    const auto* number = std::get_if<Style::LineHeight::Number>(&lineHeightValue);
    const float lineHeight = std::ceil(length ? length->pixels : number ? child.style.fontSize() * number->value : child.style.fontSize());
    const float ascent = std::min(lineHeight, child.style.fontSize() * .8f);
    if (preference == Style::ItemPosition::LastBaseline)
        return std::min(child.measured.y, ascent);
    return std::max(0.f, child.measured.y - lineHeight + ascent);
}

ChildLayout arrangedChild(const OrderedChildRef& node, const Style::ComputedStyle& style, const Vec2& measured) {
    ChildLayout result {node, style, measured, measured, {}};
    if (const Element* element = node.element())
        result.minContent = ElementInternalAccess::layoutCache(*element).minContentSize;
    return result;
}
} // namespace

Rect Engine::scrollableOverflow(Element& node, const Style::ComputedStyle& parentStyle, const Rect& scrollport, Pass& pass) {
    Rect bounds = scrollport;
    const auto childSnapshot = pass.orderedChildrenForLayout(node);
    const std::vector<OrderedChildRef>& children = *childSnapshot;
    const auto collectPseudoOverflow = [&](Style::PseudoElement& pseudoElement, const auto& collectChildren) -> Rect {
        const Style::ComputedStyle pseudoStyle = pass.style(pseudoElement);
        if (pseudoStyle.display() == Style::Display::NoneValue || pseudoStyle.position() == Style::Position::Fixed
            || pseudoElement.rect().empty())
            return {};
        Rect subtree = pseudoElement.rect();
        const bool clipsX = pseudoStyle.overflowX() != Style::Overflow::Visible;
        const bool clipsY = pseudoStyle.overflowY() != Style::Overflow::Visible;
        for (Style::PseudoElement* child : pseudoElement.generatedPseudoElements()) {
            if (!child)
                continue;
            Rect childOverflow = collectChildren(*child, collectChildren);
            if (childOverflow.empty())
                continue;
            if (clipsX) {
                const float left = std::max(pseudoElement.rect().left(), childOverflow.left());
                const float right = std::min(pseudoElement.rect().right(), childOverflow.right());
                childOverflow.x = left;
                childOverflow.w = std::max(0.f, right - left);
            }
            if (clipsY) {
                const float bottom = std::max(pseudoElement.rect().bottom(), childOverflow.bottom());
                const float top = std::min(pseudoElement.rect().top(), childOverflow.top());
                childOverflow.y = bottom;
                childOverflow.h = std::max(0.f, top - bottom);
            }
            includeOverflow(subtree, childOverflow, true, true);
        }
        return subtree;
    };
    for (std::size_t index = 0; index < children.size(); ++index) {
        const OrderedChildRef& childRef = children[index];
        if (!childRef.attachedTo(node))
            continue;
        const Style::ComputedStyle childStyle = pass.style(childRef, parentStyle);
        if (childStyle.position() == Style::Position::Fixed)
            continue;
        if (Style::PseudoElement* pseudoElement = childRef.pseudoElement) {
            includeOverflow(bounds, collectPseudoOverflow(*pseudoElement, collectPseudoOverflow), true, true);
        } else if (Element* child = childRef.element()) {
            if (!child->isDisplayed(childStyle))
                continue;
            includeOverflow(bounds, child->rect(), true, true);
            const Rect& childOverflow = ElementInternalAccess::scrollableOverflow(*child);
            if (!childOverflow.empty())
                includeOverflow(bounds, childOverflow, childStyle.overflowX() == Style::Overflow::Visible,
                    childStyle.overflowY() == Style::Overflow::Visible);
        } else if (Text* text = childRef.text()) {
            if (isWhitespaceOnlyText(childRef) && !pass.preservesNormalFlowWhitespace(children, index, parentStyle))
                continue;
            includeOverflow(bounds, text->rect(), true, true);
        }
    }
    if (bounds.left() < scrollport.left()) {
        bounds.x -= paddingPixels(parentStyle).left;
        bounds.w += paddingPixels(parentStyle).left;
    }
    if (bounds.right() > scrollport.right())
        bounds.w += paddingPixels(parentStyle).right;
    if (bounds.bottom() < scrollport.bottom()) {
        bounds.y -= paddingPixels(parentStyle).bottom;
        bounds.h += paddingPixels(parentStyle).bottom;
    }
    if (bounds.top() > scrollport.top())
        bounds.h += paddingPixels(parentStyle).top;
    return bounds;
}

namespace {
ScrollMetrics scrollMetrics(const Style::ComputedStyle& style, const Rect& scrollport, const Rect& overflow) {
    ScrollMetrics metrics;
    metrics.clientWidth = scrollport.w;
    metrics.clientHeight = scrollport.h;
    metrics.scrollWidth = std::max(metrics.clientWidth, overflow.w);
    metrics.scrollHeight = std::max(metrics.clientHeight, overflow.h);
    if (isScrollContainer(style.overflowX()))
        metrics.maxScrollLeft = metrics.scrollWidth - metrics.clientWidth;
    if (isScrollContainer(style.overflowY()))
        metrics.maxScrollTop = metrics.scrollHeight - metrics.clientHeight;
    return metrics;
}

bool needsScrollbar(Style::Overflow overflow, float extent, float client) {
    if (overflow == Style::Overflow::Scroll)
        return true;
    return overflow == Style::Overflow::Auto && extent > client + kScrollEpsilon;
}

float scrollbarThicknessFor(Style::ScrollbarWidth width, const NativeScrollbarMetrics& metrics) {
    const float thickness = std::max(0.f, metrics.thickness);
    if (width == Style::ScrollbarWidth::NoneValue)
        return 0.f;
    if (width == Style::ScrollbarWidth::Thin)
        return thickness * .5f;
    return thickness;
}

} // namespace

Engine::RowSizing Engine::resolveRowSizes(Element& node, const Style::ComputedStyle& parentStyle, const Rect& available,
    std::vector<ChildLayout>& children, Pass& pass) {
    const NodeSnapshot nodeState(node);
    const auto nodeLayoutValid = [&] {
        return nodeState.layoutValid();
    };
    const float availableMain = available.w;
    const float availableCross = available.h;
    const bool multiLine = isFlexWrapMultiLine(parentStyle.flexWrap()) && availableMain >= 0.f;
    prepareMainAxis(children, Style::FlexDirection::Row, availableMain);
    for (ChildLayout& child : children)
        child.measured.y = styledBoxDimension(child.style, false, child.style.height(), child.style.minHeight(), child.style.maxHeight(),
            child.measured.y, availableCross, child.minContent.y);

    RowSizing sizing;
    sizing.lines = flexLines(node, children, parentStyle, Style::FlexDirection::Row, availableMain);
    const auto& lines = sizing.lines;
    for (const auto& [begin, end] : lines) {
        float preliminaryHeight = 0.f;
        for (std::size_t index = begin; index < end; ++index)
            preliminaryHeight = std::max(preliminaryHeight, children[index].measured.y + verticalMargin(children[index].style.margin()));
        if (!multiLine && availableCross >= 0.f)
            preliminaryHeight = availableCross;
        for (std::size_t index = begin; index < end; ++index)
            applyCrossAxisSizing(children[index].measured, children[index].style, Style::FlexDirection::Row, preliminaryHeight,
                crossAlignment(parentStyle, children[index].style));
    }

    sizing.allocations.reserve(lines.size());
    for (const auto& [begin, end] : lines) {
        if (!nodeLayoutValid()) {
            sizing.valid = false;
            return sizing;
        }
        const MainAxisAllocation allocation =
            allocateMainAxis(node, children, begin, end, parentStyle, Style::FlexDirection::Row, availableMain);
        sizing.allocations.push_back(allocation);
        if (!allocation.valid || !remeasureRowChildren(node, children, begin, end, pass)) {
            sizing.valid = false;
            return sizing;
        }
    }

    if (!nodeLayoutValid()) {
        sizing.valid = false;
        return sizing;
    }
    sizing.lineHeights.reserve(lines.size());
    for (const auto& [begin, end] : lines) {
        float height = 0.f;
        for (std::size_t index = begin; index < end; ++index)
            height = std::max(height, children[index].measured.y + verticalMargin(children[index].style.margin()));
        sizing.lineHeights.push_back(!multiLine && availableCross >= 0.f ? availableCross : height);
    }
    if (multiLine) {
        const Style::AlignContent alignContent = parentStyle.alignContent();
        const detail::ContentDistributionResult distribution = detail::alignContentDistribution(alignContent.position,
            alignContent.distribution, alignContent.overflow, sizing.lineHeights, availableCross, parentStyle.rowGap().fixedPixels());
        sizing.crossOffset = distribution.offset;
        sizing.crossGap = distribution.extraGap;
    } else {
        sizing.crossGap = parentStyle.rowGap().fixedPixels();
    }
    for (std::size_t line = 0; line < lines.size(); ++line) {
        const auto [begin, end] = lines[line];
        for (std::size_t index = begin; index < end; ++index)
            applyCrossAxisSizing(children[index].measured, children[index].style, Style::FlexDirection::Row, sizing.lineHeights[line],
                crossAlignment(parentStyle, children[index].style));
    }
    return sizing;
}

Engine::ColumnSizing Engine::resolveColumnSizes(Element& node, const Style::ComputedStyle& parentStyle, const Rect& available,
    std::vector<ChildLayout>& children, Pass& pass) {
    const NodeSnapshot nodeState(node);
    const auto nodeLayoutValid = [&] {
        return nodeState.layoutValid();
    };
    ColumnSizing sizing;
    const float availableMain = available.h;
    const float availableCross = available.w;
    const bool multiLine = isFlexWrapMultiLine(parentStyle.flexWrap()) && availableMain >= 0.f;
    std::vector<Vec2> initialSizes;
    initialSizes.reserve(children.size());
    for (const ChildLayout& child : children)
        initialSizes.push_back(child.measured);
    for (ChildLayout& child : children) {
        if (!nodeLayoutValid()) {
            sizing.valid = false;
            return sizing;
        }
        child.measured.x = styledBoxDimension(child.style, true, child.style.width(), child.style.minWidth(), child.style.maxWidth(),
            child.measured.x, availableCross, child.minContent.x);
        applyCrossAxisSizing(child.measured, child.style, Style::FlexDirection::Column, availableCross,
            crossAlignment(parentStyle, child.style));
        if (child.style.height().isAuto() && !child.style.aspectRatio) {
            if (!child.node.attachedTo(node))
                continue;
            Element* childNode = child.node.element();
            Style::PseudoElement* pseudoElement = child.node.pseudoElement;
            const ElementRef<Element> childLifetime(childNode);
            const std::uint64_t childRevision = childNode ? childNode->mLayoutInvalidationRevision : 0;
            if (childNode)
                child.measured.y = Engine::measure(*childNode, pass, child.measured.x).y;
            else if (pseudoElement)
                child.measured.y = Engine::measurePseudoElement(*pseudoElement, child.style, child.measured.x, std::nullopt, pass).y;
            else if (Text* text = child.node.text())
                child.measured.y =
                    text->intrinsicSize(pass.styleSheet(), child.style, pass.textMetrics(), {child.measured.x, std::nullopt}).y;
            childNode = childLifetime.get();
            if (!nodeLayoutValid() || !child.node.attachedTo(*nodeState.get())
                || (childNode && (childNode->mLayoutInvalidationRevision != childRevision || childNode->mSurface != nodeState.surface))) {
                sizing.valid = false;
                return sizing;
            }
            child.fitSize.y = child.measured.y;
        }
    }
    prepareMainAxis(children, Style::FlexDirection::Column, availableMain);
    if (!nodeLayoutValid()) {
        sizing.valid = false;
        return sizing;
    }
    sizing.lines = flexLines(node, children, parentStyle, Style::FlexDirection::Column, availableMain);
    sizing.allocations.reserve(sizing.lines.size());
    for (const auto [begin, end] : sizing.lines) {
        const MainAxisAllocation allocation =
            allocateMainAxis(node, children, begin, end, parentStyle, Style::FlexDirection::Column, availableMain);
        sizing.allocations.push_back(allocation);
        if (!allocation.valid) {
            sizing.valid = false;
            return sizing;
        }
    }
    if (!remeasureColumnChildren(node, children, initialSizes, pass)) {
        sizing.valid = false;
        return sizing;
    }
    sizing.lineWidths.reserve(sizing.lines.size());
    for (const auto [begin, end] : sizing.lines) {
        float width = 0.f;
        for (std::size_t index = begin; index < end; ++index)
            width = std::max(width, children[index].measured.x + horizontalMargin(children[index].style.margin()));
        sizing.lineWidths.push_back(!multiLine && availableCross >= 0.f ? availableCross : width);
    }
    if (multiLine) {
        const Style::AlignContent alignContent = parentStyle.alignContent();
        const detail::ContentDistributionResult distribution = detail::alignContentDistribution(alignContent.position,
            alignContent.distribution, alignContent.overflow, sizing.lineWidths, availableCross, parentStyle.columnGap().fixedPixels());
        sizing.crossOffset = distribution.offset;
        sizing.crossGap = distribution.extraGap;
    } else {
        sizing.crossGap = parentStyle.columnGap().fixedPixels();
    }
    for (std::size_t line = 0; line < sizing.lines.size(); ++line) {
        const auto [begin, end] = sizing.lines[line];
        for (std::size_t index = begin; index < end; ++index)
            applyCrossAxisSizing(children[index].measured, children[index].style, Style::FlexDirection::Column, sizing.lineWidths[line],
                crossAlignment(parentStyle, children[index].style));
    }
    if (!remeasureColumnChildren(node, children, initialSizes, pass))
        sizing.valid = false;
    return sizing;
}

std::optional<std::vector<ChildLayout>> Engine::layoutChildren(Element& parent, Style::Display display, const Rect& content, Pass& pass) {
    if (display == Style::Display::Grid || display == Style::Display::InlineGrid) {
        const std::optional<float> contentWidth = content.w >= 0.f ? std::optional<float>(content.w) : std::nullopt;
        const std::optional<float> contentHeight = content.h >= 0.f ? std::optional<float>(content.h) : std::nullopt;
        return measureGridChildren(parent, contentWidth, contentHeight, pass);
    }
    if (!isFlexDisplay(display)) {
        const std::optional<float> contentWidth = content.w >= 0.f ? std::optional<float>(content.w) : std::nullopt;
        const std::optional<float> contentHeight = content.h >= 0.f ? std::optional<float>(content.h) : std::nullopt;
        std::optional<std::vector<ChildLayout>> children = measureNormalChildren(parent, contentWidth, contentHeight, pass);
        if (!children)
            return std::nullopt;
        return std::move(*children);
    }

    const Core::detail::LayoutContextKey contextKey = pass.contextKey();
    const NodeSnapshot parentState(parent);
    std::vector<ChildLayout> result;
    result.reserve(parent.mChildren.size() + parent.generatedPseudoElements().size());
    const Style::ComputedStyle parentStyle = pass.style(parent);
    const auto childSnapshot = pass.orderedChildrenForLayout(parent);
    const std::vector<OrderedChildRef>& children = *childSnapshot;
    for (const OrderedChildRef& childRef : children) {
        Element* child = childRef.element();
        if (!childRef.attachedTo(parent))
            continue;
        if (isWhitespaceOnlyText(childRef))
            continue;
        if (child && child->elementName() == HTMLTagName(HTMLTag::Br))
            continue;
        const std::uint64_t childRevision = child ? child->mLayoutInvalidationRevision : 0;
        const Style::ComputedStyle style = pass.style(childRef, parentStyle);
        if (child ? !child->isDisplayed(style) : style.display() == Style::Display::NoneValue)
            continue;
        if (style.position() == Style::Position::Absolute || style.position() == Style::Position::Fixed)
            continue;
        Element* currentParent = parentState.get();
        child = childRef.element();
        if (!parentState.layoutValid() || !currentParent || !childRef.attachedTo(*currentParent)
            || (child && child->mLayoutInvalidationRevision != childRevision))
            continue;
        Vec2 measured;
        if (child) {
            const bool cacheMatches = ElementInternalAccess::layoutCache(*child).intrinsicValid
                && !child->mInvalidationReasons.intersects(kMeasureInvalidationReasons)
                && ElementInternalAccess::layoutCache(*child).layoutContext == contextKey;
            measured = cacheMatches ? ElementInternalAccess::layoutCache(*child).intrinsicSize : Engine::measure(*child, pass);
        } else if (Style::PseudoElement* pseudoElement = childRef.pseudoElement) {
            measured = Engine::measurePseudoElement(*pseudoElement, style, std::nullopt, std::nullopt, pass);
        } else if (Text* text = childRef.text()) {
            measured = text->intrinsicSize(pass.styleSheet(), style, pass.textMetrics());
        }
        currentParent = parentState.get();
        if (!parentState.layoutValid() || !currentParent || !childRef.attachedTo(*currentParent))
            continue;
        result.push_back(arrangedChild(childRef, style, measured));
    }
    return result;
}

void Engine::arrangeNode(Element& node, Pass& pass) {
    const Core::detail::LayoutContextKey contextKey = pass.contextKey();
    const bool cacheMatches =
        ElementInternalAccess::layoutCache(node).arrangeValid && ElementInternalAccess::layoutCache(node).layoutContext == contextKey;
    if (!node.mInvalidationReasons.intersects(kArrangeInvalidationReasons) && cacheMatches) {
        pass.recordSkipped();
        return;
    }

    pass.recordArranged();
    const ElementRef<Element> lifetime(&node);
    const Surface* surface = node.mSurface;
    const Element* parent = node.mParent;
    const std::uint64_t layoutRevision = node.mLayoutInvalidationRevision;
    const Style::ComputedStyle& parentStyle = pass.style(node);
    Element* styledNode = lifetime.get();
    if (!styledNode || styledNode->mSurface != surface || styledNode->mParent != parent
        || styledNode->mLayoutInvalidationRevision != layoutRevision)
        return;
    const Rect panel = node.mRect;
    const RectEdges<float> parentBorderInsets = borderWidths(parentStyle);
    const Rect paddingBox {
        panel.x + parentBorderInsets.left,
        panel.y + parentBorderInsets.bottom,
        std::max(0.f, panel.w - parentBorderInsets.horizontal()),
        std::max(0.f, panel.h - parentBorderInsets.vertical()),
    };
    const ScrollLayoutOptions& scrollOptions = pass.scrollLayoutOptions();
    const ScrollbarMode scrollbarMode = scrollOptions.scrollbarMode;
    const bool classicScrollbars = scrollbarMode == ScrollbarMode::Classic;
    const bool scrollbarSpaceAvailable = classicScrollbars && parentStyle.scrollbarWidth() != Style::ScrollbarWidth::NoneValue;
    const float scrollbarThickness = scrollbarThicknessFor(parentStyle.scrollbarWidth(), pass.scrollbarMetrics(scrollbarMode));
    const bool stableGutter = parentStyle.scrollbarGutter() != Style::ScrollbarGutter::Auto;
    const bool reservesVerticalGutter = scrollbarSpaceAvailable && stableGutter && isScrollContainer(parentStyle.overflowY());
    const float verticalGutter =
        parentStyle.scrollbarGutter() == Style::ScrollbarGutter::StableBothEdges ? scrollbarThickness * 2.f : scrollbarThickness;
    bool verticalScrollbar = false;
    bool horizontalScrollbar = false;
    Rect scrollport;
    Rect content;
    Rect available;
    Rect overflow;
    ScrollMetrics metrics;
    const bool flexParent = isFlexDisplay(parentStyle.display());
    const bool gridParent = parentStyle.display() == Style::Display::Grid || parentStyle.display() == Style::Display::InlineGrid;
    const auto childSnapshot = pass.orderedChildrenForLayout(node);
    const std::vector<OrderedChildRef>& children = *childSnapshot;
    for (std::size_t index = 0; index < children.size(); ++index) {
        const OrderedChildRef& childRef = children[index];
        if (Text* text = childRef.text();
            text && isWhitespaceOnlyText(childRef) && !pass.preservesNormalFlowWhitespace(children, index, parentStyle))
            text->setRect({});
    }
    for (std::size_t iteration = 0; iteration < kMaxScrollLayoutIterations; ++iteration) {
        const bool reserveVerticalSpace = scrollbarSpaceAvailable && (verticalScrollbar || reservesVerticalGutter);
        const bool reserveHorizontalSpace = scrollbarSpaceAvailable && horizontalScrollbar;
        const bool rightToLeft = pass.direction() == Direction::RightToLeft;
        const bool stableBothEdges = parentStyle.scrollbarGutter() == Style::ScrollbarGutter::StableBothEdges && reservesVerticalGutter;
        const float inlineStartGutter =
            stableBothEdges ? scrollbarThickness : (rightToLeft && reserveVerticalSpace ? scrollbarThickness : 0.f);
        const float blockEndGutter = reserveHorizontalSpace ? scrollbarThickness : 0.f;
        scrollport = {paddingBox.x + inlineStartGutter, paddingBox.y + blockEndGutter,
            std::max(0.f, paddingBox.w - (reserveVerticalSpace ? verticalGutter : 0.f)),
            std::max(0.f, paddingBox.h - (reserveHorizontalSpace ? scrollbarThickness : 0.f))};
        available = {scrollport.x + paddingPixels(parentStyle).left, scrollport.y + paddingPixels(parentStyle).bottom,
            scrollport.w - paddingPixels(parentStyle).horizontal(), scrollport.h - paddingPixels(parentStyle).vertical()};
        content = {available.x, available.y, std::max(0.f, available.w), std::max(0.f, available.h)};
        std::optional<std::vector<ChildLayout>> childrenResult = layoutChildren(node, parentStyle.display(), content, pass);
        if (!childrenResult)
            return;
        std::vector<ChildLayout> children = std::move(*childrenResult);
        if (flexParent && isRowFlexDirection(parentStyle.flexDirection()))
            arrangeRow(node, parentStyle, content, available, children, pass);
        else if (flexParent)
            arrangeColumn(node, parentStyle, content, available, children, pass);
        else if (gridParent)
            arrangeGrid(node, parentStyle, content, children, pass);
        else
            arrangeNormal(node, parentStyle, content, children, pass);

        arrangeOutOfFlowChildren(node, pass);

        Element* currentNode = lifetime.get();
        if (!currentNode || currentNode->mSurface != surface || currentNode->mParent != parent
            || currentNode->mLayoutInvalidationRevision != layoutRevision)
            return;
        overflow = scrollableOverflow(*currentNode, parentStyle, scrollport, pass);
        metrics = scrollMetrics(parentStyle, scrollport, overflow);
        const bool nextVerticalScrollbar =
            scrollbarSpaceAvailable && needsScrollbar(parentStyle.overflowY(), metrics.scrollHeight, metrics.clientHeight);
        const bool nextHorizontalScrollbar =
            scrollbarSpaceAvailable && needsScrollbar(parentStyle.overflowX(), metrics.scrollWidth, metrics.clientWidth);
        if (nextVerticalScrollbar == verticalScrollbar && nextHorizontalScrollbar == horizontalScrollbar)
            break;
        verticalScrollbar = nextVerticalScrollbar;
        horizontalScrollbar = nextHorizontalScrollbar;
    }

    Element* arrangedNode = lifetime.get();
    if (!arrangedNode || arrangedNode->mSurface != surface || arrangedNode->mParent != parent
        || arrangedNode->mLayoutInvalidationRevision != layoutRevision)
        return;
    node.onArranged(parentStyle);
    Element* current = lifetime.get();
    if (!current || current->mSurface != surface || current->mParent != parent || current->mLayoutInvalidationRevision != layoutRevision)
        return;
    overflow = scrollableOverflow(*current, parentStyle, scrollport, pass);
    metrics = scrollMetrics(parentStyle, scrollport, overflow);
    current->setScrollMetrics(metrics, overflow, scrollport);
    ElementInternalAccess::layoutCache(*current).layoutContext = contextKey;
    ElementInternalAccess::layoutCache(*current).arrangeValid = true;
    current->mInvalidationReasons.remove(LayoutInvalidationReason::Arrange);
}

void Engine::setArrangedRect(const OrderedChildRef& node, const Rect& rect, Pass& pass) {
    if (node.pseudoElement)
        node.pseudoElement->setRect(rect);
    else if (Element* element = node.element()) {
        const Style::ComputedStyle& style = pass.style(*element);
        detail::setArrangedRect(*element, style.position() == Style::Position::Sticky ? stickyRect(*element, style, rect, pass) : rect);
    } else if (Text* text = node.text())
        text->setRect(rect);
}

void Engine::arrangeNode(const OrderedChildRef& node, Pass& pass) {
    if (node.pseudoElement)
        arrangePseudoElement(*node.pseudoElement, pass);
    else if (Element* element = node.element())
        arrangeNode(*element, pass);
}

void Engine::arrangeOutOfFlowChild(const OrderedChildRef& childRef, const Style::ComputedStyle& style, const Rect& containingBlock,
    Pass& pass) {
    Element* element = childRef.element();
    if (element) {
        if (!element->isDisplayed(style))
            return;
    } else if (childRef.pseudoElement) {
        if (style.display() == Style::Display::NoneValue)
            return;
    } else {
        return;
    }

    std::optional<float> width;
    std::optional<float> height;
    if (style.width().isPercentage())
        width = styledBoxDimension(style, true, style.width(), style.minWidth(), style.maxWidth(), 0.f, containingBlock.w);
    if (style.height().isPercentage())
        height = styledBoxDimension(style, false, style.height(), style.minHeight(), style.maxHeight(), 0.f, containingBlock.h);
    const Vec2 measured =
        element ? measure(*element, pass, width, height) : measurePseudoElement(*childRef.pseudoElement, style, width, height, pass);
    const ChildLayout child {childRef, style, measured, measured, {},
        element ? ElementInternalAccess::layoutCache(*element).minContentSize : Vec2 {}};
    const Rect base = outOfFlowRect(child, containingBlock);
    setArrangedRect(childRef, translatedRect(child, base), pass);
    arrangeNode(childRef, pass);
}

void Engine::arrangeOutOfFlowChildren(Element& node, Pass& pass) {
    const Style::ComputedStyle parentStyle = pass.style(node);
    const auto childSnapshot = pass.orderedChildrenForLayout(node);
    for (const OrderedChildRef& childRef : *childSnapshot) {
        if (!childRef.attachedTo(node))
            continue;
        const Style::ComputedStyle style = pass.style(childRef, parentStyle);
        if (style.position() != Style::Position::Absolute && style.position() != Style::Position::Fixed)
            continue;
        arrangeOutOfFlowChild(childRef, style, containingBlockFor(node, style.position(), pass), pass);
    }
}

void Engine::arrangeOutOfFlowChildren(Style::PseudoElement& node, Pass& pass) {
    const auto childSnapshot = pass.orderedChildrenForLayout(node);
    for (const OrderedChildRef& childRef : *childSnapshot) {
        if (!childRef.attachedTo(node))
            continue;
        const Style::ComputedStyle style = pass.style(*childRef.pseudoElement);
        if (style.position() != Style::Position::Absolute && style.position() != Style::Position::Fixed)
            continue;
        arrangeOutOfFlowChild(childRef, style, containingBlockFor(node, style.position(), pass), pass);
    }
}

void Engine::arrangePseudoElement(Style::PseudoElement& node, Pass& pass) {
    if (node.style().display() == Style::Display::NoneValue) {
        node.setRect({});
        return;
    }

    const Rect borderBox = node.rect();
    const RectEdges<float> borderInsets = borderWidths(node.style());
    const Rect content {
        borderBox.x + borderInsets.left + paddingPixels(node.style()).left,
        borderBox.y + borderInsets.bottom + paddingPixels(node.style()).bottom,
        std::max(0.f, borderBox.w - borderInsets.horizontal() - paddingPixels(node.style()).horizontal()),
        std::max(0.f, borderBox.h - borderInsets.vertical() - paddingPixels(node.style()).vertical()),
    };
    std::vector<ChildLayout> children;
    const auto childSnapshot = pass.orderedChildrenForLayout(node);
    for (const OrderedChildRef& child : *childSnapshot) {
        if (!child.attachedTo(node) || !child.pseudoElement)
            continue;
        const Style::ComputedStyle childStyle = pass.style(*child.pseudoElement);
        if (childStyle.display() == Style::Display::NoneValue) {
            child.pseudoElement->setRect({});
            continue;
        }
        if (childStyle.position() == Style::Position::Absolute || childStyle.position() == Style::Position::Fixed)
            continue;
        const Vec2 measured = measurePseudoElement(*child.pseudoElement, childStyle, std::nullopt, std::nullopt, pass);
        children.push_back({child, childStyle, measured, measured});
    }

    const auto arrangeChild = [&](ChildLayout& child, const Rect& base) {
        setArrangedRect(child.node, translatedRect(child, relativeRect(child, base, content)), pass);
        arrangeNode(child.node, pass);
    };
    if (children.empty()) {
        arrangeOutOfFlowChildren(node, pass);
        return;
    }

    if (node.style().display() == Style::Display::Grid || node.style().display() == Style::Display::InlineGrid) {
        const detail::GridTrackSizes tracks =
            gridTrackSizes(children, content.w, content.h, node.style().columnGap().fixedPixels(), node.style().rowGap().fixedPixels());
        const auto sumBefore = [](const std::vector<float>& sizes, std::size_t end, std::size_t gapCount, float gap) {
            float result = 0.f;
            for (std::size_t index = 0; index < end; ++index)
                result += sizes[index];
            result += gap * static_cast<float>(gapCount);
            return result;
        };
        const Direction direction = pass.direction();
        for (ChildLayout& child : children) {
            const Style::GridArea area = child.style.gridArea.value_or(Style::GridArea {});
            const std::size_t column = static_cast<std::size_t>(std::max(1, area.column)) - 1;
            const std::size_t row = static_cast<std::size_t>(std::max(1, area.row)) - 1;
            if (column >= tracks.columns.size() || row >= tracks.rows.size())
                continue;
            const float cellLeft = direction == Direction::RightToLeft
                ? content.right() - sumBefore(tracks.columns, column + 1, column, node.style().columnGap().fixedPixels())
                : content.left() + sumBefore(tracks.columns, column, column, node.style().columnGap().fixedPixels());
            const float cellTop = content.top() - sumBefore(tracks.rows, row, row, node.style().rowGap().fixedPixels());
            const float cellWidth = tracks.columns[column];
            const float cellHeight = tracks.rows[row];
            const RectEdges<Style::MarginEdge>& margin = child.style.margin();
            const float availableWidth = std::max(0.f, cellWidth - horizontalMargin(margin));
            const float availableHeight = std::max(0.f, cellHeight - verticalMargin(margin));
            const bool ownJustify = child.style.justifySelf().position != Style::ItemPosition::Auto;
            const Style::ItemPosition justify = ownJustify ? child.style.justifySelf().position : node.style().justifyItems().position;
            const Style::OverflowAlignment justifySafety =
                ownJustify ? child.style.justifySelf().overflow : node.style().justifyItems().overflow;
            const Style::SelfAlignmentData childAlignSelf = child.style.alignSelf();
            const Style::SelfAlignmentData parentAlignItems = node.style().alignItems();
            const bool ownAlign = childAlignSelf.position != Style::ItemPosition::Auto;
            const Style::ItemPosition align = ownAlign ? childAlignSelf.position : parentAlignItems.position;
            const Style::OverflowAlignment alignSafety = ownAlign ? childAlignSelf.overflow : parentAlignItems.overflow;
            const bool stretchWidth = justify == Style::ItemPosition::Normal || justify == Style::ItemPosition::Stretch;
            const bool stretchHeight = align == Style::ItemPosition::Normal || align == Style::ItemPosition::Stretch;
            const float widthFallback =
                child.style.width().isAuto() ? (stretchWidth ? availableWidth : child.measured.x) : child.measured.x;
            const float heightFallback =
                child.style.height().isAuto() ? (stretchHeight ? availableHeight : child.measured.y) : child.measured.y;
            const float width = styledBoxDimension(child.style, true, child.style.width(), child.style.minWidth(), child.style.maxWidth(),
                widthFallback, content.w, child.minContent.x);
            const float height = styledBoxDimension(child.style, false, child.style.height(), child.style.minHeight(),
                child.style.maxHeight(), heightFallback, content.h, child.minContent.y);
            const float horizontalFreeSpace = availableWidth - width;
            const float verticalFreeSpace = availableHeight - height;
            const float x =
                cellLeft + margin.left.fixedPixels() + justifySelfOffset(justify, justifySafety, direction, horizontalFreeSpace);
            const float y = cellTop - margin.top.fixedPixels() - height - alignSelfOffset(align, alignSafety, verticalFreeSpace);
            arrangeChild(child, {x, y, width, height});
        }
        arrangeOutOfFlowChildren(node, pass);
        return;
    }

    if (isFlexDisplay(node.style().display())) {
        const bool row = isRowFlexDirection(node.style().flexDirection());
        const Direction direction = pass.direction();
        const float availableMain = row ? content.w : content.h;
        const float availableCross = row ? content.h : content.w;
        std::vector<Vec2> sizes;
        sizes.reserve(children.size());
        const float mainGap = (row ? node.style().columnGap() : node.style().rowGap()).fixedPixels();
        float usedMain = mainGap * static_cast<float>(children.size() - 1);
        for (ChildLayout& child : children) {
            const Style::ItemPosition alignment = crossAlignment(node.style(), child.style);
            const float widthFallback = !row && child.style.width().isAuto() && alignment == Style::ItemPosition::Stretch
                ? std::max(0.f, availableCross - horizontalMargin(child.style.margin()))
                : child.measured.x;
            const float heightFallback = row && child.style.height().isAuto() && alignment == Style::ItemPosition::Stretch
                ? std::max(0.f, availableCross - verticalMargin(child.style.margin()))
                : child.measured.y;
            const float width = styledBoxDimension(child.style, true, child.style.width(), child.style.minWidth(), child.style.maxWidth(),
                widthFallback, content.w, child.minContent.x);
            const float height = styledBoxDimension(child.style, false, child.style.height(), child.style.minHeight(),
                child.style.maxHeight(), heightFallback, content.h, child.minContent.y);
            sizes.push_back({width, height});
            usedMain += row ? width + horizontalMargin(child.style.margin()) : height + verticalMargin(child.style.margin());
        }
        const float freeSpace = std::max(0.f, availableMain - usedMain);
        const bool forward = row ? (direction == Direction::LeftToRight) != isReverseFlexDirection(node.style().flexDirection())
                                 : !isReverseFlexDirection(node.style().flexDirection());
        float mainPosition = row ? (forward ? content.left() : content.right()) : (forward ? content.top() : content.bottom());
        const detail::ContentDistributionResult distribution =
            detail::justifyContentDistribution(node.style().justifyContent().position, node.style().justifyContent().distribution,
                node.style().justifyContent().overflow, direction, node.style().flexDirection(), freeSpace, children.size());
        if (row)
            mainPosition += forward ? distribution.offset : -distribution.offset;
        else
            mainPosition += forward ? -distribution.offset : distribution.offset;

        for (std::size_t index = 0; index < children.size(); ++index) {
            ChildLayout& child = children[index];
            const RectEdges<Style::MarginEdge>& margin = child.style.margin();
            if (index != 0) {
                const float spacing = mainGap + distribution.extraGap;
                mainPosition += row ? (forward ? spacing : -spacing) : (forward ? -spacing : spacing);
            }
            const float width = sizes[index].x;
            const float height = sizes[index].y;
            const Style::ItemPosition alignment = crossAlignment(node.style(), child.style);
            if (row) {
                const float crossSpace = std::max(0.f, availableCross - height - verticalMargin(margin));
                const float crossOffset = alignment == Style::ItemPosition::Center                         ? crossSpace * .5f
                    : (alignment == Style::ItemPosition::End || alignment == Style::ItemPosition::FlexEnd) ? crossSpace
                                                                                                           : 0.f;
                float x;
                if (!forward) {
                    mainPosition -= margin.right.fixedPixels() + width;
                    x = mainPosition;
                    mainPosition -= margin.left.fixedPixels();
                } else {
                    x = mainPosition + margin.left.fixedPixels();
                    mainPosition = x + width + margin.right.fixedPixels();
                }
                arrangeChild(child, {x, content.top() - margin.top.fixedPixels() - height - crossOffset, width, height});
            } else {
                const bool crossStartRight = direction == Direction::RightToLeft;
                const float crossSpace = std::max(0.f, availableCross - width - horizontalMargin(margin));
                float x = content.left() + margin.left.fixedPixels();
                if (alignment == Style::ItemPosition::Center)
                    x += crossSpace * .5f;
                else {
                    const bool startRight = alignment == Style::ItemPosition::Start ? direction == Direction::RightToLeft
                        : alignment == Style::ItemPosition::End                     ? direction == Direction::LeftToRight
                        : alignment == Style::ItemPosition::FlexEnd                 ? !crossStartRight
                                                                                    : crossStartRight;
                    if (startRight)
                        x = content.right() - margin.right.fixedPixels() - width;
                }
                Rect base;
                if (forward) {
                    mainPosition -= margin.top.fixedPixels() + height;
                    base = {x, mainPosition, width, height};
                    mainPosition -= margin.bottom.fixedPixels();
                } else {
                    mainPosition += margin.bottom.fixedPixels();
                    base = {x, mainPosition, width, height};
                    mainPosition += height + margin.top.fixedPixels();
                }
                arrangeChild(child, base);
            }
        }
        arrangeOutOfFlowChildren(node, pass);
        return;
    }

    float y = content.top();
    for (ChildLayout& child : children) {
        const RectEdges<Style::MarginEdge>& margin = child.style.margin();
        const float widthFallback = child.style.width().isAuto() ? content.w : child.measured.x;
        const float width = styledBoxDimension(child.style, true, child.style.width(), child.style.minWidth(), child.style.maxWidth(),
            widthFallback, content.w, child.minContent.x);
        const float height = styledBoxDimension(child.style, false, child.style.height(), child.style.minHeight(), child.style.maxHeight(),
            child.measured.y, content.h, child.minContent.y);
        y -= margin.top.fixedPixels() + height;
        arrangeChild(child, {content.left() + margin.left.fixedPixels(), y, width, height});
        y -= margin.bottom.fixedPixels();
    }
    arrangeOutOfFlowChildren(node, pass);
}

void Engine::arrangeRow(Element& node, const Style::ComputedStyle& parentStyle, const Rect& content, const Rect& available,
    std::vector<ChildLayout>& children, Pass& pass) {
    const Direction direction = pass.direction();
    const NodeSnapshot nodeState(node);
    removeChildrenExcludedFromLayout(node, children);
    const RowSizing sizing = resolveRowSizes(node, parentStyle, available, children, pass);
    if (!sizing.valid)
        return;
    const auto& lines = sizing.lines;
    const auto& lineHeights = sizing.lineHeights;
    const bool crossStartTop = !isFlexWrapReverse(parentStyle.flexWrap());
    float lineTop = crossStartTop ? content.top() - sizing.crossOffset : content.bottom() + sizing.crossOffset;

    for (std::size_t line = 0; line < sizing.lines.size(); ++line) {
        const auto [begin, end] = sizing.lines[line];
        const float lineHeight = lineHeights[line];
        const float lineBottom = crossStartTop ? lineTop - lineHeight : lineTop;
        const float lineTopForPlacement = crossStartTop ? lineTop : lineBottom + lineHeight;
        const MainAxisAllocation& allocation = sizing.allocations[line];
        const float gap = allocation.gap;
        const float freeSpace = allocation.freeSpace;
        const float autoMargin = allocation.autoMargin;
        const bool forward = (direction == Direction::LeftToRight) != isReverseFlexDirection(parentStyle.flexDirection());
        const detail::ContentDistributionResult distribution = allocation.hasAutoMargins
            ? detail::ContentDistributionResult {}
            : detail::justifyContentDistribution(parentStyle.justifyContent().position, parentStyle.justifyContent().distribution,
                  parentStyle.justifyContent().overflow, direction, parentStyle.flexDirection(), freeSpace, end - begin);
        float x = forward ? content.left() : content.right();
        x += forward ? distribution.offset : -distribution.offset;
        float firstLineBaseline = 0.f;
        float lastLineBaseline = 0.f;
        for (std::size_t index = begin; index < end; ++index) {
            const Style::ItemPosition alignment = crossAlignment(parentStyle, children[index].style);
            if (alignment != Style::ItemPosition::Baseline && alignment != Style::ItemPosition::LastBaseline)
                continue;
            const float baseline = flexBaseline(children[index], alignment) + children[index].style.margin().bottom.fixedPixels();
            if (alignment == Style::ItemPosition::LastBaseline)
                lastLineBaseline = std::max(lastLineBaseline, baseline);
            else
                firstLineBaseline = std::max(firstLineBaseline, baseline);
        }
        for (std::size_t index = begin; index < end; ++index) {
            ChildLayout& child = children[index];
            const OrderedChildRef& current = child.node;
            Element* currentNode = nodeState.get();
            if (!nodeState.layoutValid())
                return;
            if (!current || !current.attachedTo(*currentNode) || !isDisplayed(child))
                continue;
            OrderedChildRef next = index + 1 < end ? children[index + 1].node : OrderedChildRef();
            if (next && (!next.attachedTo(*currentNode) || !isDisplayed(children[index + 1])))
                next = {};
            float adjacencyGap = 0.f;
            float adjacencyOverlap = 0.f;
            if (next) {
                const std::optional<AdjacentLayout> adjacent = adjacentLayout(nodeState, current, next, parentStyle);
                if (!adjacent)
                    return;
                adjacencyGap = (adjacent->hasGap ? gap : 0.f) + distribution.extraGap;
                adjacencyOverlap = adjacent->overlap;
            }
            const RectEdges<Style::MarginEdge>& margin = child.style.margin();
            const float availableCrossSpace = lineHeight - child.measured.y - verticalMargin(margin);
            const float crossSpace = std::max(0.f, availableCrossSpace);
            const int crossAutoCount = verticalAutoMarginCount(margin);
            const float crossAuto = crossAutoCount ? crossSpace / static_cast<float>(crossAutoCount) : 0.f;
            float y = lineBottom + margin.bottom.fixedPixels();
            if (crossAutoCount)
                y += margin.bottom.isAuto() ? crossAuto : 0.f;
            else {
                Style::ItemPosition alignment = crossAlignment(parentStyle, child.style);
                if (crossAlignmentSafety(parentStyle, child.style) == Style::OverflowAlignment::Safe && availableCrossSpace < 0.f) {
                    if (alignment == Style::ItemPosition::Center || alignment == Style::ItemPosition::End)
                        alignment = Style::ItemPosition::Start;
                    else if (alignment == Style::ItemPosition::FlexEnd)
                        alignment = Style::ItemPosition::FlexStart;
                }
                if (alignment == Style::ItemPosition::Baseline || alignment == Style::ItemPosition::LastBaseline) {
                    const float lineBaseline = alignment == Style::ItemPosition::LastBaseline ? lastLineBaseline : firstLineBaseline;
                    y += lineBaseline - flexBaseline(child, alignment);
                } else if (alignment == Style::ItemPosition::Center)
                    y += availableCrossSpace * .5f;
                else {
                    const bool startTop = alignment == Style::ItemPosition::Start ? true
                        : alignment == Style::ItemPosition::End                   ? false
                        : alignment == Style::ItemPosition::FlexStart             ? crossStartTop
                        : alignment == Style::ItemPosition::FlexEnd               ? !crossStartTop
                                                                                  : crossStartTop;
                    if (startTop)
                        y = lineTopForPlacement - margin.top.fixedPixels() - child.measured.y;
                    else
                        y = crossStartTop ? lineBottom + margin.bottom.fixedPixels()
                                          : lineTopForPlacement - margin.top.fixedPixels() - child.measured.y;
                }
            }
            if (!forward) {
                x -= margin.right.fixedPixels() + (margin.right.isAuto() ? autoMargin : 0.f);
                const Rect base {x - child.measured.x, y, child.measured.x, child.measured.y};
                setArrangedRect(child.node, translatedRect(child, relativeRect(child, base, content)), pass);
                x -= child.measured.x + margin.left.fixedPixels() + (margin.left.isAuto() ? autoMargin : 0.f);
                if (next) {
                    x -= adjacencyGap;
                    x += adjacencyOverlap;
                }
            } else {
                x += margin.left.fixedPixels() + (margin.left.isAuto() ? autoMargin : 0.f);
                const Rect base {x, y, child.measured.x, child.measured.y};
                setArrangedRect(child.node, translatedRect(child, relativeRect(child, base, content)), pass);
                x += child.measured.x + margin.right.fixedPixels() + (margin.right.isAuto() ? autoMargin : 0.f);
                if (next) {
                    x += adjacencyGap;
                    x -= adjacencyOverlap;
                }
            }
            arrangeNode(child.node, pass);
            currentNode = nodeState.get();
            if (!nodeState.layoutValid())
                return;
        }
        lineTop = crossStartTop ? lineBottom - sizing.crossGap : lineTopForPlacement + sizing.crossGap;
    }
}

void Engine::arrangeColumn(Element& node, const Style::ComputedStyle& parentStyle, const Rect& content, const Rect& available,
    std::vector<ChildLayout>& children, Pass& pass) {
    const Direction direction = pass.direction();
    const NodeSnapshot nodeState(node);
    removeChildrenExcludedFromLayout(node, children);
    const ColumnSizing sizing = resolveColumnSizes(node, parentStyle, available, children, pass);
    if (!sizing.valid)
        return;
    const bool crossStartRight = (direction == Direction::RightToLeft) != isFlexWrapReverse(parentStyle.flexWrap());
    float crossPosition = crossStartRight ? content.right() - sizing.crossOffset : content.left() + sizing.crossOffset;
    const bool forwardDown = !isReverseFlexDirection(parentStyle.flexDirection());
    for (std::size_t line = 0; line < sizing.lines.size(); ++line) {
        const auto [begin, end] = sizing.lines[line];
        const MainAxisAllocation& allocation = sizing.allocations[line];
        const float lineWidth = sizing.lineWidths[line];
        const float crossLeft = crossStartRight ? crossPosition - lineWidth : crossPosition;
        const float crossRight = crossLeft + lineWidth;
        const detail::ContentDistributionResult distribution = allocation.hasAutoMargins
            ? detail::ContentDistributionResult {}
            : detail::justifyContentDistribution(parentStyle.justifyContent().position, parentStyle.justifyContent().distribution,
                  parentStyle.justifyContent().overflow, direction, parentStyle.flexDirection(), allocation.freeSpace, end - begin);
        float y = forwardDown ? content.top() - distribution.offset : content.bottom() + distribution.offset;
        for (std::size_t index = begin; index < end; ++index) {
            ChildLayout& child = children[index];
            const OrderedChildRef& current = child.node;
            Element* currentNode = nodeState.get();
            if (!nodeState.layoutValid())
                return;
            if (!current || !current.attachedTo(*currentNode) || !isDisplayed(child))
                continue;
            OrderedChildRef previous = index > begin ? children[index - 1].node : OrderedChildRef();
            if (previous && (!previous.attachedTo(*currentNode) || !isDisplayed(children[index - 1])))
                previous = {};
            if (previous) {
                const std::optional<AdjacentLayout> adjacent = adjacentLayout(nodeState, previous, current, parentStyle);
                if (!adjacent)
                    return;
                const float spacing = (adjacent->hasGap ? allocation.gap : 0.f) + distribution.extraGap;
                if (forwardDown) {
                    y -= spacing;
                    y += adjacent->overlap;
                } else {
                    y += spacing;
                    y -= adjacent->overlap;
                }
            }
            const RectEdges<Style::MarginEdge>& margin = child.style.margin();
            const float width = styledBoxDimension(child.style, true, child.style.width(), child.style.minWidth(), child.style.maxWidth(),
                child.measured.x, lineWidth, child.minContent.x);
            const float availableHorizontalSpace = lineWidth - width - horizontalMargin(margin);
            const float horizontalSpace = std::max(0.f, availableHorizontalSpace);
            const int horizontalAutoCount = horizontalAutoMarginCount(margin);
            const float horizontalAuto = horizontalAutoCount ? horizontalSpace / static_cast<float>(horizontalAutoCount) : 0.f;
            float x = crossLeft + margin.left.fixedPixels();
            if (horizontalAutoCount) {
                if (crossStartRight)
                    x = crossRight - margin.right.fixedPixels() - width - (margin.right.isAuto() ? horizontalAuto : 0.f);
                else
                    x += margin.left.isAuto() ? horizontalAuto : 0.f;
            } else {
                Style::ItemPosition alignment = crossAlignment(parentStyle, child.style);
                const Style::OverflowAlignment alignmentSafety = crossAlignmentSafety(parentStyle, child.style);
                if (alignmentSafety == Style::OverflowAlignment::Safe && availableHorizontalSpace < 0.f) {
                    if (alignment == Style::ItemPosition::Center || alignment == Style::ItemPosition::End)
                        alignment = Style::ItemPosition::Start;
                    else if (alignment == Style::ItemPosition::FlexEnd)
                        alignment = Style::ItemPosition::FlexStart;
                }
                const float distributableSpace =
                    alignmentSafety == Style::OverflowAlignment::Safe ? horizontalSpace : availableHorizontalSpace;
                if (alignment == Style::ItemPosition::Center)
                    x += distributableSpace * .5f;
                else if (alignment == Style::ItemPosition::Baseline || alignment == Style::ItemPosition::LastBaseline) {
                    const bool startRight = alignment == Style::ItemPosition::LastBaseline ? direction == Direction::LeftToRight
                                                                                           : direction == Direction::RightToLeft;
                    x = startRight ? crossRight - margin.right.fixedPixels() - width : crossLeft + margin.left.fixedPixels();
                } else {
                    const bool startRight = alignment == Style::ItemPosition::Start ? direction == Direction::RightToLeft
                        : alignment == Style::ItemPosition::End                     ? direction == Direction::LeftToRight
                        : alignment == Style::ItemPosition::FlexEnd                 ? !crossStartRight
                                                                                    : crossStartRight;
                    x = startRight ? crossRight - margin.right.fixedPixels() - width : crossLeft + margin.left.fixedPixels();
                }
            }
            Rect base;
            if (forwardDown) {
                y -= margin.top.fixedPixels() + (margin.top.isAuto() ? allocation.autoMargin : 0.f) + child.measured.y;
                base = {x, y, width, child.measured.y};
            } else {
                y += margin.bottom.fixedPixels() + (margin.bottom.isAuto() ? allocation.autoMargin : 0.f);
                base = {x, y, width, child.measured.y};
            }
            setArrangedRect(child.node, translatedRect(child, relativeRect(child, base, content)), pass);
            arrangeNode(child.node, pass);
            currentNode = nodeState.get();
            if (!nodeState.layoutValid())
                return;
            if (forwardDown)
                y -= margin.bottom.fixedPixels() + (margin.bottom.isAuto() ? allocation.autoMargin : 0.f);
            else
                y += child.measured.y + margin.top.fixedPixels() + (margin.top.isAuto() ? allocation.autoMargin : 0.f);
        }
        crossPosition += crossStartRight ? -(lineWidth + sizing.crossGap) : lineWidth + sizing.crossGap;
    }
}

void Engine::arrangeGrid(Element& node, const Style::ComputedStyle& style, const Rect& content, std::vector<ChildLayout>& children,
    Pass& pass) {
    const Direction direction = pass.direction();
    const NodeSnapshot nodeState(node);
    removeChildrenExcludedFromLayout(node, children);
    detail::GridTrackSizes tracks =
        gridTrackSizes(children, content.w, content.h, style.columnGap().fixedPixels(), style.rowGap().fixedPixels());
    const auto sumBefore = [](const std::vector<float>& sizes, std::size_t end, std::size_t gapCount, float gap) {
        float result = 0.f;
        for (std::size_t index = 0; index < end; ++index)
            result += sizes[index];
        result += gap * static_cast<float>(gapCount);
        return result;
    };
    const auto total = [](const std::vector<float>& sizes) {
        float result = 0.f;
        for (const float size : sizes)
            result += size;
        return result;
    };
    const float columnGap = style.columnGap().fixedPixels();
    const float rowGap = style.rowGap().fixedPixels();
    const float columnFreeSpace = content.w - total(tracks.columns) - columnGap * static_cast<float>(tracks.columns.size() - 1);
    const detail::ContentDistributionResult columnDistribution =
        detail::justifyContentDistribution(style.justifyContent().position, style.justifyContent().distribution,
            style.justifyContent().overflow, direction, Style::FlexDirection::Row, columnFreeSpace, tracks.columns.size(), true);
    if (columnDistribution.stretch && !tracks.columns.empty()) {
        const float extra = std::max(0.f, columnFreeSpace) / static_cast<float>(tracks.columns.size());
        for (float& size : tracks.columns)
            size += extra;
    }
    const Style::AlignContent alignContent = style.alignContent();
    const detail::ContentDistributionResult rowDistribution = detail::alignContentDistribution(alignContent.position,
        alignContent.distribution, alignContent.overflow, tracks.rows, content.h, rowGap);
    const float columnTrackGap = columnGap + columnDistribution.extraGap;
    const float rowTrackGap = rowDistribution.extraGap;

    struct GridPlacement {
        ChildLayout* child = nullptr;
        std::size_t column = 0;
        std::size_t row = 0;
        Rect cell;
        RectEdges<Style::MarginEdge> margin;
        Style::ItemPosition justify = Style::ItemPosition::Normal;
        Style::ItemPosition align = Style::ItemPosition::Normal;
        Style::OverflowAlignment justifySafety = Style::OverflowAlignment::Default;
        Style::OverflowAlignment alignSafety = Style::OverflowAlignment::Default;
        Vec2 size;
    };

    std::vector<GridPlacement> placements;
    placements.reserve(children.size());
    std::vector<float> firstBaselines(tracks.rows.size(), 0.f);
    std::vector<float> lastBaselines(tracks.rows.size(), 0.f);
    std::vector<bool> hasFirstBaseline(tracks.rows.size(), false);
    std::vector<bool> hasLastBaseline(tracks.rows.size(), false);
    for (ChildLayout& child : children) {
        Element* currentNode = nodeState.get();
        if (!nodeState.layoutValid())
            return;
        if (!child.node || !child.node.attachedTo(*currentNode) || !isDisplayed(child))
            continue;

        const Style::GridArea area = child.style.gridArea.value_or(Style::GridArea {});
        const std::size_t column = static_cast<std::size_t>(std::max(1, area.column)) - 1;
        const std::size_t row = static_cast<std::size_t>(std::max(1, area.row)) - 1;
        if (column >= tracks.columns.size() || row >= tracks.rows.size())
            continue;
        const float cellLeft = direction == Direction::RightToLeft
            ? content.right() - columnDistribution.offset - sumBefore(tracks.columns, column + 1, column, columnTrackGap)
            : content.left() + columnDistribution.offset + sumBefore(tracks.columns, column, column, columnTrackGap);
        const float cellTop = content.top() - rowDistribution.offset - sumBefore(tracks.rows, row, row, rowTrackGap);
        const float cellWidth = tracks.columns[column];
        const float cellHeight = tracks.rows[row];
        const RectEdges<Style::MarginEdge>& margin = child.style.margin();
        const float availableWidth = std::max(0.f, cellWidth - horizontalMargin(margin));
        const float availableHeight = std::max(0.f, cellHeight - verticalMargin(margin));
        const bool ownJustify = child.style.justifySelf().position != Style::ItemPosition::Auto;
        const Style::ItemPosition justify = ownJustify ? child.style.justifySelf().position : style.justifyItems().position;
        const Style::OverflowAlignment justifySafety = ownJustify ? child.style.justifySelf().overflow : style.justifyItems().overflow;
        const Style::SelfAlignmentData childAlignSelf = child.style.alignSelf();
        const Style::SelfAlignmentData parentAlignItems = style.alignItems();
        const bool ownAlign = childAlignSelf.position != Style::ItemPosition::Auto;
        const Style::ItemPosition align = ownAlign ? childAlignSelf.position : parentAlignItems.position;
        const Style::OverflowAlignment alignSafety = ownAlign ? childAlignSelf.overflow : parentAlignItems.overflow;
        const bool stretchWidth = justify == Style::ItemPosition::Normal || justify == Style::ItemPosition::Stretch;
        const bool stretchHeight = align == Style::ItemPosition::Normal || align == Style::ItemPosition::Stretch;
        const float widthFallback = child.style.width().isAuto() ? (stretchWidth ? availableWidth : child.measured.x) : child.measured.x;
        const float heightFallback =
            child.style.height().isAuto() ? (stretchHeight ? availableHeight : child.measured.y) : child.measured.y;
        const float width = styledBoxDimension(child.style, true, child.style.width(), child.style.minWidth(), child.style.maxWidth(),
            widthFallback, content.w, child.minContent.x);
        const float height = styledBoxDimension(child.style, false, child.style.height(), child.style.minHeight(), child.style.maxHeight(),
            heightFallback, content.h, child.minContent.y);
        GridPlacement placement {&child, column, row, {cellLeft, cellTop - cellHeight, cellWidth, cellHeight}, margin, justify, align,
            justifySafety, alignSafety, {width, height}};
        const Style::ItemPosition preference = crossAlignment(style, child.style);
        if (preference == Style::ItemPosition::Baseline || preference == Style::ItemPosition::LastBaseline) {
            const float baseline =
                placement.cell.top() - margin.top.fixedPixels() - height + margin.bottom.fixedPixels() + flexBaseline(child, preference);
            if (preference == Style::ItemPosition::LastBaseline) {
                lastBaselines[row] = std::max(lastBaselines[row], baseline);
                hasLastBaseline[row] = true;
            } else {
                firstBaselines[row] = std::max(firstBaselines[row], baseline);
                hasFirstBaseline[row] = true;
            }
        }
        placements.push_back(placement);
    }

    for (GridPlacement& placement : placements) {
        ChildLayout& child = *placement.child;
        const float freeWidth = placement.cell.w - placement.size.x - horizontalMargin(placement.margin);
        const float x = placement.cell.left() + placement.margin.left.fixedPixels()
            + justifySelfOffset(placement.justify, placement.justifySafety, direction, freeWidth);
        const Style::ItemPosition preference = crossAlignment(style, child.style);
        float y;
        if (preference == Style::ItemPosition::Baseline && hasFirstBaseline[placement.row]) {
            y = firstBaselines[placement.row] - placement.margin.bottom.fixedPixels() - flexBaseline(child, preference);
        } else if (preference == Style::ItemPosition::LastBaseline && hasLastBaseline[placement.row]) {
            y = lastBaselines[placement.row] - placement.margin.bottom.fixedPixels() - flexBaseline(child, preference);
        } else {
            const float freeHeight = placement.cell.h - placement.size.y - verticalMargin(placement.margin);
            y = placement.cell.top() - placement.margin.top.fixedPixels() - placement.size.y
                - alignSelfOffset(placement.align, placement.alignSafety, freeHeight);
        }
        const Rect item {x, y, placement.size.x, placement.size.y};
        setArrangedRect(child.node, translatedRect(child, relativeRect(child, item, content)), pass);
        arrangeNode(child.node, pass);
        Element* currentNode = nodeState.get();
        if (!nodeState.layoutValid())
            return;
    }
}

void Engine::arrangeNormal(Element& node, const Style::ComputedStyle& parentStyle, const Rect& content, std::vector<ChildLayout>& children,
    Pass& pass) {
    const Direction direction = pass.direction();
    const NodeSnapshot nodeState(node);
    removeChildrenExcludedFromLayout(node, children);
    const std::optional<float> availableWidth = content.w >= 0.f ? std::optional<float>(content.w) : std::nullopt;
    const std::vector<detail::NormalLine> lines = normalLines(children, availableWidth);
    float contentHeight = 0.f;
    for (const detail::NormalLine& line : lines)
        contentHeight += line.height;
    const float blockAlignmentOffset = parentStyle.alignContentBlockCenter ? (content.h - contentHeight) * .5f : 0.f;
    float lineTop = content.top() - blockAlignmentOffset;
    const bool rtl = direction == Direction::RightToLeft;

    for (const detail::NormalLine& line : lines) {
        const float lineBottom = lineTop - line.height;
        const float freeSpace = std::max(0.f, content.w - line.width);
        const float alignmentOffset = line.block ? 0.f : textAlignmentOffset(parentStyle.textAlign(), direction, freeSpace);
        float x = rtl ? content.right() - alignmentOffset : content.left() + alignmentOffset;
        for (std::size_t index = line.begin; index < line.end; ++index) {
            ChildLayout& child = children[index];
            const OrderedChildRef& childNode = child.node;
            Element* currentNode = nodeState.get();
            if (!nodeState.layoutValid())
                return;
            if (!childNode || !childNode.attachedTo(*currentNode) || !isDisplayed(child))
                continue;

            const RectEdges<Style::MarginEdge>& margin = child.style.margin();
            const float width = styledBoxDimension(child.style, true, child.style.width(), child.style.minWidth(), child.style.maxWidth(),
                child.measured.x, content.w, child.minContent.x);
            const float height = styledBoxDimension(child.style, false, child.style.height(), child.style.minHeight(),
                child.style.maxHeight(), child.measured.y, content.h, child.minContent.y);
            const float horizontalSpace = std::max(0.f, content.w - width - horizontalMargin(margin));
            float childX;
            if (rtl) {
                x -= margin.right.fixedPixels() + (margin.right.isAuto() ? horizontalSpace : 0.f);
                childX = x - width;
                x = childX - margin.left.fixedPixels();
            } else {
                x += margin.left.fixedPixels() + (margin.left.isAuto() ? horizontalSpace : 0.f);
                childX = x;
                x += width + margin.right.fixedPixels();
            }
            float childY = lineBottom + margin.bottom.fixedPixels();
            if (line.block || child.style.verticalAlign().value == Style::VerticalAlign::Top) {
                childY = lineTop - margin.top.fixedPixels() - height;
            } else {
                const float freeSpace = std::max(0.f, line.height - height - verticalMargin(margin));
                childY = lineBottom + margin.bottom.fixedPixels() + verticalAlignmentOffset(child.style, freeSpace);
            }
            Rect base;
            if (Element* element = childNode.element(); element && element->mRectExplicit)
                base = positionedRect(child, content);
            else
                base = {childX, childY, width, height};
            setArrangedRect(child.node, translatedRect(child, relativeRect(child, base, content)), pass);
            arrangeNode(child.node, pass);
            currentNode = nodeState.get();
            if (!nodeState.layoutValid())
                return;
        }
        lineTop = lineBottom;
    }
}
} // namespace Core::Layout
