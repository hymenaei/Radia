/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <cmath>
#include <utility>
#include "Element.h"
#include "HTMLName.h"
#include "IntrinsicSizeConstraints.h"
#include "LayoutGeometry.h"
#include "LayoutPass.h"
#include "StyleSheet.h"
#include "Text.h"
#include "TextMeasurer.h"

namespace Core::Layout {
using Core::detail::ElementInternalAccess;
using detail::AdjacentLayout;
using detail::adjacentLayout;
using detail::allocateMainAxis;
using detail::applyCrossAxisSizing;
using detail::ChildLayout;
using detail::clampBoxDimension;
using detail::contentBoxDimension;
using detail::crossAlignment;
using detail::flexLines;
using detail::gridTrackSizes;
using detail::invalidChildLayout;
using detail::isDisplayed;
using detail::isInlineLevel;
using detail::isWhitespaceOnlyText;
using detail::MainAxisAllocation;
using detail::minimumBoxDimension;
using detail::prepareMainAxis;
using detail::styledBoxDimension;

namespace {
float boxSizingExtra(const Style::ComputedStyle& style, bool horizontal) {
    if (style.boxSizing() == Style::BoxSizing::BorderBox)
        return 0.f;
    const RectEdges<float> borderInsets = borderWidths(style);
    return (horizontal ? paddingPixels(style).horizontal() : paddingPixels(style).vertical())
        + (horizontal ? borderInsets.horizontal() : borderInsets.vertical());
}

float intrinsicContentFallback(const Style::ComputedStyle& style, bool horizontal, const Style::Dimension& value, float maxContent,
    float minContent, std::optional<float> reference) {
    if (!value.isIntrinsic())
        return maxContent;
    switch (value.intrinsicKeyword()) {
    case Style::DimensionKeyword::Content:
        return maxContent;
    case Style::DimensionKeyword::MaxContent:
        return maxContent;
    case Style::DimensionKeyword::MinContent:
        return minContent;
    case Style::DimensionKeyword::FitContent:
        if (!reference)
            return maxContent;
        return std::min(maxContent, std::max(minContent, *reference - boxSizingExtra(style, horizontal)));
    }
    return maxContent;
}

ChildLayout measuredChild(const OrderedChildRef& node, const Style::ComputedStyle& style, const Vec2& measured) {
    ChildLayout result {node, style, measured, measured, {}};
    if (const Element* element = node.element())
        result.minContent = ElementInternalAccess::layoutCache(*element).minContentSize;
    return result;
}
} // namespace

Vec2 Engine::measure(Element& node, Pass& pass, std::optional<float> outerWidth, std::optional<float> outerHeight, bool intrinsicProbe) {
    const Core::detail::LayoutContextKey contextKey = pass.contextKey();
    const CSS::StyleSheet& styleSheet = pass.styleSheet();
    const TextMeasurer& textMetrics = pass.textMetrics();
    const Style::ComputedStyle& style = pass.style(node);
    if (!node.isDisplayed(style)) {
        ElementInternalAccess::layoutCache(node).measuredSize = {};
        ElementInternalAccess::layoutCache(node).minContentSize = {};
        ElementInternalAccess::layoutCache(node).measuredWidth = outerWidth.value_or(0.f);
        ElementInternalAccess::layoutCache(node).measuredHeight = outerHeight.value_or(0.f);
        ElementInternalAccess::layoutCache(node).measuredWidthSet = outerWidth.has_value();
        ElementInternalAccess::layoutCache(node).measuredHeightSet = outerHeight.has_value();
        ElementInternalAccess::layoutCache(node).measuredRectExplicit = node.mRectExplicit;
        ElementInternalAccess::layoutCache(node).measuredRectConstraintSet = node.mRectExplicit;
        ElementInternalAccess::layoutCache(node).measuredRectWidth = node.mRect.w;
        ElementInternalAccess::layoutCache(node).measuredRectHeight = node.mRect.h;
        ElementInternalAccess::layoutCache(node).layoutContext = contextKey;
        ElementInternalAccess::layoutCache(node).measureValid = true;
        ElementInternalAccess::layoutCache(node).intrinsicValid = false;
        ElementInternalAccess::layoutCache(node).arrangeValid = false;
        if (!outerWidth && !outerHeight)
            node.mDesiredSize = {};
        node.mInvalidationReasons.remove(kMeasureInvalidationReasons);
        return {};
    }

    const bool widthMatches = ElementInternalAccess::layoutCache(node).measuredWidthSet == outerWidth.has_value()
        && (!outerWidth || ElementInternalAccess::layoutCache(node).measuredWidth == *outerWidth);
    const bool heightMatches = ElementInternalAccess::layoutCache(node).measuredHeightSet == outerHeight.has_value()
        && (!outerHeight || ElementInternalAccess::layoutCache(node).measuredHeight == *outerHeight);
    const bool rectModeMatches = ElementInternalAccess::layoutCache(node).measuredRectExplicit == node.mRectExplicit;
    const bool rectConstraintMatches = !ElementInternalAccess::layoutCache(node).measuredRectConstraintSet
        || (node.mRectExplicit && ElementInternalAccess::layoutCache(node).measuredRectWidth == node.mRect.w
            && ElementInternalAccess::layoutCache(node).measuredRectHeight == node.mRect.h);
    const bool cacheMatches = ElementInternalAccess::layoutCache(node).measureValid && widthMatches && heightMatches && rectModeMatches
        && rectConstraintMatches && ElementInternalAccess::layoutCache(node).layoutContext == contextKey;
    const bool cacheContextMatches = ElementInternalAccess::layoutCache(node).layoutContext == contextKey;
    if (node.mInvalidationReasons.intersects(kMeasureInvalidationReasons) || !cacheContextMatches || !rectModeMatches
        || !rectConstraintMatches)
        ElementInternalAccess::layoutCache(node).intrinsicValid = false;
    if (!intrinsicProbe && !node.mInvalidationReasons.intersects(kMeasureInvalidationReasons) && cacheMatches) {
        pass.recordSkipped();
        return ElementInternalAccess::layoutCache(node).measuredSize;
    }

    pass.recordMeasured(outerWidth.has_value() || outerHeight.has_value());
    ElementInternalAccess::layoutCache(node).arrangeValid = false;

    const NodeSnapshot styledState(node);
    if (!styledState.layoutValid())
        return {};
    std::optional<float> resolvedWidth = outerWidth;
    if (!resolvedWidth && !style.width().isAuto() && !style.width().isPercentage() && !style.width().isIntrinsic())
        resolvedWidth = styledBoxDimension(style, true, style.width(), style.minWidth(), style.maxWidth(), 0.f);
    if (!resolvedWidth && node.mRectExplicit && style.width().isAuto())
        resolvedWidth = std::max(0.f, node.mRect.w);
    if (!resolvedWidth && node.mRectExplicit && style.width().isPercentage())
        resolvedWidth = std::max(0.f, node.mRect.w);
    std::optional<float> resolvedHeight = outerHeight;
    if (!resolvedHeight && !style.height().isAuto() && !style.height().isPercentage() && !style.height().isIntrinsic())
        resolvedHeight = styledBoxDimension(style, false, style.height(), style.minHeight(), style.maxHeight(), 0.f);
    if (!resolvedHeight && node.mRectExplicit && style.height().isAuto())
        resolvedHeight = std::max(0.f, node.mRect.h);
    if (!resolvedHeight && node.mRectExplicit && style.height().isPercentage())
        resolvedHeight = std::max(0.f, node.mRect.h);
    const IntrinsicSizeConstraints constraints {resolvedWidth, resolvedHeight, pass.nativeMetrics()};
    const ElementRef<Element> lifetime(&node);
    const Surface* surface = node.mSurface;
    const Element* parent = node.mParent;
    const std::uint64_t layoutRevision = node.mLayoutInvalidationRevision;
    const Vec2 intrinsic = node.intrinsicSize(styleSheet, style, textMetrics, constraints);
    Element* current = lifetime.get();
    if (!current || current->mSurface != surface || current->mParent != parent || current->mLayoutInvalidationRevision != layoutRevision)
        return {};
    Vec2 content;
    if (isFlexDisplay(style.display()) && isRowFlexDirection(style.flexDirection()))
        content = measureRow(node, style, intrinsic, resolvedWidth, resolvedHeight, pass);
    else if (isFlexDisplay(style.display()))
        content = measureColumn(node, style, intrinsic, resolvedWidth, resolvedHeight, pass);
    else if (style.display() == Style::Display::Grid || style.display() == Style::Display::InlineGrid)
        content = measureGrid(node, style, intrinsic, resolvedWidth, resolvedHeight, pass);
    else
        content = measureNormal(node, style, intrinsic, resolvedWidth, resolvedHeight, pass);
    current = lifetime.get();
    if (!current || current->mSurface != surface || current->mParent != parent || current->mLayoutInvalidationRevision != layoutRevision)
        return content;

    Vec2 minContent = content;
    if (!intrinsicProbe && (style.width().isIntrinsic() || style.height().isIntrinsic() || style.flexBasis().isIntrinsic())) {
        const bool probeWidthForIntrinsic = style.width().isIntrinsic() || style.flexBasis().isIntrinsic();
        const bool probeHeightForIntrinsic = style.height().isIntrinsic();
        const std::optional<float> probeWidth = probeWidthForIntrinsic ? std::optional<float>(0.f) : resolvedWidth;
        const std::optional<float> probeHeight = probeHeightForIntrinsic ? std::optional<float>(0.f) : resolvedHeight;
        const Vec2 measuredMinContent = measure(node, pass, probeWidth, probeHeight, true);
        const float widthExtra = boxSizingExtra(style, true);
        const float heightExtra = boxSizingExtra(style, false);
        minContent = {std::max(0.f, measuredMinContent.x - widthExtra), std::max(0.f, measuredMinContent.y - heightExtra)};
    }
    const RectEdges<float> borderInsets = borderWidths(style);
    const Vec2 natural(content.x + paddingPixels(style).horizontal() + borderInsets.horizontal(),
        content.y + paddingPixels(style).vertical() + borderInsets.vertical());
    const bool authoredWidth = !style.width().isAuto() && !style.width().isPercentage() && !style.width().isIntrinsic();
    const bool authoredHeight = !style.height().isAuto() && !style.height().isPercentage() && !style.height().isIntrinsic();
    const bool explicitPercentageWidth = node.mRectExplicit && style.width().isPercentage();
    const bool explicitPercentageHeight = node.mRectExplicit && style.height().isPercentage();
    const float widthFallback = style.width().isIntrinsic()
        ? intrinsicContentFallback(style, true, style.width(), content.x, minContent.x, resolvedWidth)
        : natural.x;
    const float heightFallback = style.height().isIntrinsic()
        ? intrinsicContentFallback(style, false, style.height(), content.y, minContent.y, resolvedHeight)
        : natural.y;
    float desiredWidth = styledBoxDimension(style, true, style.width(), style.minWidth(), style.maxWidth(), widthFallback,
        resolvedWidth.value_or(0.f), minContent.x);
    if (explicitPercentageWidth)
        desiredWidth = clampBoxDimension(style, true, *resolvedWidth, style.minWidth(), style.maxWidth(), *resolvedWidth, minContent.x);
    else if (resolvedWidth && (outerWidth || authoredWidth) && !style.width().isIntrinsic() && !(intrinsicProbe && style.width().isAuto()))
        desiredWidth = clampBoxDimension(style, true, *resolvedWidth, style.minWidth(), style.maxWidth(), *resolvedWidth, minContent.x);
    float desiredHeight = styledBoxDimension(style, false, style.height(), style.minHeight(), style.maxHeight(), heightFallback,
        resolvedHeight.value_or(0.f), minContent.y);
    if (explicitPercentageHeight)
        desiredHeight =
            clampBoxDimension(style, false, *resolvedHeight, style.minHeight(), style.maxHeight(), *resolvedHeight, minContent.y);
    else if (resolvedHeight && (outerHeight || authoredHeight) && !style.height().isIntrinsic()
        && !(intrinsicProbe && style.height().isAuto()))
        desiredHeight =
            clampBoxDimension(style, false, *resolvedHeight, style.minHeight(), style.maxHeight(), *resolvedHeight, minContent.y);
    const Vec2 desired = {desiredWidth, desiredHeight};
    ElementInternalAccess::layoutCache(node).measuredSize = desired;
    ElementInternalAccess::layoutCache(node).minContentSize = minContent;
    ElementInternalAccess::layoutCache(node).measuredWidth = outerWidth.value_or(0.f);
    ElementInternalAccess::layoutCache(node).measuredHeight = outerHeight.value_or(0.f);
    ElementInternalAccess::layoutCache(node).measuredWidthSet = outerWidth.has_value();
    ElementInternalAccess::layoutCache(node).measuredHeightSet = outerHeight.has_value();
    ElementInternalAccess::layoutCache(node).measuredRectExplicit = node.mRectExplicit;
    ElementInternalAccess::layoutCache(node).measuredRectConstraintSet = node.mRectExplicit;
    ElementInternalAccess::layoutCache(node).measuredRectWidth = node.mRect.w;
    ElementInternalAccess::layoutCache(node).measuredRectHeight = node.mRect.h;
    ElementInternalAccess::layoutCache(node).layoutContext = contextKey;
    ElementInternalAccess::layoutCache(node).measureValid = true;
    if (!outerWidth && !outerHeight) {
        ElementInternalAccess::layoutCache(node).intrinsicSize = desired;
        ElementInternalAccess::layoutCache(node).intrinsicValid = true;
        node.mDesiredSize = desired;
    }
    node.mInvalidationReasons.remove(kMeasureInvalidationReasons);
    return desired;
}

Vec2 Engine::measurePseudoElement(Style::PseudoElement& node, const Style::ComputedStyle& style, std::optional<float> outerWidth,
    std::optional<float> outerHeight, Pass& pass) {
    if (style.display() == Style::Display::NoneValue) {
        node.setDesiredSize({});
        return {};
    }

    std::optional<float> resolvedWidth = outerWidth;
    if (!resolvedWidth && !style.width().isAuto() && !style.width().isPercentage() && !style.width().isIntrinsic())
        resolvedWidth = styledBoxDimension(style, true, style.width(), style.minWidth(), style.maxWidth(), 0.f);
    std::optional<float> resolvedHeight = outerHeight;
    if (!resolvedHeight && !style.height().isAuto() && !style.height().isPercentage() && !style.height().isIntrinsic())
        resolvedHeight = styledBoxDimension(style, false, style.height(), style.minHeight(), style.maxHeight(), 0.f);

    const Vec2 textSize = style.content && !style.content->empty() ? pass.textMetrics().measureText(*style.content, style) : Vec2 {};
    Vec2 contentSize = textSize;
    std::vector<ChildLayout> children;
    const std::optional<float> contentWidth =
        resolvedWidth ? std::optional<float>(contentBoxDimension(style, true, *resolvedWidth)) : std::nullopt;
    const std::optional<float> contentHeight =
        resolvedHeight ? std::optional<float>(contentBoxDimension(style, false, *resolvedHeight)) : std::nullopt;
    const auto childSnapshot = pass.orderedChildrenForLayout(node);
    for (const OrderedChildRef& child : *childSnapshot) {
        if (!child.attachedTo(node) || !child.pseudoElement)
            continue;
        const Style::ComputedStyle childStyle = pass.style(*child.pseudoElement);
        if (childStyle.display() == Style::Display::NoneValue)
            continue;
        if (childStyle.position() == Style::Position::Absolute || childStyle.position() == Style::Position::Fixed)
            continue;
        const std::optional<float> childWidth = contentWidth && childStyle.width().isPercentage()
            ? std::optional<float>(styledBoxDimension(childStyle, true, childStyle.width(), childStyle.minWidth(), childStyle.maxWidth(),
                  0.f, *contentWidth))
            : std::nullopt;
        const std::optional<float> childHeight = contentHeight && childStyle.height().isPercentage()
            ? std::optional<float>(styledBoxDimension(childStyle, false, childStyle.height(), childStyle.minHeight(),
                  childStyle.maxHeight(), 0.f, *contentHeight))
            : std::nullopt;
        const Vec2 measured = measurePseudoElement(*child.pseudoElement, childStyle, childWidth, childHeight, pass);
        children.push_back({child, childStyle, measured, measured});
    }
    if (!children.empty()) {
        if (style.display() == Style::Display::Grid || style.display() == Style::Display::InlineGrid) {
            const detail::GridTrackSizes tracks =
                gridTrackSizes(children, contentWidth, contentHeight, style.columnGap().fixedPixels(), style.rowGap().fixedPixels());
            for (const float width : tracks.columns)
                contentSize.x += width;
            for (const float height : tracks.rows)
                contentSize.y += height;
            if (tracks.columns.size() > 1)
                contentSize.x += style.columnGap().fixedPixels() * static_cast<float>(tracks.columns.size() - 1);
            if (tracks.rows.size() > 1)
                contentSize.y += style.rowGap().fixedPixels() * static_cast<float>(tracks.rows.size() - 1);
        } else if (isFlexDisplay(style.display()) && isRowFlexDirection(style.flexDirection())) {
            contentSize.x += style.columnGap().fixedPixels() * static_cast<float>(children.size() - 1);
            for (const ChildLayout& child : children) {
                contentSize.x += child.measured.x + horizontalMargin(child.style.margin());
                contentSize.y = std::max(contentSize.y, child.measured.y + verticalMargin(child.style.margin()));
            }
        } else if (isFlexDisplay(style.display())) {
            contentSize.y += style.rowGap().fixedPixels() * static_cast<float>(children.size() - 1);
            for (const ChildLayout& child : children) {
                contentSize.y += child.measured.y + verticalMargin(child.style.margin());
                contentSize.x = std::max(contentSize.x, child.measured.x + horizontalMargin(child.style.margin()));
            }
        } else {
            for (const ChildLayout& child : children) {
                contentSize.x = std::max(contentSize.x, child.measured.x + horizontalMargin(child.style.margin()));
                contentSize.y += child.measured.y + verticalMargin(child.style.margin());
            }
        }
    }
    const RectEdges<float> borderInsets = borderWidths(style);
    const Vec2 natural(contentSize.x + paddingPixels(style).horizontal() + borderInsets.horizontal(),
        contentSize.y + paddingPixels(style).vertical() + borderInsets.vertical());
    const bool authoredWidth = !style.width().isAuto() && !style.width().isPercentage() && !style.width().isIntrinsic();
    const bool authoredHeight = !style.height().isAuto() && !style.height().isPercentage() && !style.height().isIntrinsic();
    float desiredWidth =
        styledBoxDimension(style, true, style.width(), style.minWidth(), style.maxWidth(), natural.x, resolvedWidth.value_or(0.f));
    if (resolvedWidth && (outerWidth || authoredWidth))
        desiredWidth = clampBoxDimension(style, true, *resolvedWidth, style.minWidth(), style.maxWidth(), *resolvedWidth);
    float desiredHeight =
        styledBoxDimension(style, false, style.height(), style.minHeight(), style.maxHeight(), natural.y, resolvedHeight.value_or(0.f));
    if (resolvedHeight && (outerHeight || authoredHeight))
        desiredHeight = clampBoxDimension(style, false, *resolvedHeight, style.minHeight(), style.maxHeight(), *resolvedHeight);

    const Vec2 desired {desiredWidth, desiredHeight};
    node.setDesiredSize(desired);
    return desired;
}

ChildLayout Engine::measureChild(Element& parent, OrderedChildRef child, const Style::ComputedStyle& parentStyle,
    Style::FlexDirection flexDirection, std::optional<float> resolvedWidth, std::optional<float> resolvedHeight, Pass& pass) {
    const NodeSnapshot parentState(parent);
    const OrderedChildRef childState = child;
    Element* childElement = child.element();
    Style::PseudoElement* childPseudoElement = child.pseudoElement;
    const auto isCurrent = [&]() {
        Element* currentParent = parentState.get();
        if (!parentState.layoutValid() || !currentParent || !childState.attachedTo(*currentParent))
            return false;
        return childPseudoElement || !childElement || childElement->mSurface == parentState.surface;
    };

    const Style::ComputedStyle childStyle = pass.style(child, parentStyle);
    if (!isCurrent())
        return invalidChildLayout();
    if (childElement ? !childElement->isDisplayed(childStyle) : childStyle.display() == Style::Display::NoneValue)
        return invalidChildLayout();
    if (childStyle.position() == Style::Position::Absolute || childStyle.position() == Style::Position::Fixed)
        return invalidChildLayout();
    std::optional<float> childWidth;
    std::optional<float> childHeight;
    if (resolvedWidth && childStyle.width().isPercentage())
        childWidth = styledBoxDimension(childStyle, true, childStyle.width(), childStyle.minWidth(), childStyle.maxWidth(), 0.f,
            contentBoxDimension(parentStyle, true, *resolvedWidth));
    if (resolvedHeight && childStyle.height().isPercentage())
        childHeight = styledBoxDimension(childStyle, false, childStyle.height(), childStyle.minHeight(), childStyle.maxHeight(), 0.f,
            contentBoxDimension(parentStyle, false, *resolvedHeight));
    Vec2 childSize;
    if (childElement)
        childSize = measure(*childElement, pass, childWidth, childHeight);
    else if (childPseudoElement)
        childSize = measurePseudoElement(*childPseudoElement, childStyle, childWidth, childHeight, pass);
    else if (Text* text = childState.text())
        childSize = text->intrinsicSize(pass.styleSheet(), childStyle, pass.textMetrics(), {childWidth, childHeight});
    if (!isCurrent())
        return invalidChildLayout();
    Element* currentParent = parentState.get();
    Element* currentElement = childState.element();
    if (currentElement && currentElement->mRectExplicit) {
        if (childStyle.width().isAuto())
            childSize.x = currentElement->mRect.w;
        if (childStyle.height().isAuto())
            childSize.y = currentElement->mRect.h;
    }
    if (childStyle.aspectRatio) {
        std::optional<float> crossSize;
        if (isRowFlexDirection(flexDirection)) {
            if (resolvedHeight)
                crossSize = resolvedHeight;
            else if (!parentStyle.height().isAuto())
                crossSize = styledBoxDimension(parentStyle, false, parentStyle.height(), parentStyle.minHeight(), parentStyle.maxHeight(),
                    currentParent->mRect.h, currentParent->mRect.h);
            else if (currentParent->mRectExplicit)
                crossSize = currentParent->mRect.h;
        } else {
            if (resolvedWidth)
                crossSize = resolvedWidth;
            else if (!parentStyle.width().isAuto())
                crossSize = styledBoxDimension(parentStyle, true, parentStyle.width(), parentStyle.minWidth(), parentStyle.maxWidth(),
                    currentParent->mRect.w, currentParent->mRect.w);
            else if (currentParent->mRectExplicit)
                crossSize = currentParent->mRect.w;
        }
        if (crossSize) {
            const float availableCross = contentBoxDimension(parentStyle, !isRowFlexDirection(flexDirection), *crossSize);
            applyCrossAxisSizing(childSize, childStyle, flexDirection, availableCross, crossAlignment(parentStyle, childStyle));
        }
    }

    Vec2 childAutomaticMinimum = childSize;
    if (!isRowFlexDirection(flexDirection) && resolvedWidth) {
        const float availableCross = contentBoxDimension(parentStyle, true, *resolvedWidth);
        childSize.x = styledBoxDimension(childStyle, true, childStyle.width(), childStyle.minWidth(), childStyle.maxWidth(), childSize.x,
            availableCross);
        applyCrossAxisSizing(childSize, childStyle, flexDirection, availableCross, crossAlignment(parentStyle, childStyle));
        if (childStyle.height().isAuto() && !childStyle.aspectRatio) {
            if (currentElement)
                childSize.y = measure(*currentElement, pass, childSize.x).y;
            else if (childPseudoElement)
                childSize.y = measurePseudoElement(*childPseudoElement, childStyle, childSize.x, std::nullopt, pass).y;
            else if (Text* text = childState.text())
                childSize.y = text->intrinsicSize(pass.styleSheet(), childStyle, pass.textMetrics(), {childSize.x, std::nullopt}).y;
            if (!isCurrent())
                return invalidChildLayout();
        }
        childAutomaticMinimum = childSize;
    }

    const Vec2 childMinContent = childElement ? ElementInternalAccess::layoutCache(*childElement).minContentSize : childSize;
    if (!childStyle.flexBasis().isAuto()) {
        const Style::Dimension& parentDimension = isRowFlexDirection(flexDirection) ? parentStyle.width() : parentStyle.height();
        const std::optional<float> resolvedParent = isRowFlexDirection(flexDirection) ? resolvedWidth : resolvedHeight;
        const float rectSize = isRowFlexDirection(flexDirection) ? currentParent->mRect.w : currentParent->mRect.h;
        const RectEdges<float> parentBorderInsets = borderWidths(parentStyle);
        const float padding = isRowFlexDirection(flexDirection) ? paddingPixels(parentStyle).horizontal() + parentBorderInsets.horizontal()
                                                                : paddingPixels(parentStyle).vertical() + parentBorderInsets.vertical();
        const bool definiteParent = resolvedParent || !parentDimension.isAuto() || parent.mRectExplicit;
        const bool percentageIsAuto = childStyle.flexBasis().isPercentage() && !definiteParent;
        if (!percentageIsAuto) {
            const bool horizontal = isRowFlexDirection(flexDirection);
            const float parentSize = resolvedParent.value_or(parentDimension.isAuto()
                    ? rectSize
                    : styledBoxDimension(parentStyle, horizontal, parentDimension,
                          horizontal ? parentStyle.minWidth() : parentStyle.minHeight(),
                          horizontal ? parentStyle.maxWidth() : parentStyle.maxHeight(), rectSize, rectSize));
            const float reference = std::max(0.f, parentSize - padding);
            const float basis = styledBoxDimension(childStyle, horizontal, childStyle.flexBasis(), std::nullopt, std::nullopt,
                childStyle.flexBasis().isIntrinsic() ? (horizontal ? childAutomaticMinimum.x : childAutomaticMinimum.y) : 0.f, reference,
                horizontal ? childMinContent.x : childMinContent.y);
            const std::optional<Style::Dimension>& authoredMinimum =
                isRowFlexDirection(flexDirection) ? childStyle.minWidth() : childStyle.minHeight();
            const float automaticMinimum = isRowFlexDirection(flexDirection) ? childAutomaticMinimum.x : childAutomaticMinimum.y;
            const float minimum = authoredMinimum ? minimumBoxDimension(childStyle, horizontal, authoredMinimum, reference, basis,
                                                        horizontal ? childMinContent.x : childMinContent.y)
                                                  : std::min(automaticMinimum, basis);
            if (isRowFlexDirection(flexDirection))
                childSize.x = std::max(basis, minimum);
            else
                childSize.y = std::max(basis, minimum);
        }
    }
    if (!isCurrent())
        return invalidChildLayout();
    ChildLayout result {childState, childStyle, childAutomaticMinimum, childSize, {}, childMinContent};
    return result;
}

std::optional<std::vector<ChildLayout>> Engine::measureNormalChildren(Element& parent, std::optional<float> contentWidth,
    std::optional<float> contentHeight, Pass& pass) {
    const NodeSnapshot parentState(parent);
    std::vector<ChildLayout> layouts;
    layouts.reserve(parent.mChildren.size() + parent.generatedPseudoElements().size());
    const Style::ComputedStyle parentStyle = pass.style(parent);
    const auto childSnapshot = pass.orderedChildrenForLayout(parent);
    const std::vector<OrderedChildRef>& children = *childSnapshot;
    bool pendingFlowBreak = false;
    for (std::size_t index = 0; index < children.size(); ++index) {
        const OrderedChildRef& childRef = children[index];
        Element* childPtr = childRef.element();
        if (!childRef.attachedTo(parent))
            continue;
        pendingFlowBreak = pendingFlowBreak || detail::flowBreakBefore(childRef);
        if (isWhitespaceOnlyText(childRef) && !pass.preservesNormalFlowWhitespace(children, index, parentStyle))
            continue;
        if (childPtr && childPtr->elementName() == HTMLTagName(HTMLTag::Br))
            continue;

        const Style::ComputedStyle childStyle = pass.style(childRef, parentStyle);
        if (childStyle.position() == Style::Position::Absolute || childStyle.position() == Style::Position::Fixed)
            continue;
        if (childPtr ? !childPtr->isDisplayed(childStyle) : childStyle.display() == Style::Display::NoneValue)
            continue;

        const auto isCurrent = [&] {
            Element* currentParent = parentState.get();
            return parentState.layoutValid() && currentParent && childRef.attachedTo(*currentParent);
        };
        if (!isCurrent())
            return std::nullopt;

        const bool blockLevel = !isInlineLevel(childStyle.display());
        const bool fillsContainingBlock =
            contentWidth && blockLevel && childStyle.width().isAuto() && (!childPtr || !childPtr->mRectExplicit);
        std::optional<float> childWidth;
        if (contentWidth && childStyle.width().isPercentage())
            childWidth =
                styledBoxDimension(childStyle, true, childStyle.width(), childStyle.minWidth(), childStyle.maxWidth(), 0.f, *contentWidth);
        else if (fillsContainingBlock)
            childWidth = std::max(0.f, *contentWidth - horizontalMargin(childStyle.margin()));
        const std::optional<float> childHeight = contentHeight && childStyle.height().isPercentage()
            ? std::optional<float>(styledBoxDimension(childStyle, false, childStyle.height(), childStyle.minHeight(),
                  childStyle.maxHeight(), 0.f, *contentHeight))
            : std::nullopt;

        const auto measureNode = [&](std::optional<float> width, std::optional<float> height) {
            if (Element* element = childRef.element())
                return Engine::measure(*element, pass, width, height);
            if (Style::PseudoElement* pseudoElement = childRef.pseudoElement)
                return Engine::measurePseudoElement(*pseudoElement, childStyle, width, height, pass);
            if (Text* text = childRef.text())
                return text->intrinsicSize(pass.styleSheet(), childStyle, pass.textMetrics(), {width, height});
            return Vec2 {};
        };
        Vec2 childSize = measureNode(fillsContainingBlock ? std::nullopt : childWidth, childHeight);
        if (!isCurrent())
            return std::nullopt;

        Element* currentChild = childRef.element();
        if (currentChild && currentChild->mRectExplicit) {
            if (childStyle.width().isAuto() && !childWidth)
                childSize.x = currentChild->mRect.w;
            if (childStyle.height().isAuto() && !childHeight)
                childSize.y = currentChild->mRect.h;
        }
        if (fillsContainingBlock) {
            childSize = measureNode(childWidth, childHeight);
            if (!isCurrent())
                return std::nullopt;
        }
        const bool constrainedEllipsisText = childRef.text() && childStyle.textWrapMode() == Style::TextWrapMode::NoWrap
            && childStyle.overflowX() == Style::Overflow::Hidden && childStyle.textOverflow() != Style::TextOverflow::Clip;
        if (contentWidth && !blockLevel && !childWidth && childSize.x + horizontalMargin(childStyle.margin()) > *contentWidth
            && (childStyle.textWrapMode() == Style::TextWrapMode::Wrap || constrainedEllipsisText)) {
            const float availableInlineWidth = std::max(0.f, *contentWidth - horizontalMargin(childStyle.margin()));
            childSize = measureNode(availableInlineWidth, childHeight);
            if (constrainedEllipsisText)
                childSize.x = availableInlineWidth;
            if (!isCurrent())
                return std::nullopt;
        }
        ChildLayout measured = measuredChild(childRef, childStyle, childSize);
        measured.flowBreakBefore = pendingFlowBreak;
        pendingFlowBreak = false;
        layouts.push_back(std::move(measured));
    }
    return layouts;
}

std::optional<std::vector<ChildLayout>> Engine::measureGridChildren(Element& parent, std::optional<float> contentWidth,
    std::optional<float> contentHeight, Pass& pass) {
    const NodeSnapshot parentState(parent);
    std::vector<ChildLayout> layouts;
    layouts.reserve(parent.mChildren.size() + parent.generatedPseudoElements().size());
    const auto childSnapshot = pass.orderedChildrenForLayout(parent);
    const std::vector<OrderedChildRef>& children = *childSnapshot;
    for (const OrderedChildRef& childRef : children) {
        Element* childElement = childRef.element();
        if (!childRef.attachedTo(parent))
            continue;
        if (isWhitespaceOnlyText(childRef))
            continue;
        if (childElement && childElement->elementName() == HTMLTagName(HTMLTag::Br))
            continue;

        const Style::ComputedStyle childStyle = pass.style(childRef, pass.style(parent));
        if (childElement ? !childElement->isDisplayed(childStyle) : childStyle.display() == Style::Display::NoneValue)
            continue;
        if (childStyle.position() == Style::Position::Absolute || childStyle.position() == Style::Position::Fixed)
            continue;

        const auto isCurrent = [&] {
            Element* currentParent = parentState.get();
            return parentState.layoutValid() && currentParent && childRef.attachedTo(*currentParent);
        };
        if (!isCurrent())
            return std::nullopt;

        const std::optional<float> childWidth = contentWidth && childStyle.width().isPercentage()
            ? std::optional<float>(styledBoxDimension(childStyle, true, childStyle.width(), childStyle.minWidth(), childStyle.maxWidth(),
                  0.f, *contentWidth))
            : std::nullopt;
        const std::optional<float> childHeight = contentHeight && childStyle.height().isPercentage()
            ? std::optional<float>(styledBoxDimension(childStyle, false, childStyle.height(), childStyle.minHeight(),
                  childStyle.maxHeight(), 0.f, *contentHeight))
            : std::nullopt;
        Vec2 childSize;
        if (childElement)
            childSize = measure(*childElement, pass, childWidth, childHeight);
        else if (Style::PseudoElement* pseudoElement = childRef.pseudoElement)
            childSize = measurePseudoElement(*pseudoElement, childStyle, childWidth, childHeight, pass);
        else if (Text* text = childRef.text())
            childSize = text->intrinsicSize(pass.styleSheet(), childStyle, pass.textMetrics(), {childWidth, childHeight});
        if (!isCurrent())
            return std::nullopt;

        if (childElement && childElement->mRectExplicit) {
            if (childStyle.width().isAuto() && !childWidth)
                childSize.x = childElement->mRect.w;
            if (childStyle.height().isAuto() && !childHeight)
                childSize.y = childElement->mRect.h;
        }
        layouts.push_back(measuredChild(childRef, childStyle, childSize));
    }
    return layouts;
}

Vec2 Engine::measureGrid(Element& node, const Style::ComputedStyle& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
    std::optional<float> resolvedHeight, Pass& pass) {
    Vec2 content = intrinsic;
    const std::optional<float> contentWidth =
        resolvedWidth ? std::optional<float>(contentBoxDimension(style, true, *resolvedWidth)) : std::nullopt;
    const std::optional<float> contentHeight =
        resolvedHeight ? std::optional<float>(contentBoxDimension(style, false, *resolvedHeight)) : std::nullopt;
    const std::optional<std::vector<ChildLayout>> layoutsResult = measureGridChildren(node, contentWidth, contentHeight, pass);
    if (!layoutsResult)
        return content;
    const detail::GridTrackSizes tracks =
        gridTrackSizes(*layoutsResult, contentWidth, contentHeight, style.columnGap().fixedPixels(), style.rowGap().fixedPixels());
    const auto total = [](const std::vector<float>& sizes) {
        float result = 0.f;
        for (const float size : sizes)
            result += size;
        return result;
    };
    content.x =
        std::max(content.x, total(tracks.columns) + style.columnGap().fixedPixels() * static_cast<float>(tracks.columns.size() - 1));
    content.y = std::max(content.y, total(tracks.rows) + style.rowGap().fixedPixels() * static_cast<float>(tracks.rows.size() - 1));
    return content;
}

Vec2 Engine::measureRow(Element& node, const Style::ComputedStyle& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
    std::optional<float> resolvedHeight, Pass& pass) {
    const NodeSnapshot nodeState(node);
    Vec2 content;
    float rowWidth = intrinsic.x;
    float rowHeight = intrinsic.y;
    std::size_t rowChildren = 0;
    std::size_t rowLines = 0;
    std::vector<ChildLayout> rowLayouts;
    OrderedChildRef previousChild;
    const float itemGap = style.columnGap().fixedPixels();
    const float lineGap = style.rowGap().fixedPixels();
    const auto finishRow = [&] {
        if (!rowChildren && intrinsic.x == 0.f && intrinsic.y == 0.f)
            return;
        content.x = std::max(content.x, rowWidth);
        if (rowLines)
            content.y += lineGap;
        content.y += rowHeight;
        ++rowLines;
        rowWidth = 0.f;
        rowHeight = 0.f;
        rowChildren = 0;
        previousChild = {};
    };

    const auto childSnapshot = pass.orderedChildrenForLayout(node);
    const std::vector<OrderedChildRef>& children = *childSnapshot;
    for (const OrderedChildRef& childRef : children) {
        if (!childRef || !childRef.attachedTo(node))
            continue;
        if (isWhitespaceOnlyText(childRef))
            continue;
        if (const Element* child = childRef.element(); child && child->elementName() == HTMLTagName(HTMLTag::Br))
            continue;
        ChildLayout measured = measureChild(node, childRef, style, Style::FlexDirection::Row, resolvedWidth, resolvedHeight, pass);
        Element* currentNode = nodeState.get();
        if (!nodeState.layoutValid())
            return content;
        if (!measured.node || !measured.node.attachedTo(*currentNode) || !isDisplayed(measured))
            continue;
        const float childOuterWidth = measured.measured.x + horizontalMargin(measured.style.margin());
        const float outerHeight = measured.measured.y + verticalMargin(measured.style.margin());
        rowLayouts.push_back(measured);
        if (previousChild && previousChild.attachedTo(*currentNode)) {
            const std::optional<AdjacentLayout> adjacent = adjacentLayout(nodeState, previousChild, measured.node, style);
            if (!adjacent)
                return content;
            if (adjacent->hasGap)
                rowWidth += itemGap;
            rowWidth -= adjacent->overlap;
        }
        rowWidth += childOuterWidth;
        rowHeight = std::max(rowHeight, outerHeight);
        ++rowChildren;
        previousChild = measured.node;
    }

    if (rowChildren || !rowLines)
        finishRow();
    if (resolvedWidth && !rowLayouts.empty()) {
        const float availableMain = contentBoxDimension(style, true, *resolvedWidth);
        prepareMainAxis(rowLayouts, Style::FlexDirection::Row, availableMain);
        const RowSizing sizing = allocateRowLines(node, rowLayouts, style, availableMain, pass);
        content.y = 0.f;
        for (std::size_t line = 0; line < sizing.lines.size(); ++line) {
            if (line)
                content.y += lineGap;
            float height = line == 0 ? intrinsic.y : 0.f;
            const auto [begin, end] = sizing.lines[line];
            for (std::size_t index = begin; index < end; ++index)
                height = std::max(height, rowLayouts[index].measured.y + verticalMargin(rowLayouts[index].style.margin()));
            content.y += height;
        }
    }
    return content;
}

Vec2 Engine::measureColumn(Element& node, const Style::ComputedStyle& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
    std::optional<float> resolvedHeight, Pass& pass) {
    const NodeSnapshot nodeState(node);
    Vec2 content = intrinsic;
    std::vector<ChildLayout> columnLayouts;
    columnLayouts.reserve(node.mChildren.size() + node.generatedPseudoElements().size());
    OrderedChildRef previousChild;
    const float fixedGap = style.rowGap().fixedPixels();
    const auto childSnapshot = pass.orderedChildrenForLayout(node);
    const std::vector<OrderedChildRef>& children = *childSnapshot;
    for (const OrderedChildRef& childRef : children) {
        if (!childRef || !childRef.attachedTo(node))
            continue;
        if (isWhitespaceOnlyText(childRef))
            continue;
        if (const Element* child = childRef.element(); child && child->elementName() == HTMLTagName(HTMLTag::Br))
            continue;
        ChildLayout measured = measureChild(node, childRef, style, Style::FlexDirection::Column, resolvedWidth, resolvedHeight, pass);
        Element* currentNode = nodeState.get();
        if (!nodeState.layoutValid())
            return content;
        if (!measured.node || !measured.node.attachedTo(*currentNode) || !isDisplayed(measured))
            continue;
        columnLayouts.push_back(measured);
        const float childOuterWidth = measured.measured.x + horizontalMargin(measured.style.margin());
        const float outerHeight = measured.measured.y + verticalMargin(measured.style.margin());
        if (previousChild && previousChild.attachedTo(*currentNode)) {
            const std::optional<AdjacentLayout> adjacent = adjacentLayout(nodeState, previousChild, measured.node, style);
            if (!adjacent)
                return content;
            if (adjacent->hasGap)
                content.y += fixedGap;
            content.y -= adjacent->overlap;
        }
        content.x = std::max(content.x, childOuterWidth);
        content.y += outerHeight;
        previousChild = measured.node;
    }
    if (resolvedHeight && !columnLayouts.empty()) {
        const Rect available {0.f, 0.f, resolvedWidth ? contentBoxDimension(style, true, *resolvedWidth) : -1.f,
            contentBoxDimension(style, false, *resolvedHeight)};
        const ColumnSizing sizing = resolveColumnSizes(node, style, available, columnLayouts, pass);
        if (!sizing.valid)
            return content;
        float width = 0.f;
        float height = 0.f;
        for (std::size_t line = 0; line < sizing.lines.size(); ++line) {
            const auto [begin, end] = sizing.lines[line];
            float lineHeight = 0.f;
            for (std::size_t index = begin; index < end; ++index)
                lineHeight = std::max(lineHeight, columnLayouts[index].measured.y + verticalMargin(columnLayouts[index].style.margin()));
            if (line)
                width += style.columnGap().fixedPixels();
            width += sizing.lineWidths[line];
            height = std::max(height, lineHeight);
        }
        content.x = std::max(content.x, width);
        content.y = std::max(content.y, height);
    }
    return content;
}

Vec2 Engine::measureNormal(Element& node, const Style::ComputedStyle& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
    std::optional<float> resolvedHeight, Pass& pass) {
    Vec2 content = intrinsic;
    const std::optional<float> contentWidth =
        resolvedWidth ? std::optional<float>(contentBoxDimension(style, true, *resolvedWidth)) : std::nullopt;
    const std::optional<float> contentHeight =
        resolvedHeight ? std::optional<float>(contentBoxDimension(style, false, *resolvedHeight)) : std::nullopt;
    const std::optional<std::vector<ChildLayout>> layoutsResult = measureNormalChildren(node, contentWidth, contentHeight, pass);
    if (!layoutsResult)
        return content;
    const std::vector<ChildLayout>& layouts = *layoutsResult;

    const std::vector<detail::NormalLine> lines = detail::normalLines(layouts, contentWidth);
    for (const detail::NormalLine& line : lines) {
        content.x = std::max(content.x, line.width);
        content.y += line.height;
    }
    for (const ChildLayout& child : layouts) {
        const Element* childNode = child.node.element();
        if (!child.node)
            continue;
        if (!childNode || !childNode->mRectExplicit)
            continue;
        content.x = std::max(content.x, childNode->mRect.x + child.measured.x + horizontalMargin(child.style.margin()));
        content.y = std::max(content.y, childNode->mRect.y + child.measured.y + verticalMargin(child.style.margin()));
    }
    return content;
}

bool Engine::remeasureRowChildren(Element& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, Pass& pass) {
    const NodeSnapshot parentState(parent);
    for (std::size_t index = begin; index < end; ++index) {
        ChildLayout& child = children[index];
        if (!child.style.height().isAuto() || child.style.aspectRatio)
            continue;
        Element* node = child.node.element();
        Style::PseudoElement* pseudoElement = child.node.pseudoElement;
        if (!child.node.attachedTo(parent))
            continue;
        const ElementRef<Element> nodeLifetime(node);
        const std::uint64_t nodeRevision = node ? node->mLayoutInvalidationRevision : 0;
        if (node)
            child.measured.y = measure(*node, pass, child.measured.x).y;
        else if (pseudoElement)
            child.measured.y = measurePseudoElement(*pseudoElement, child.style, child.measured.x, std::nullopt, pass).y;
        else if (Text* text = child.node.text())
            child.measured.y = text->intrinsicSize(pass.styleSheet(), child.style, pass.textMetrics(), {child.measured.x, std::nullopt}).y;
        Element* currentParent = parentState.get();
        node = nodeLifetime.get();
        if (!parentState.layoutValid() || !currentParent || !child.node.attachedTo(*currentParent)
            || (node && (node->mSurface != parentState.surface || node->mLayoutInvalidationRevision != nodeRevision)))
            return false;
        child.fitSize.y = child.measured.y;
    }
    return true;
}

bool Engine::remeasureColumnChildren(Element& parent, std::vector<ChildLayout>& children, const std::vector<Vec2>& initialSizes,
    Pass& pass) {
    const NodeSnapshot parentState(parent);
    for (std::size_t index = 0; index < children.size(); ++index) {
        ChildLayout& child = children[index];
        if (std::abs(child.measured.x - initialSizes[index].x) <= 1.0e-4f && std::abs(child.measured.y - initialSizes[index].y) <= 1.0e-4f)
            continue;
        Element* node = child.node.element();
        Style::PseudoElement* pseudoElement = child.node.pseudoElement;
        if (!child.node.attachedTo(parent))
            continue;
        const ElementRef<Element> nodeLifetime(node);
        const std::uint64_t nodeRevision = node ? node->mLayoutInvalidationRevision : 0;
        const Vec2 constrained = node ? measure(*node, pass, child.measured.x, child.measured.y)
            : pseudoElement
            ? measurePseudoElement(*pseudoElement, child.style, child.measured.x, child.measured.y, pass)
            : child.node.text()->intrinsicSize(pass.styleSheet(), child.style, pass.textMetrics(), {child.measured.x, child.measured.y});
        Element* currentParent = parentState.get();
        node = nodeLifetime.get();
        if (!parentState.layoutValid() || !currentParent || !child.node.attachedTo(*currentParent)
            || (node && (node->mSurface != parentState.surface || node->mLayoutInvalidationRevision != nodeRevision)))
            return false;
        child.measured = constrained;
    }
    return true;
}

Engine::RowSizing Engine::allocateRowLines(Element& parent, std::vector<ChildLayout>& children, const Style::ComputedStyle& parentStyle,
    float availableMain, Pass& pass) {
    RowSizing sizing;
    sizing.lines = flexLines(parent, children, parentStyle, Style::FlexDirection::Row, availableMain);
    sizing.allocations.reserve(sizing.lines.size());
    for (const auto& [begin, end] : sizing.lines) {
        const MainAxisAllocation allocation =
            allocateMainAxis(parent, children, begin, end, parentStyle, Style::FlexDirection::Row, availableMain);
        sizing.allocations.push_back(allocation);
        if (!allocation.valid || !remeasureRowChildren(parent, children, begin, end, pass)) {
            sizing.valid = false;
            return sizing;
        }
    }
    return sizing;
}
} // namespace Core::Layout
