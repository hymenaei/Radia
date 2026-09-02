/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <cmath>
#include "dom/element.h"
#include "dom/text.h"
#include "html/elementnames.h"
#include "layout/layoutcontext.h"
#include "style/stylesheet.h"
#include "text/metrics.h"

namespace radia::ui {
using detail::ElementInternalAccess;
using layout_detail::AdjacentLayout;
using layout_detail::adjacentLayout;
using layout_detail::allocateMainAxis;
using layout_detail::applyCrossAxisSizing;
using layout_detail::ChildLayout;
using layout_detail::contentBoxDimension;
using layout_detail::crossAlignment;
using layout_detail::flowBreakBefore;
using layout_detail::gridTrackSizes;
using layout_detail::invalidChildLayout;
using layout_detail::isDisplayed;
using layout_detail::isInlineLevel;
using layout_detail::isWhitespaceOnlyText;
using layout_detail::LayoutChildRef;
using layout_detail::MainAxisAllocation;
using layout_detail::minimumBoxDimension;
using layout_detail::prepareMainAxis;
using layout_detail::rowLines;
using layout_detail::styledBoxDimension;

Vec2 LayoutEngine::measure(Element& node, LayoutPass& pass, std::optional<float> outerWidth, std::optional<float> outerHeight) {
    const LayoutContextKey contextKey = pass.contextKey();
    const StyleSheet& styleSheet = pass.styleSheet();
    const TextMetrics& textMetrics = pass.textMetrics();
    const Style& style = pass.style(node);
    if (!node.isDisplayed(style)) {
        ElementInternalAccess::layoutCache(node).measuredSize = {};
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
        if (!outerWidth && !outerHeight) node.mDesiredSize = {};
        node.mInvalidationReasons.remove(kMeasureInvalidationReasons);
        return {};
    }

    const bool widthMatches = ElementInternalAccess::layoutCache(node).measuredWidthSet == outerWidth.has_value()
        && (!outerWidth || ElementInternalAccess::layoutCache(node).measuredWidth == *outerWidth);
    const bool heightMatches = ElementInternalAccess::layoutCache(node).measuredHeightSet == outerHeight.has_value()
        && (!outerHeight || ElementInternalAccess::layoutCache(node).measuredHeight == *outerHeight);
    const bool rectModeMatches = ElementInternalAccess::layoutCache(node).measuredRectExplicit == node.mRectExplicit;
    const bool rectConstraintMatches = !ElementInternalAccess::layoutCache(node).measuredRectConstraintSet
        || (node.mRectExplicit
            && ElementInternalAccess::layoutCache(node).measuredRectWidth == node.mRect.w
            && ElementInternalAccess::layoutCache(node).measuredRectHeight == node.mRect.h);
    const bool cacheMatches = ElementInternalAccess::layoutCache(node).measureValid
        && widthMatches
        && heightMatches
        && rectModeMatches
        && rectConstraintMatches
        && ElementInternalAccess::layoutCache(node).layoutContext == contextKey;
    const bool cacheContextMatches = ElementInternalAccess::layoutCache(node).layoutContext == contextKey;
    if (node.mInvalidationReasons.intersects(kMeasureInvalidationReasons) || !cacheContextMatches || !rectModeMatches || !rectConstraintMatches)
        ElementInternalAccess::layoutCache(node).intrinsicValid = false;
    if (!node.mInvalidationReasons.intersects(kMeasureInvalidationReasons) && cacheMatches) {
        pass.recordSkipped();
        return ElementInternalAccess::layoutCache(node).measuredSize;
    }

    pass.recordMeasured(outerWidth.has_value() || outerHeight.has_value());
    ElementInternalAccess::layoutCache(node).arrangeValid = false;

    const NodeSnapshot styledState(node);
    if (!styledState.layoutValid()) return {};
    std::optional<float> resolvedWidth = outerWidth;
    if (!resolvedWidth && !style.width.isAuto() && !style.width.isPercentage())
        resolvedWidth = styledBoxDimension(style, true, style.width, style.minWidth, 0.f);
    if (!resolvedWidth && node.mRectExplicit && style.width.isAuto()) resolvedWidth = std::max(0.f, node.mRect.w);
    if (!resolvedWidth && node.mRectExplicit && style.width.isPercentage()) resolvedWidth = std::max(0.f, node.mRect.w);
    std::optional<float> resolvedHeight = outerHeight;
    if (!resolvedHeight && !style.height.isAuto() && !style.height.isPercentage())
        resolvedHeight = styledBoxDimension(style, false, style.height, style.minHeight, 0.f);
    if (!resolvedHeight && node.mRectExplicit && style.height.isAuto()) resolvedHeight = std::max(0.f, node.mRect.h);
    if (!resolvedHeight && node.mRectExplicit && style.height.isPercentage()) resolvedHeight = std::max(0.f, node.mRect.h);
    const IntrinsicSizeConstraints constraints{resolvedWidth, resolvedHeight, &pass.nativeAppearance()};
    const ElementRef<Element> lifetime(&node);
    const Surface* surface = node.mSurface;
    const Element* parent = node.mParent;
    const std::uint64_t layoutRevision = node.mLayoutInvalidationRevision;
    const Vec2 intrinsic = node.intrinsicSize(styleSheet, style, textMetrics, constraints);
    Element* current = lifetime.get();
    if (!current || current->mSurface != surface || current->mParent != parent || current->mLayoutInvalidationRevision != layoutRevision) return {};
    Vec2 content;
    if (isFlexDisplay(style.display) && style.flexDirection == FlexDirection::Row)
        content = measureRow(node, style, intrinsic, resolvedWidth, resolvedHeight, pass);
    else if (isFlexDisplay(style.display)) content = measureColumn(node, style, intrinsic, resolvedWidth, resolvedHeight, pass);
    else if (style.display == DisplayMode::Grid || style.display == DisplayMode::InlineGrid)
        content = measureGrid(node, style, intrinsic, resolvedWidth, resolvedHeight, pass);
    else content = measureNormal(node, style, intrinsic, resolvedWidth, resolvedHeight, pass);
    current = lifetime.get();
    if (!current || current->mSurface != surface || current->mParent != parent || current->mLayoutInvalidationRevision != layoutRevision)
        return content;

    const Vec2 natural(content.x + style.padding.horizontal() + style.borderWidth.horizontal(),
                       content.y + style.padding.vertical() + style.borderWidth.vertical());
    const bool authoredWidth = !style.width.isAuto() && !style.width.isPercentage();
    const bool authoredHeight = !style.height.isAuto() && !style.height.isPercentage();
    const bool explicitPercentageWidth = node.mRectExplicit && style.width.isPercentage();
    const bool explicitPercentageHeight = node.mRectExplicit && style.height.isPercentage();
    float desiredWidth = styledBoxDimension(style, true, style.width, style.minWidth, natural.x, resolvedWidth.value_or(0.f));
    if (explicitPercentageWidth) desiredWidth = std::max(*resolvedWidth, minimumBoxDimension(style, true, style.minWidth, *resolvedWidth));
    else if (resolvedWidth && (outerWidth || authoredWidth))
        desiredWidth = std::max(*resolvedWidth, minimumBoxDimension(style, true, style.minWidth, *resolvedWidth));
    float desiredHeight = styledBoxDimension(style, false, style.height, style.minHeight, natural.y, resolvedHeight.value_or(0.f));
    if (explicitPercentageHeight) desiredHeight = std::max(*resolvedHeight, minimumBoxDimension(style, false, style.minHeight, *resolvedHeight));
    else if (resolvedHeight && (outerHeight || authoredHeight))
        desiredHeight = std::max(*resolvedHeight, minimumBoxDimension(style, false, style.minHeight, *resolvedHeight));
    const Vec2 desired = {desiredWidth, desiredHeight};
    ElementInternalAccess::layoutCache(node).measuredSize = desired;
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

Vec2 LayoutEngine::measurePseudoElement(PseudoElement& node, const Style& style, std::optional<float> outerWidth, std::optional<float> outerHeight,
                                        LayoutPass& pass) {
    if (style.display == DisplayMode::NoneValue) {
        node.setDesiredSize({});
        return {};
    }

    std::optional<float> resolvedWidth = outerWidth;
    if (!resolvedWidth && !style.width.isAuto() && !style.width.isPercentage())
        resolvedWidth = styledBoxDimension(style, true, style.width, style.minWidth, 0.f);
    std::optional<float> resolvedHeight = outerHeight;
    if (!resolvedHeight && !style.height.isAuto() && !style.height.isPercentage())
        resolvedHeight = styledBoxDimension(style, false, style.height, style.minHeight, 0.f);

    const Vec2 textSize = style.content && !style.content->empty() ? pass.textMetrics().measureText(*style.content, style) : Vec2{};
    Vec2 contentSize = textSize;
    std::vector<ChildLayout> children;
    const std::optional<float> contentWidth = resolvedWidth ? std::optional<float>(contentBoxDimension(style, true, *resolvedWidth)) : std::nullopt;
    const std::optional<float> contentHeight =
        resolvedHeight ? std::optional<float>(contentBoxDimension(style, false, *resolvedHeight)) : std::nullopt;
    for (const LayoutChildRef& child : pass.orderedChildrenForLayout(node)) {
        if (!child.attachedTo(node) || !child.pseudoElement) continue;
        const Style childStyle = pass.style(*child.pseudoElement);
        if (childStyle.display == DisplayMode::NoneValue) continue;
        const std::optional<float> childWidth = contentWidth && childStyle.width.isPercentage()
            ? std::optional<float>(styledBoxDimension(childStyle, true, childStyle.width, childStyle.minWidth, 0.f, *contentWidth))
            : std::nullopt;
        const std::optional<float> childHeight = contentHeight && childStyle.height.isPercentage()
            ? std::optional<float>(styledBoxDimension(childStyle, false, childStyle.height, childStyle.minHeight, 0.f, *contentHeight))
            : std::nullopt;
        const Vec2 measured = measurePseudoElement(*child.pseudoElement, childStyle, childWidth, childHeight, pass);
        children.push_back({child, childStyle, measured, measured});
    }
    if (!children.empty()) {
        if (style.display == DisplayMode::Grid || style.display == DisplayMode::InlineGrid) {
            const layout_detail::GridTrackSizes tracks = gridTrackSizes(children, contentWidth, contentHeight);
            for (const float width : tracks.columns) contentSize.x += width;
            for (const float height : tracks.rows) contentSize.y += height;
        } else if (isFlexDisplay(style.display) && style.flexDirection == FlexDirection::Row) {
            contentSize.x += style.gap.fixedPixels() * static_cast<float>(children.size() - 1);
            for (const ChildLayout& child : children) {
                contentSize.x += child.measured.x + child.style.margin.horizontal();
                contentSize.y = std::max(contentSize.y, child.measured.y + child.style.margin.vertical());
            }
        } else if (isFlexDisplay(style.display)) {
            contentSize.y += style.gap.fixedPixels() * static_cast<float>(children.size() - 1);
            for (const ChildLayout& child : children) {
                contentSize.y += child.measured.y + child.style.margin.vertical();
                contentSize.x = std::max(contentSize.x, child.measured.x + child.style.margin.horizontal());
            }
        } else {
            for (const ChildLayout& child : children) {
                contentSize.x = std::max(contentSize.x, child.measured.x + child.style.margin.horizontal());
                contentSize.y += child.measured.y + child.style.margin.vertical();
            }
        }
    }
    const Vec2 natural(contentSize.x + style.padding.horizontal() + style.borderWidth.horizontal(),
                       contentSize.y + style.padding.vertical() + style.borderWidth.vertical());
    const bool authoredWidth = !style.width.isAuto() && !style.width.isPercentage();
    const bool authoredHeight = !style.height.isAuto() && !style.height.isPercentage();
    float desiredWidth = styledBoxDimension(style, true, style.width, style.minWidth, natural.x, resolvedWidth.value_or(0.f));
    if (resolvedWidth && (outerWidth || authoredWidth))
        desiredWidth = std::max(*resolvedWidth, minimumBoxDimension(style, true, style.minWidth, *resolvedWidth));
    float desiredHeight = styledBoxDimension(style, false, style.height, style.minHeight, natural.y, resolvedHeight.value_or(0.f));
    if (resolvedHeight && (outerHeight || authoredHeight))
        desiredHeight = std::max(*resolvedHeight, minimumBoxDimension(style, false, style.minHeight, *resolvedHeight));

    const Vec2 desired{desiredWidth, desiredHeight};
    node.setDesiredSize(desired);
    return desired;
}

ChildLayout LayoutEngine::measureChild(Element& parent, LayoutChildRef child, const Style& parentStyle, FlexDirection flexDirection,
                                       std::optional<float> resolvedWidth, std::optional<float> resolvedHeight, LayoutPass& pass) {
    const NodeSnapshot parentState(parent);
    const LayoutChildRef childState = child;
    Element* childElement = child.element();
    PseudoElement* childPseudoElement = child.pseudoElement;
    const auto isCurrent = [&]() {
        Element* currentParent = parentState.get();
        if (!parentState.layoutValid() || !currentParent || !childState.attachedTo(*currentParent)) return false;
        return childPseudoElement || !childElement || childElement->mSurface == parentState.surface;
    };

    const Style childStyle = pass.style(child, parentStyle);
    if (!isCurrent()) return invalidChildLayout();
    if (childElement ? !childElement->isDisplayed(childStyle) : childStyle.display == DisplayMode::NoneValue) return invalidChildLayout();
    std::optional<float> childWidth;
    std::optional<float> childHeight;
    if (resolvedWidth && childStyle.width.isPercentage())
        childWidth =
            styledBoxDimension(childStyle, true, childStyle.width, childStyle.minWidth, 0.f, contentBoxDimension(parentStyle, true, *resolvedWidth));
    if (resolvedHeight && childStyle.height.isPercentage())
        childHeight = styledBoxDimension(childStyle, false, childStyle.height, childStyle.minHeight, 0.f,
                                         contentBoxDimension(parentStyle, false, *resolvedHeight));
    Vec2 childSize;
    if (childElement) childSize = measure(*childElement, pass, childWidth, childHeight);
    else if (childPseudoElement) childSize = measurePseudoElement(*childPseudoElement, childStyle, childWidth, childHeight, pass);
    else if (Text* text = childState.text())
        childSize = text->intrinsicSize(pass.styleSheet(), childStyle, pass.textMetrics(), {childWidth, childHeight});
    if (!isCurrent()) return invalidChildLayout();
    Element* currentParent = parentState.get();
    Element* currentElement = childState.element();
    if (currentElement && currentElement->mRectExplicit) {
        if (childStyle.width.isAuto()) childSize.x = currentElement->mRect.w;
        if (childStyle.height.isAuto()) childSize.y = currentElement->mRect.h;
    }
    if (childStyle.aspectRatio) {
        std::optional<float> crossSize;
        if (flexDirection == FlexDirection::Row) {
            if (resolvedHeight) crossSize = resolvedHeight;
            else if (!parentStyle.height.isAuto())
                crossSize =
                    styledBoxDimension(parentStyle, false, parentStyle.height, parentStyle.minHeight, currentParent->mRect.h, currentParent->mRect.h);
            else if (currentParent->mRectExplicit) crossSize = currentParent->mRect.h;
        } else {
            if (resolvedWidth) crossSize = resolvedWidth;
            else if (!parentStyle.width.isAuto())
                crossSize =
                    styledBoxDimension(parentStyle, true, parentStyle.width, parentStyle.minWidth, currentParent->mRect.w, currentParent->mRect.w);
            else if (currentParent->mRectExplicit) crossSize = currentParent->mRect.w;
        }
        if (crossSize) {
            const float availableCross = contentBoxDimension(parentStyle, flexDirection == FlexDirection::Column, *crossSize);
            applyCrossAxisSizing(childSize, childStyle, flexDirection, availableCross, crossAlignment(parentStyle, childStyle, flexDirection));
        }
    }

    Vec2 childAutomaticMinimum = childSize;
    if (flexDirection == FlexDirection::Column && resolvedWidth) {
        const float availableCross = contentBoxDimension(parentStyle, true, *resolvedWidth);
        childSize.x = styledBoxDimension(childStyle, true, childStyle.width, childStyle.minWidth, childSize.x, availableCross);
        applyCrossAxisSizing(childSize, childStyle, flexDirection, availableCross, crossAlignment(parentStyle, childStyle, flexDirection));
        if (childStyle.height.isAuto() && !childStyle.aspectRatio) {
            if (currentElement) childSize.y = measure(*currentElement, pass, childSize.x).y;
            else if (childPseudoElement) childSize.y = measurePseudoElement(*childPseudoElement, childStyle, childSize.x, std::nullopt, pass).y;
            else if (Text* text = childState.text())
                childSize.y = text->intrinsicSize(pass.styleSheet(), childStyle, pass.textMetrics(), {childSize.x, std::nullopt}).y;
            if (!isCurrent()) return invalidChildLayout();
        }
        childAutomaticMinimum = childSize;
    }

    if (!childStyle.flexBasis.isAuto()) {
        const Dimension& parentDimension = flexDirection == FlexDirection::Row ? parentStyle.width : parentStyle.height;
        const std::optional<float> resolvedParent = flexDirection == FlexDirection::Row ? resolvedWidth : resolvedHeight;
        const float rectSize = flexDirection == FlexDirection::Row ? currentParent->mRect.w : currentParent->mRect.h;
        const float padding = flexDirection == FlexDirection::Row ? parentStyle.padding.horizontal() + parentStyle.borderWidth.horizontal()
                                                                  : parentStyle.padding.vertical() + parentStyle.borderWidth.vertical();
        const bool definiteParent = resolvedParent || !parentDimension.isAuto() || parent.mRectExplicit;
        const bool percentageIsAuto = childStyle.flexBasis.isPercentage() && !definiteParent;
        if (!percentageIsAuto) {
            const bool horizontal = flexDirection == FlexDirection::Row;
            const float parentSize = resolvedParent.value_or(
                parentDimension.isAuto() ? rectSize
                                         : styledBoxDimension(parentStyle, horizontal, parentDimension,
                                                              horizontal ? parentStyle.minWidth : parentStyle.minHeight, rectSize, rectSize));
            const float reference = std::max(0.f, parentSize - padding);
            const float basis = styledBoxDimension(childStyle, horizontal, childStyle.flexBasis, std::nullopt, 0.f, reference);
            const std::optional<Length>& authoredMinimum = flexDirection == FlexDirection::Row ? childStyle.minWidth : childStyle.minHeight;
            const float automaticMinimum = flexDirection == FlexDirection::Row ? childAutomaticMinimum.x : childAutomaticMinimum.y;
            const float minimum =
                authoredMinimum ? minimumBoxDimension(childStyle, horizontal, authoredMinimum, reference) : std::min(automaticMinimum, basis);
            if (flexDirection == FlexDirection::Row) childSize.x = std::max(basis, minimum);
            else childSize.y = std::max(basis, minimum);
        }
    }
    if (!isCurrent()) return invalidChildLayout();
    return {childState, childStyle, childAutomaticMinimum, childSize};
}

std::optional<std::vector<ChildLayout>> LayoutEngine::measureNormalChildren(Element& parent, std::optional<float> contentWidth,
                                                                            std::optional<float> contentHeight, LayoutPass& pass) {
    const NodeSnapshot parentState(parent);
    std::vector<ChildLayout> layouts;
    layouts.reserve(parent.mChildren.size() + parent.generatedPseudoElements().size());
    const Style parentStyle = pass.style(parent);
    const std::vector<LayoutChildRef> children = orderedChildren(parent, pass);
    for (std::size_t index = 0; index < children.size(); ++index) {
        const LayoutChildRef& childRef = children[index];
        Element* childPtr = childRef.element();
        if (!childRef.attachedTo(parent)) continue;
        if (isWhitespaceOnlyText(childRef) && !pass.preservesNormalFlowWhitespace(children, index, parentStyle)) continue;
        if (childPtr && childPtr->elementName() == kBrTag.localName) continue;

        const Style childStyle = pass.style(childRef, parentStyle);
        if (childPtr ? !childPtr->isDisplayed(childStyle) : childStyle.display == DisplayMode::NoneValue) continue;

        const auto isCurrent = [&] {
            Element* currentParent = parentState.get();
            return parentState.layoutValid() && currentParent && childRef.attachedTo(*currentParent);
        };
        if (!isCurrent()) return std::nullopt;

        const bool blockLevel = !isInlineLevel(childStyle.display);
        const bool fillsContainingBlock = contentWidth && blockLevel && childStyle.width.isAuto() && (!childPtr || !childPtr->mRectExplicit);
        std::optional<float> childWidth;
        if (contentWidth && childStyle.width.isPercentage())
            childWidth = styledBoxDimension(childStyle, true, childStyle.width, childStyle.minWidth, 0.f, *contentWidth);
        else if (fillsContainingBlock) childWidth = std::max(0.f, *contentWidth - childStyle.margin.horizontal());
        const std::optional<float> childHeight = contentHeight && childStyle.height.isPercentage()
            ? std::optional<float>(styledBoxDimension(childStyle, false, childStyle.height, childStyle.minHeight, 0.f, *contentHeight))
            : std::nullopt;

        const auto measureNode = [&](std::optional<float> width, std::optional<float> height) {
            if (Element* element = childRef.element()) return LayoutEngine::measure(*element, pass, width, height);
            if (PseudoElement* pseudoElement = childRef.pseudoElement)
                return LayoutEngine::measurePseudoElement(*pseudoElement, childStyle, width, height, pass);
            if (Text* text = childRef.text()) return text->intrinsicSize(pass.styleSheet(), childStyle, pass.textMetrics(), {width, height});
            return Vec2{};
        };
        Vec2 childSize = measureNode(fillsContainingBlock ? std::nullopt : childWidth, childHeight);
        if (!isCurrent()) return std::nullopt;

        Element* currentChild = childRef.element();
        if (currentChild && currentChild->mRectExplicit) {
            if (childStyle.width.isAuto() && !childWidth) childSize.x = currentChild->mRect.w;
            if (childStyle.height.isAuto() && !childHeight) childSize.y = currentChild->mRect.h;
        }
        if (fillsContainingBlock) {
            childSize = measureNode(childWidth, childHeight);
            if (!isCurrent()) return std::nullopt;
        }
        const bool constrainedEllipsisText = childRef.text()
            && childStyle.textWrap == TextWrap::NoWrap
            && childStyle.overflowX == Overflow::Hidden
            && childStyle.textOverflow != TextOverflow::Clip;
        if (contentWidth
            && !blockLevel
            && !childWidth
            && childSize.x + childStyle.margin.horizontal() > *contentWidth
            && (childStyle.textWrap == TextWrap::Wrap || constrainedEllipsisText)) {
            const float availableInlineWidth = std::max(0.f, *contentWidth - childStyle.margin.horizontal());
            childSize = measureNode(availableInlineWidth, childHeight);
            if (constrainedEllipsisText) childSize.x = availableInlineWidth;
            if (!isCurrent()) return std::nullopt;
        }
        layouts.push_back({childRef, childStyle, childSize, childSize});
    }
    return layouts;
}

std::optional<std::vector<ChildLayout>> LayoutEngine::measureGridChildren(Element& parent, std::optional<float> contentWidth,
                                                                          std::optional<float> contentHeight, LayoutPass& pass) {
    const NodeSnapshot parentState(parent);
    std::vector<ChildLayout> layouts;
    layouts.reserve(parent.mChildren.size() + parent.generatedPseudoElements().size());
    const std::vector<LayoutChildRef> children = orderedChildren(parent, pass);
    for (const LayoutChildRef& childRef : children) {
        Element* childElement = childRef.element();
        if (!childRef.attachedTo(parent)) continue;
        if (isWhitespaceOnlyText(childRef)) continue;
        if (childElement && childElement->elementName() == kBrTag.localName) continue;

        const Style childStyle = pass.style(childRef, pass.style(parent));
        if (childElement ? !childElement->isDisplayed(childStyle) : childStyle.display == DisplayMode::NoneValue) continue;

        const auto isCurrent = [&] {
            Element* currentParent = parentState.get();
            return parentState.layoutValid() && currentParent && childRef.attachedTo(*currentParent);
        };
        if (!isCurrent()) return std::nullopt;

        const std::optional<float> childWidth = contentWidth && childStyle.width.isPercentage()
            ? std::optional<float>(styledBoxDimension(childStyle, true, childStyle.width, childStyle.minWidth, 0.f, *contentWidth))
            : std::nullopt;
        const std::optional<float> childHeight = contentHeight && childStyle.height.isPercentage()
            ? std::optional<float>(styledBoxDimension(childStyle, false, childStyle.height, childStyle.minHeight, 0.f, *contentHeight))
            : std::nullopt;
        Vec2 childSize;
        if (childElement) childSize = measure(*childElement, pass, childWidth, childHeight);
        else if (PseudoElement* pseudoElement = childRef.pseudoElement)
            childSize = measurePseudoElement(*pseudoElement, childStyle, childWidth, childHeight, pass);
        else if (Text* text = childRef.text())
            childSize = text->intrinsicSize(pass.styleSheet(), childStyle, pass.textMetrics(), {childWidth, childHeight});
        if (!isCurrent()) return std::nullopt;

        if (childElement && childElement->mRectExplicit) {
            if (childStyle.width.isAuto() && !childWidth) childSize.x = childElement->mRect.w;
            if (childStyle.height.isAuto() && !childHeight) childSize.y = childElement->mRect.h;
        }
        layouts.push_back({childRef, childStyle, childSize, childSize});
    }
    return layouts;
}

Vec2 LayoutEngine::measureGrid(Element& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                               std::optional<float> resolvedHeight, LayoutPass& pass) {
    Vec2 content = intrinsic;
    const std::optional<float> contentWidth = resolvedWidth ? std::optional<float>(contentBoxDimension(style, true, *resolvedWidth)) : std::nullopt;
    const std::optional<float> contentHeight =
        resolvedHeight ? std::optional<float>(contentBoxDimension(style, false, *resolvedHeight)) : std::nullopt;
    const std::optional<std::vector<ChildLayout>> layoutsResult = measureGridChildren(node, contentWidth, contentHeight, pass);
    if (!layoutsResult) return content;
    const layout_detail::GridTrackSizes tracks = gridTrackSizes(*layoutsResult, contentWidth, contentHeight);
    const auto total = [](const std::vector<float>& sizes) {
        float result = 0.f;
        for (const float size : sizes) result += size;
        return result;
    };
    content.x = std::max(content.x, total(tracks.columns));
    content.y = std::max(content.y, total(tracks.rows));
    return content;
}

Vec2 LayoutEngine::measureRow(Element& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                              std::optional<float> resolvedHeight, LayoutPass& pass) {
    const NodeSnapshot nodeState(node);
    Vec2 content;
    float rowWidth = intrinsic.x;
    float rowHeight = intrinsic.y;
    std::size_t rowChildren = 0;
    std::size_t rowLines = 0;
    std::vector<ChildLayout> rowLayouts;
    LayoutChildRef previousChild;
    const float fixedGap = style.gap.fixedPixels();
    const auto finishRow = [&] {
        if (!rowChildren && intrinsic.x == 0.f && intrinsic.y == 0.f) return;
        content.x = std::max(content.x, rowWidth);
        if (rowLines) content.y += fixedGap;
        content.y += rowHeight;
        ++rowLines;
        rowWidth = 0.f;
        rowHeight = 0.f;
        rowChildren = 0;
        previousChild = {};
    };

    const std::vector<LayoutChildRef> children = orderedChildren(node, pass);
    for (const LayoutChildRef& childRef : children) {
        if (!childRef || !childRef.attachedTo(node)) continue;
        if (isWhitespaceOnlyText(childRef)) continue;
        if (const Element* child = childRef.element(); child && child->elementName() == kBrTag.localName) continue;
        ChildLayout measured = measureChild(node, childRef, style, FlexDirection::Row, resolvedWidth, resolvedHeight, pass);
        Element* currentNode = nodeState.get();
        if (!nodeState.layoutValid()) return content;
        if (!measured.node || !measured.node.attachedTo(*currentNode) || !isDisplayed(measured)) continue;
        const float childOuterWidth = measured.measured.x + measured.style.margin.horizontal();
        const float outerHeight = measured.measured.y + measured.style.margin.vertical();
        rowLayouts.push_back(measured);
        if (flowBreakBefore(measured) && rowChildren) finishRow();
        if (previousChild && previousChild.attachedTo(*currentNode)) {
            const std::optional<AdjacentLayout> adjacent = adjacentLayout(nodeState, previousChild, measured.node, style);
            if (!adjacent) return content;
            if (adjacent->hasGap) rowWidth += fixedGap;
            rowWidth -= adjacent->overlap;
        }
        rowWidth += childOuterWidth;
        rowHeight = std::max(rowHeight, outerHeight);
        ++rowChildren;
        previousChild = measured.node;
    }

    if (rowChildren || !rowLines) finishRow();
    if (resolvedWidth && !rowLayouts.empty()) {
        const float availableMain = contentBoxDimension(style, true, *resolvedWidth);
        prepareMainAxis(rowLayouts, FlexDirection::Row, availableMain);
        const RowSizing sizing = allocateRowLines(node, rowLayouts, style, availableMain, pass);
        content.y = 0.f;
        for (std::size_t line = 0; line < sizing.lines.size(); ++line) {
            if (line) content.y += fixedGap;
            float height = line == 0 ? intrinsic.y : 0.f;
            const auto [begin, end] = sizing.lines[line];
            for (std::size_t index = begin; index < end; ++index)
                height = std::max(height, rowLayouts[index].measured.y + rowLayouts[index].style.margin.vertical());
            content.y += height;
        }
    }
    return content;
}

Vec2 LayoutEngine::measureColumn(Element& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                                 std::optional<float> resolvedHeight, LayoutPass& pass) {
    const NodeSnapshot nodeState(node);
    Vec2 content = intrinsic;
    LayoutChildRef previousChild;
    const float fixedGap = style.gap.fixedPixels();
    const std::vector<LayoutChildRef> children = orderedChildren(node, pass);
    for (const LayoutChildRef& childRef : children) {
        if (!childRef || !childRef.attachedTo(node)) continue;
        if (isWhitespaceOnlyText(childRef)) continue;
        if (const Element* child = childRef.element(); child && child->elementName() == kBrTag.localName) continue;
        ChildLayout measured = measureChild(node, childRef, style, FlexDirection::Column, resolvedWidth, resolvedHeight, pass);
        Element* currentNode = nodeState.get();
        if (!nodeState.layoutValid()) return content;
        if (!measured.node || !measured.node.attachedTo(*currentNode) || !isDisplayed(measured)) continue;
        const float childOuterWidth = measured.measured.x + measured.style.margin.horizontal();
        const float outerHeight = measured.measured.y + measured.style.margin.vertical();
        if (previousChild && previousChild.attachedTo(*currentNode)) {
            const std::optional<AdjacentLayout> adjacent = adjacentLayout(nodeState, previousChild, measured.node, style);
            if (!adjacent) return content;
            if (adjacent->hasGap) content.y += fixedGap;
            content.y -= adjacent->overlap;
        }
        content.x = std::max(content.x, childOuterWidth);
        content.y += outerHeight;
        previousChild = measured.node;
    }
    return content;
}

Vec2 LayoutEngine::measureNormal(Element& node, const Style& style, const Vec2& intrinsic, std::optional<float> resolvedWidth,
                                 std::optional<float> resolvedHeight, LayoutPass& pass) {
    Vec2 content = intrinsic;
    const std::optional<float> contentWidth = resolvedWidth ? std::optional<float>(contentBoxDimension(style, true, *resolvedWidth)) : std::nullopt;
    const std::optional<float> contentHeight =
        resolvedHeight ? std::optional<float>(contentBoxDimension(style, false, *resolvedHeight)) : std::nullopt;
    const std::optional<std::vector<ChildLayout>> layoutsResult = measureNormalChildren(node, contentWidth, contentHeight, pass);
    if (!layoutsResult) return content;
    const std::vector<ChildLayout>& layouts = *layoutsResult;

    const std::vector<layout_detail::NormalLine> lines = layout_detail::normalLines(layouts, contentWidth);
    for (const layout_detail::NormalLine& line : lines) {
        content.x = std::max(content.x, line.width);
        content.y += line.height;
    }
    for (const ChildLayout& child : layouts) {
        const Element* childNode = child.node.element();
        if (!child.node) continue;
        if (!childNode || !childNode->mRectExplicit) continue;
        content.x = std::max(content.x, childNode->mRect.x + child.measured.x + child.style.margin.horizontal());
        content.y = std::max(content.y, childNode->mRect.y + child.measured.y + child.style.margin.vertical());
    }
    return content;
}

bool LayoutEngine::remeasureRowChildren(Element& parent, std::vector<ChildLayout>& children, std::size_t begin, std::size_t end, LayoutPass& pass) {
    const NodeSnapshot parentState(parent);
    for (std::size_t index = begin; index < end; ++index) {
        ChildLayout& child = children[index];
        if (!child.style.height.isAuto() || child.style.aspectRatio) continue;
        Element* node = child.node.element();
        PseudoElement* pseudoElement = child.node.pseudoElement;
        if (!child.node.attachedTo(parent)) continue;
        const ElementRef<Element> nodeLifetime(node);
        const std::uint64_t nodeRevision = node ? node->mLayoutInvalidationRevision : 0;
        if (node) child.measured.y = measure(*node, pass, child.measured.x).y;
        else if (pseudoElement) child.measured.y = measurePseudoElement(*pseudoElement, child.style, child.measured.x, std::nullopt, pass).y;
        else if (Text* text = child.node.text())
            child.measured.y = text->intrinsicSize(pass.styleSheet(), child.style, pass.textMetrics(), {child.measured.x, std::nullopt}).y;
        Element* currentParent = parentState.get();
        node = nodeLifetime.get();
        if (!parentState.layoutValid()
            || !currentParent
            || !child.node.attachedTo(*currentParent)
            || (node && (node->mSurface != parentState.surface || node->mLayoutInvalidationRevision != nodeRevision)))
            return false;
        child.fitSize.y = child.measured.y;
    }
    return true;
}

bool LayoutEngine::remeasureColumnChildren(Element& parent, std::vector<ChildLayout>& children, const std::vector<Vec2>& initialSizes,
                                           LayoutPass& pass) {
    const NodeSnapshot parentState(parent);
    for (std::size_t index = 0; index < children.size(); ++index) {
        ChildLayout& child = children[index];
        if (std::abs(child.measured.x - initialSizes[index].x) <= 1.0e-4f && std::abs(child.measured.y - initialSizes[index].y) <= 1.0e-4f) continue;
        Element* node = child.node.element();
        PseudoElement* pseudoElement = child.node.pseudoElement;
        if (!child.node.attachedTo(parent)) continue;
        const ElementRef<Element> nodeLifetime(node);
        const std::uint64_t nodeRevision = node ? node->mLayoutInvalidationRevision : 0;
        const Vec2 constrained = node ? measure(*node, pass, child.measured.x, child.measured.y)
            : pseudoElement
            ? measurePseudoElement(*pseudoElement, child.style, child.measured.x, child.measured.y, pass)
            : child.node.text()->intrinsicSize(pass.styleSheet(), child.style, pass.textMetrics(), {child.measured.x, child.measured.y});
        Element* currentParent = parentState.get();
        node = nodeLifetime.get();
        if (!parentState.layoutValid()
            || !currentParent
            || !child.node.attachedTo(*currentParent)
            || (node && (node->mSurface != parentState.surface || node->mLayoutInvalidationRevision != nodeRevision)))
            return false;
        child.measured = constrained;
    }
    return true;
}

LayoutEngine::RowSizing LayoutEngine::allocateRowLines(Element& parent, std::vector<ChildLayout>& children, const Style& parentStyle,
                                                       float availableMain, LayoutPass& pass) {
    RowSizing sizing;
    sizing.lines = rowLines(children);
    sizing.allocations.reserve(sizing.lines.size());
    for (const auto& [begin, end] : sizing.lines) {
        const MainAxisAllocation allocation = allocateMainAxis(parent, children, begin, end, parentStyle, FlexDirection::Row, availableMain);
        sizing.allocations.push_back(allocation);
        if (!allocation.valid || !remeasureRowChildren(parent, children, begin, end, pass)) {
            sizing.valid = false;
            return sizing;
        }
    }
    return sizing;
}
} // namespace radia::ui
