/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "style/stylepass.h"
#include <algorithm>
#include <utility>
#include "elements/element.h"
#include "style/stylesheet.h"
#include "text/metrics.h"

namespace radia::ui {
namespace {
LayoutContextKey makeContextKey(const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction) {
    return {&styleSheet, &textMetrics, styleSheet.generation(), textMetrics.generation(), direction};
}
} // namespace

StylePass::StylePass(const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction)
    : mStyleSheet(styleSheet), mTextMetrics(textMetrics), mDirection(direction), mContext(makeContextKey(styleSheet, textMetrics, direction)) {}

bool StylePass::matches(const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction) const {
    return mContext == makeContextKey(styleSheet, textMetrics, direction);
}

void StylePass::beginTraversal() {
    mTree.beginTraversal();
    if (mTraversalDepth++ != 0) return;
    if (mResetStorageAtBoundary) {
        mStyles.clear();
        mStyleStorage.clear();
        mInvalidated = false;
        mResetStorageAtBoundary = false;
    } else {
        compactStyles();
    }
}

void StylePass::compactStyles() {
    constexpr std::size_t kStorageSlack = 32;
    if (mStyleStorage.size() <= mStyles.size() * 2 + kStorageSlack) return;

    std::deque<Style> compacted;
    std::unordered_map<const Element*, CachedStyle> styles;
    styles.reserve(mStyles.size());
    for (const auto& [element, cached] : mStyles) {
        compacted.push_back(mStyleStorage[cached.storageIndex]);
        styles.emplace(element, CachedStyle{compacted.size() - 1, cached.lifetime, cached.contextRevision});
    }
    mStyleStorage.swap(compacted);
    mStyles.swap(styles);
}

void StylePass::endTraversal() {
    llassert(mTraversalDepth != 0);
    if (mTraversalDepth) --mTraversalDepth;
    mTree.endTraversal();
}

const Style& StylePass::style(const Element& element) {
    if (mInvalidated) {
        mStyles.clear();
        mInvalidated = false;
    }
    const auto found = mStyles.find(&element);
    const auto lifetime = detail::NodeAccess::lifetime(element).lock();
    if (found != mStyles.end() && found->second.lifetime.lock() == lifetime && found->second.contextRevision == element.styleContextRevision())
        return mStyleStorage[found->second.storageIndex];
    if (found != mStyles.end()) mStyles.erase(found);

    const ConstElementVisit elementSnapshot(element);
    const std::weak_ptr<char> elementLifetime = detail::NodeAccess::lifetime(element);
    const ElementRef<const Element> styledRef(&element);
    const Element* parent = elementSnapshot.parent;
    const std::uint64_t contextRevision = element.styleContextRevision();
    const Element* owner = &element;
    Style resolved;
    if (element.part().empty()) resolved = mStyleSheet.resolveElement(element, mDirection);
    else {
        for (const Element* candidate = element.parentElement(); candidate; candidate = candidate->parentElement()) {
            if (candidate->styleElement() != element.styleElement()) continue;
            owner = candidate;
            if (candidate->part().empty()) break;
        }
        resolved = mStyleSheet.resolveElementPart(*owner, element, mDirection);
    }
    element.constrainResolvedStyle(resolved);
    const Element* current = styledRef.get();
    const auto transient = [&]() -> const Style& {
        mStyleStorage.emplace_back(std::move(resolved));
        return mStyleStorage.back();
    };
    if (!current || !elementSnapshot.styleValid()) return transient();
    if (parent) {
        const ConstElementVisit parentSnapshot(*parent);
        const Style& parentStyle = style(*parent);
        current = styledRef.get();
        const Element* currentParent = parentSnapshot.get();
        if (!current || !currentParent || !elementSnapshot.styleValid() || !parentSnapshot.styleValid()) return transient();
        inheritStyle(resolved, parentStyle);
    }

    current = styledRef.get();
    if (!current || !elementSnapshot.styleValid()) return transient();
    const std::uint64_t finalContextRevision = current->styleContextRevision();
    mStyleStorage.emplace_back(std::move(resolved));
    const std::size_t storageIndex = mStyleStorage.size() - 1;
    mStyles[&element] = CachedStyle{storageIndex, elementLifetime, finalContextRevision == contextRevision ? contextRevision : finalContextRevision};
    return mStyleStorage[storageIndex];
}

StylePass::ChildSnapshot StylePass::orderedChildren(Element& parent) {
    return mTree.orderedChildren(parent, [this](const Element& node) -> const Style& { return style(node); });
}

StylePass::ChildSnapshot StylePass::sourceChildren(Element& parent) {
    return mTree.sourceChildren(parent);
}
} // namespace radia::ui
