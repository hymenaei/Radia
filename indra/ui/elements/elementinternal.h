/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <functional>
#include <iterator>
#include <map>
#include <memory>
#include <string>
#include <string_view>
#include <type_traits>
#include <vector>
#include "elements/element.h"
#include "elements/elementtext.h"

namespace radia::ui::detail {
using Node = radia::ui::Node;
struct DocumentIdentity {};

std::unique_ptr<Element> createRuntimeElement(std::string_view elementName);

struct LayoutContextKey {
    const StyleSheet* styleSheet = nullptr;
    const TextMetrics* textMetrics = nullptr;
    std::uint64_t styleGeneration = 0;
    std::uint64_t textMetricsGeneration = 0;
    LayoutDirection direction = LayoutDirection::LeftToRight;
    ScrollbarMode scrollbarMode = ScrollbarMode::Classic;
    float scrollbarThickness = 15.f;

    constexpr bool operator==(const LayoutContextKey& other) const {
        return styleSheet == other.styleSheet
            && textMetrics == other.textMetrics
            && styleGeneration == other.styleGeneration
            && textMetricsGeneration == other.textMetricsGeneration
            && direction == other.direction
            && scrollbarMode == other.scrollbarMode
            && scrollbarThickness == other.scrollbarThickness;
    }
};

struct ElementLayoutCache {
    Vec2 measuredSize;
    Vec2 intrinsicSize;
    float measuredWidth = 0.f;
    float measuredHeight = 0.f;
    LayoutContextKey layoutContext;
    bool measuredWidthSet = false;
    bool measuredHeightSet = false;
    bool measuredRectExplicit = false;
    bool measuredRectConstraintSet = false;
    float measuredRectWidth = 0.f;
    float measuredRectHeight = 0.f;
    bool measureValid = false;
    bool intrinsicValid = false;
    bool arrangeValid = false;
};

struct ElementPrivateData {
    std::shared_ptr<char> lifetime = std::make_shared<char>(0);
    std::shared_ptr<char> mountLifetime;
    ElementLayoutCache layoutCache;
};

class NodeAccess {
public:
    static std::weak_ptr<char> lifetime(const Node& node) { return node.mLifetime; }
    static const std::shared_ptr<DocumentIdentity>& documentIdentity(const Node& node) { return node.mDocumentIdentity; }
    static void setDocumentIdentity(Node& node, std::shared_ptr<DocumentIdentity> identity) { node.mDocumentIdentity = std::move(identity); }
    static void setParent(Node& node, Node* parent) {
        node.mParentNode = parent;
        node.mParent = parent ? parent->asElement() : nullptr;
    }
    static bool flowBreakBefore(const Node& node) { return node.mFlowBreakBefore; }
    static void setFlowBreakBefore(Node& node, bool enabled) { node.mFlowBreakBefore = enabled; }
};

class ElementInternalAccess {
public:
    using NodeOwners = std::vector<std::unique_ptr<Node>>;

    static std::weak_ptr<char> lifetime(const Element& element) { return element.mPrivate->lifetime; }
    static std::shared_ptr<char>& mountLifetime(Element& element) { return element.mPrivate->mountLifetime; }
    static const std::shared_ptr<char>& mountLifetime(const Element& element) { return element.mPrivate->mountLifetime; }
    static ElementLayoutCache& layoutCache(Element& element) { return element.mPrivate->layoutCache; }
    static const ElementLayoutCache& layoutCache(const Element& element) { return element.mPrivate->layoutCache; }
    static const Rect& scrollableOverflow(const Element& element) { return element.mScrollableOverflow; }
    static const Rect& scrollport(const Element& element) { return element.mScrollport; }
    static NodeOwners& childOwners(Element& element) { return element.mChildren; }
    static const NodeOwners& childOwners(const Element& element) { return element.mChildren; }
    static std::map<std::string, std::string>& styleAttributes(Element& element) { return element.mStyleAttributes; }
    static const std::map<std::string, std::string>& styleAttributes(const Element& element) { return element.mStyleAttributes; }
};

class NodeRef {
public:
    NodeRef() = default;
    NodeRef(Node* node) { set(node); }

    Node* get() const noexcept { return mLifetime.expired() ? nullptr : mNode; }
    Element* element() const noexcept {
        Node* node = get();
        return node ? node->asElement() : nullptr;
    }
    Text* text() const noexcept {
        Node* node = get();
        return node ? node->asText() : nullptr;
    }
    explicit operator bool() const noexcept { return get() != nullptr; }

    void set(Node* node) {
        mNode = node;
        mLifetime = node ? NodeAccess::lifetime(*node) : std::weak_ptr<char>();
    }

private:
    Node* mNode = nullptr;
    std::weak_ptr<char> mLifetime;
};

class NodeChildren {
public:
    class Iterator {
    public:
        using iterator_category = std::forward_iterator_tag;
        using value_type = Node;
        using difference_type = std::ptrdiff_t;
        using pointer = Node*;
        using reference = Node&;

        Iterator() = default;
        explicit Iterator(const ElementInternalAccess::NodeOwners* nodes, std::size_t index) : mNodes(nodes), mIndex(index) {}

        Node& operator*() const { return *(*mNodes)[mIndex]; }
        Node* operator->() const { return (*mNodes)[mIndex].get(); }
        Iterator& operator++() {
            ++mIndex;
            return *this;
        }
        friend bool operator==(const Iterator& left, const Iterator& right) { return left.mNodes == right.mNodes && left.mIndex == right.mIndex; }
        friend bool operator!=(const Iterator& left, const Iterator& right) { return !(left == right); }

    private:
        const ElementInternalAccess::NodeOwners* mNodes = nullptr;
        std::size_t mIndex = 0;
    };

    Iterator begin() const { return Iterator(mNodes, 0); }
    Iterator end() const { return Iterator(mNodes, mNodes ? mNodes->size() : 0); }
    std::size_t size() const { return mNodes ? mNodes->size() : 0; }
    bool empty() const { return begin() == end(); }

private:
    explicit NodeChildren(const ElementInternalAccess::NodeOwners* nodes) : mNodes(nodes) {}

    friend NodeChildren nodes(Element& element);
    const ElementInternalAccess::NodeOwners* mNodes = nullptr;
};

class ConstNodeChildren {
public:
    class Iterator {
    public:
        using iterator_category = std::forward_iterator_tag;
        using value_type = Node;
        using difference_type = std::ptrdiff_t;
        using pointer = const Node*;
        using reference = const Node&;

        Iterator() = default;
        explicit Iterator(const ElementInternalAccess::NodeOwners* nodes, std::size_t index) : mNodes(nodes), mIndex(index) {}

        const Node& operator*() const { return *(*mNodes)[mIndex]; }
        const Node* operator->() const { return (*mNodes)[mIndex].get(); }
        Iterator& operator++() {
            ++mIndex;
            return *this;
        }
        friend bool operator==(const Iterator& left, const Iterator& right) { return left.mNodes == right.mNodes && left.mIndex == right.mIndex; }
        friend bool operator!=(const Iterator& left, const Iterator& right) { return !(left == right); }

    private:
        const ElementInternalAccess::NodeOwners* mNodes = nullptr;
        std::size_t mIndex = 0;
    };

    Iterator begin() const { return Iterator(mNodes, 0); }
    Iterator end() const { return Iterator(mNodes, mNodes ? mNodes->size() : 0); }
    std::size_t size() const { return mNodes ? mNodes->size() : 0; }
    bool empty() const { return begin() == end(); }

private:
    explicit ConstNodeChildren(const ElementInternalAccess::NodeOwners* nodes) : mNodes(nodes) {}

    friend ConstNodeChildren nodes(const Element& element);
    const ElementInternalAccess::NodeOwners* mNodes = nullptr;
};

const std::string* styleAttribute(const Element& element, std::string_view name);
Element* findElementInScope(Element& element, std::string_view id);
const Element* findElementInScope(const Element& element, std::string_view id);
void indexElementsInScope(Element& element, std::map<std::string, Element*>& index);
Node& appendText(Element& parent, std::string text);
Node& appendLocalizedText(Element& parent, LocalizedText text, std::string markup);
NodeChildren nodes(Element& element);
ConstNodeChildren nodes(const Element& element);

template<typename ElementT> class ElementVisit;
} // namespace radia::ui::detail

namespace radia::ui {
using LayoutContextKey = detail::LayoutContextKey;

template<typename ElementT> class ElementRef {
public:
    ElementRef() = default;
    ElementRef(ElementT* element) { set(element); }

    ElementT* get() const { return mLifetime.expired() ? nullptr : mElement; }
    ElementT* getMounted() const {
        ElementT* element = get();
        return element && (!mHasMountLifetime || !mMountLifetime.expired()) ? element : nullptr;
    }
    ElementT* operator->() const { return get(); }
    ElementT& operator*() const { return *get(); }
    explicit operator bool() const { return get() != nullptr; }

    void set(ElementT* element) {
        mElement = element;
        mLifetime = element ? detail::ElementInternalAccess::lifetime(*element) : std::weak_ptr<char>();
        mMountLifetime = element ? detail::ElementInternalAccess::mountLifetime(*element) : std::weak_ptr<char>();
        mHasMountLifetime = element && detail::ElementInternalAccess::mountLifetime(*element) != nullptr;
    }

private:
    ElementT* mElement = nullptr;
    std::weak_ptr<char> mLifetime;
    std::weak_ptr<char> mMountLifetime;
    bool mHasMountLifetime = false;
};

namespace detail {
template<typename ElementT> class ElementVisit {
public:
    static_assert(std::is_same_v<ElementT, radia::ui::Element> || std::is_same_v<ElementT, const radia::ui::Element>);

    ElementVisit() = default;
    explicit ElementVisit(ElementT& element)
        : lifetime(&element), surface(element.mSurface), parent(element.parentElement()), layoutRevision(element.mLayoutInvalidationRevision),
          styleRevision(element.mStyleRevision) {}

    ElementT* get() const { return lifetime.get(); }
    bool valid() const {
        const ElementT* element = lifetime.get();
        return element && element->mSurface == surface && element->parentElement() == parent && element->mLayoutInvalidationRevision == layoutRevision;
    }
    bool validChildOf(const Element& expectedParent) const {
        const ElementT* element = lifetime.get();
        return valid() && element->parentElement() == &expectedParent;
    }
    bool styleValid() const {
        const ElementT* element = lifetime.get();
        return valid() && element->mStyleRevision == styleRevision;
    }

    ElementRef<ElementT> lifetime;
    const Surface* surface = nullptr;
    const Element* parent = nullptr;
    std::uint64_t layoutRevision = 0;
    std::uint64_t styleRevision = 0;
};
} // namespace detail

using ElementVisit = detail::ElementVisit<Element>;
using ConstElementVisit = detail::ElementVisit<const Element>;
} // namespace radia::ui
