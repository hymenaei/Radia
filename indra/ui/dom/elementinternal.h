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
#include <set>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>
#include <vector>
#include "dom/element.h"
#include "dom/text.h"

namespace radia::ui::detail {
using Node = radia::ui::Node;
struct DocumentIdentity {};

class ElementConstructionAccess {
public:
    template<typename ElementT, typename... Args> static std::unique_ptr<ElementT> create(Args&&... args) {
        return std::unique_ptr<ElementT>(new ElementT(std::forward<Args>(args)...));
    }

    template<typename ElementT, typename... Args> static ElementT createValue(Args&&... args) { return ElementT(std::forward<Args>(args)...); }
};

template<typename ElementT, typename... Args> std::unique_ptr<ElementT> makeElement(Args&&... args) {
    return ElementConstructionAccess::create<ElementT>(std::forward<Args>(args)...);
}

template<typename ElementT, typename... Args> ElementT makeElementValue(Args&&... args) {
    return ElementConstructionAccess::createValue<ElementT>(std::forward<Args>(args)...);
}

struct LayoutContextKey {
    const StyleSheet* styleSheet = nullptr;
    const TextMetrics* textMetrics = nullptr;
    std::uint64_t styleGeneration = 0;
    std::uint64_t textMetricsGeneration = 0;
    LayoutDirection direction = LayoutDirection::LeftToRight;
    ScrollbarMode scrollbarMode = ScrollbarMode::Classic;
    const NativeAppearance* nativeAppearance = nullptr;
    std::uint64_t nativeAppearanceRevision = 0;

    constexpr bool operator==(const LayoutContextKey& other) const {
        return styleSheet == other.styleSheet
            && textMetrics == other.textMetrics
            && styleGeneration == other.styleGeneration
            && textMetricsGeneration == other.textMetricsGeneration
            && direction == other.direction
            && scrollbarMode == other.scrollbarMode
            && nativeAppearance == other.nativeAppearance
            && nativeAppearanceRevision == other.nativeAppearanceRevision;
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
    MountEpoch mountEpoch;
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
    static MountEpoch& mountEpoch(Element& element) { return element.mPrivate->mountEpoch; }
    static const MountEpoch& mountEpoch(const Element& element) { return element.mPrivate->mountEpoch; }
    static bool isMounted(const Element& element) { return element.mSurface != nullptr; }
    static ElementLayoutCache& layoutCache(Element& element) { return element.mPrivate->layoutCache; }
    static const ElementLayoutCache& layoutCache(const Element& element) { return element.mPrivate->layoutCache; }
    static const Rect& scrollableOverflow(const Element& element) { return element.mScrollableOverflow; }
    static const Rect& scrollport(const Element& element) { return element.mScrollport; }
    static std::map<std::string, std::string>& styleAttributes(Element& element) { return element.mStyleAttributes; }
    static const std::map<std::string, std::string>& styleAttributes(const Element& element) { return element.mStyleAttributes; }
    static void setStyleAttribute(Element& element, std::string name, std::string value) {
        element.mStyleAttributes[std::move(name)] = std::move(value);
        element.invalidateStyleTree(true, true);
    }
    static void removeStyleAttribute(Element& element, std::string_view name) {
        element.mStyleAttributes.erase(std::string(name));
        element.invalidateStyleTree(true, true);
    }
    static void setIdScopeRoot(Element& element) { element.setIdScopeRoot(true); }
    static void setState(Element& element, ElementState state, bool enabled) { element.setState(state, enabled); }
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
struct ElementIdIndex {
    std::map<std::string, Element*> first;
    std::set<std::string> ambiguous;

    void add(Element& element) {
        if (element.id().empty()) return;
        if (!first.emplace(element.id(), &element).second) ambiguous.emplace(element.id());
    }
};
void indexElementsInScope(Element& element, ElementIdIndex& index);
Node& appendText(Element& parent, std::string text);
Node& appendLocalizedText(Element& parent, LocalizedText text, std::string html);
NodeChildren nodes(Element& element);
ConstNodeChildren nodes(const Element& element);

template<typename ElementT> class ElementVisit;
} // namespace radia::ui::detail

namespace radia::ui {
using LayoutContextKey = detail::LayoutContextKey;

template<typename ElementT> class ElementRef {
    using ElementInternalAccess = detail::ElementInternalAccess;
    using Lifetime = std::weak_ptr<char>;
    using MountEpoch = detail::MountEpoch;

public:
    ElementRef() = default;
    ElementRef(ElementT* element) { set(element); }

    ElementT* get() const { return mLifetime.expired() ? nullptr : mElement; }
    ElementT* getMounted() const {
        ElementT* element = get();
        return element && ElementInternalAccess::isMounted(*element) && ElementInternalAccess::mountEpoch(*element) == mMountEpoch ? element
                                                                                                                                   : nullptr;
    }
    ElementT* operator->() const { return get(); }
    ElementT& operator*() const { return *get(); }
    explicit operator bool() const { return get() != nullptr; }

    void set(ElementT* element) {
        mElement = element;
        mLifetime = element ? ElementInternalAccess::lifetime(*element) : Lifetime{};
        mMountEpoch = element ? ElementInternalAccess::mountEpoch(*element) : MountEpoch{};
    }

private:
    ElementT* mElement = nullptr;
    Lifetime mLifetime;
    MountEpoch mMountEpoch;
};

namespace detail {
template<typename ElementT> class ElementVisit {
public:
    static_assert(std::is_same_v<ElementT, radia::ui::Element> || std::is_same_v<ElementT, const radia::ui::Element>);

    ElementVisit() = default;
    explicit ElementVisit(ElementT& element)
        : lifetime(&element), parentLifetime(element.parentElement()), surface(element.mSurface), parentNode(element.parentNode()),
          parent(element.parentElement()), layoutRevision(element.mLayoutInvalidationRevision), childTopologyRevision(element.mChildTopologyRevision),
          parentChildTopologyRevision(parent ? parent->mChildTopologyRevision : 0), styleRevision(element.mStyleRevision),
          mountEpoch(ElementInternalAccess::mountEpoch(element)) {}

    ElementT* get() const { return lifetime.get(); }
    bool objectAlive() const { return get() != nullptr; }
    bool mountValid() const {
        const ElementT* element = get();
        return element && element->mSurface == surface && ElementInternalAccess::mountEpoch(*element) == mountEpoch;
    }
    bool topologyValid() const {
        const ElementT* element = get();
        const Element* currentParent = parentLifetime.get();
        return element
            && currentParent == parent
            && element->parentNode() == parentNode
            && element->parentElement() == parent
            && element->mChildTopologyRevision == childTopologyRevision
            && (!parent || currentParent->mChildTopologyRevision == parentChildTopologyRevision);
    }
    bool layoutValid() const {
        const ElementT* element = get();
        return mountValid() && topologyValid() && element->mLayoutInvalidationRevision == layoutRevision;
    }
    bool styleValid() const {
        const ElementT* element = get();
        return mountValid() && topologyValid() && element->mStyleRevision == styleRevision;
    }
    bool attachedTo(const Element& expectedParent) const {
        const ElementT* element = get();
        return element && element->parentElement() == &expectedParent;
    }

    ElementRef<ElementT> lifetime;
    ElementRef<const Element> parentLifetime;
    const Surface* surface = nullptr;
    const Node* parentNode = nullptr;
    const Element* parent = nullptr;
    std::uint64_t layoutRevision = 0;
    std::uint64_t childTopologyRevision = 0;
    std::uint64_t parentChildTopologyRevision = 0;
    std::uint64_t styleRevision = 0;
    MountEpoch mountEpoch;
};
} // namespace detail

using ElementVisit = detail::ElementVisit<Element>;
using ConstElementVisit = detail::ElementVisit<const Element>;
} // namespace radia::ui
