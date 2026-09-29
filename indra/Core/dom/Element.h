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
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <type_traits>
#include <utility>
#include <vector>
#include "CSSPseudoSelectors.h"
#include "ComputedStyle.h"
#include "Event.h"
#include "IntrinsicSizeConstraints.h"
#include "LayoutGeometry.h"
#include "LocalizedText.h"
#include "NativeAppearance.h"
#include "Node.h"

namespace Core {
enum class LayoutInvalidationReason : uint8_t {
    NoInvalidation = 0,
    Measure = 1 << 0,
    Arrange = 1 << 1,
    ComputedStyle = 1 << 2,
    Text = 1 << 3,
    Paint = 1 << 4
};

class InvalidationFlags {
public:
    constexpr InvalidationFlags() = default;
    constexpr InvalidationFlags(LayoutInvalidationReason reason)
        : mBits(static_cast<uint8_t>(reason)) {}
    constexpr explicit InvalidationFlags(uint8_t bits)
        : mBits(bits) {}

    constexpr bool intersects(InvalidationFlags other) const { return (mBits & other.mBits) != 0; }
    constexpr bool contains(LayoutInvalidationReason reason) const { return (mBits & static_cast<uint8_t>(reason)) != 0; }
    constexpr uint8_t value() const { return mBits; }

    constexpr void add(InvalidationFlags other) { mBits = static_cast<uint8_t>(mBits | other.mBits); }
    constexpr void remove(InvalidationFlags other) { mBits = static_cast<uint8_t>(mBits & static_cast<uint8_t>(~other.mBits)); }

private:
    uint8_t mBits = 0;
};

inline constexpr InvalidationFlags operator|(LayoutInvalidationReason left, LayoutInvalidationReason right) {
    return InvalidationFlags(static_cast<uint8_t>(left) | static_cast<uint8_t>(right));
}

inline constexpr InvalidationFlags operator|(InvalidationFlags left, LayoutInvalidationReason right) {
    return InvalidationFlags(static_cast<uint8_t>(left.value() | static_cast<uint8_t>(right)));
}

inline constexpr InvalidationFlags operator|(LayoutInvalidationReason left, InvalidationFlags right) { return right | left; }

inline constexpr InvalidationFlags kMeasureInvalidationReasons =
    LayoutInvalidationReason::Measure | LayoutInvalidationReason::ComputedStyle | LayoutInvalidationReason::Text;
inline constexpr InvalidationFlags kArrangeInvalidationReasons = kMeasureInvalidationReasons | LayoutInvalidationReason::Arrange;
inline constexpr InvalidationFlags kTextInvalidationReasons =
    LayoutInvalidationReason::Measure | LayoutInvalidationReason::Arrange | LayoutInvalidationReason::Text;
inline constexpr InvalidationFlags kPaintStyleInvalidationReasons = InvalidationFlags(LayoutInvalidationReason::Paint);
inline constexpr InvalidationFlags kLayoutStyleInvalidationReasons = kArrangeInvalidationReasons | LayoutInvalidationReason::Paint;

class Element;
namespace Style {
class PseudoElement;
}

enum class AccessibleRole : uint8_t {
    Generic,
    Button,
    TextInput,
    Checkbox,
    Radio,
    Switch,
    Label
};

struct AccessibleSemantics {
    AccessibleRole role = AccessibleRole::Generic;
    std::string name;
    std::optional<bool> checked;
    const Element* labelTarget = nullptr;
    bool indeterminate = false;
    bool focusable = false;
    bool disabled = false;
    bool focused = false;
    bool focusVisible = false;
};

namespace detail {
struct ElementPrivateData;
class EventHandlerCallStore;
class ElementInternalAccess;
class ElementConstructionAccess;

struct MountEpoch {
    std::uint64_t value = 0;

    friend constexpr bool operator==(MountEpoch left, MountEpoch right) { return left.value == right.value; }
    friend constexpr bool operator!=(MountEpoch left, MountEpoch right) { return !(left == right); }
};
} // namespace detail

namespace detail {
template<typename ElementT> class ElementVisit;
class NodeChildren;
class ConstNodeChildren;
Node& appendText(Element& parent, std::string text);
Node& appendLocalizedText(Element& parent, LocalizedText text, std::string html);
Element* findElementInTree(Element& element, std::string_view id);
const Element* findElementInTree(const Element& element, std::string_view id);
NodeChildren nodes(Element& element);
ConstNodeChildren nodes(const Element& element);
} // namespace detail

class PaintContext;
class System;
class Surface;
class Binding;
class TextMeasurer;
namespace CSS {
class StyleSheet;
}
namespace Layout {
class Engine;
class Pass;
class TreeTraversalCache;
} // namespace Layout
namespace Style {
class Pass;
}

struct ScrollMetrics {
    float scrollWidth = 0.f;
    float scrollHeight = 0.f;
    float clientWidth = 0.f;
    float clientHeight = 0.f;
    float maxScrollLeft = 0.f;
    float maxScrollTop = 0.f;
};

namespace Layout::detail {
class ElementLayoutAccess;
struct ChildLayout;
Layout::Rect positionedRect(const ChildLayout& child, const Layout::Rect& parent);
void setArrangedRect(Element& node, const Layout::Rect& rect);
} // namespace Layout::detail

class Element : public Node {
    template<typename> friend class detail::ElementVisit;
    friend class detail::ElementInternalAccess;
    friend class Node;
    friend class Text;
    friend class Document;
    friend class Layout::TreeTraversalCache;
    friend class Binder;
    friend class Binding;
    friend class Layout::Pass;
    friend class Layout::Engine;
    friend class Style::Pass;
    friend class Surface;
    friend Layout::Rect Layout::detail::positionedRect(const Layout::detail::ChildLayout&, const Layout::Rect&);
    friend void Layout::detail::setArrangedRect(Element&, const Layout::Rect&);
    friend class Layout::detail::ElementLayoutAccess;
    friend class detail::ElementConstructionAccess;
    friend class detail::NodeMutation;
    friend class detail::EventHandlerCallStore;
    friend Node& detail::appendText(Element&, std::string);
    friend Node& detail::appendLocalizedText(Element&, LocalizedText, std::string);
    friend detail::NodeChildren detail::nodes(Element&);
    friend detail::ConstNodeChildren detail::nodes(const Element&);

public:
    class ClassList;
    class ConstClassList;

    struct Attribute {
        std::string name;
        std::optional<std::string> value;
    };
    using AttributeList = std::vector<Attribute>;

    virtual ~Element();

    Element* asElement() noexcept override { return this; }
    const Element* asElement() const noexcept override { return this; }

    Node* firstChild() noexcept override;
    const Node* firstChild() const noexcept override;
    Node* lastChild() noexcept override;
    const Node* lastChild() const noexcept override;
    NodeSnapshot childNodes() override;
    ConstNodeSnapshot childNodes() const override;
    Element& setId(std::string id);
    ClassList classList();
    ConstClassList classList() const;
    const AttributeList& attributes() const noexcept { return mAttributes; }
    void setAttribute(std::string name, std::optional<std::string> value = std::nullopt);
    bool hasAttribute(std::string_view name) const;
    const Attribute* attribute(std::string_view name) const;
    void removeAttribute(std::string_view name);
    Element& setRect(const Layout::Rect& rect);
    Element& setPointerEvents(bool pointerEvents);
    Element& disabled(bool disabled);
    Element& setVisibility(Style::Visibility visibility);
    Element& setHidden(bool hidden);
    Node* append(NodePtr child);
    Node* append(FragmentPtr fragment);
    Node* prepend(NodePtr child);
    Node* prepend(FragmentPtr fragment);
    void replaceChildren();
    void replaceChildren(FragmentPtr fragment);
    Node* replaceChildren(NodePtr child);
    std::string innerHTML() const;
    Element& innerHTML(std::string html);
    Element& innerHTML(LocalizedText text);
    virtual std::string textContent() const;
    Element& textContent(std::string text);
    Element& textContent(LocalizedText text);
    Element& setOnActivate(std::function<void(Element&)> callback);
    void addEventListener(std::string_view type, EventHandler handler, bool capture = false);
    void removeEventListener(std::string_view type, const EventHandler& handler, bool capture = false);

    const std::string& elementName() const { return mElementName; }
    const std::string& id() const { return mId; }
    const std::set<std::string>& classes() const { return mClasses; }
    const Layout::Rect& rect() const { return mRect; }
    const Layout::Vec2& desiredSize() const { return mDesiredSize; }
    const ScrollMetrics& scrollMetrics() const noexcept { return mScrollMetrics; }
    float scrollLeft() const noexcept { return mScrollPosition.inlineOffset; }
    float scrollTop() const noexcept { return mScrollPosition.blockOffset; }
    float scrollWidth() const noexcept { return mScrollMetrics.scrollWidth; }
    float scrollHeight() const noexcept { return mScrollMetrics.scrollHeight; }
    float clientWidth() const noexcept { return mScrollMetrics.clientWidth; }
    float clientHeight() const noexcept { return mScrollMetrics.clientHeight; }
    void scrollTo(float left, float top);
    void scrollBy(float deltaLeft, float deltaTop);
    ElementList children();
    ConstElementList children() const;
    std::uint64_t styleContextRevision() const;
    bool pointerEvents() const { return mPointerEvents.value_or(defaultPointerEvents()); }
    Style::Visibility visibility() const { return mVisibilityOverride.value_or(Style::Visibility::Visible); }
    bool isDisplayed(const Style::ComputedStyle& style) const;
    bool isVisible(const Style::ComputedStyle& style) const;
    bool disabled() const;
    bool hovered() const { return mHovered; }
    bool active() const { return mActive; }
    bool focused() const { return mFocused; }
    bool focusVisible() const { return mFocusVisible; }
    bool idScopeRoot() const { return mIdScopeRoot; }
    Layout::Direction directionality() const;
    bool flowBreakBefore() const;

    void activate();
    void activateFromLabel();

    virtual Layout::Vec2 intrinsicSize(const CSS::StyleSheet& styleSheet, const Style::ComputedStyle& style,
        const TextMeasurer& textMetrics, const Layout::IntrinsicSizeConstraints& constraints = Layout::IntrinsicSizeConstraints()) const;
    virtual bool defaultPointerEvents() const { return false; }
    virtual bool focusable() const { return false; }
    virtual AccessibleSemantics accessibleSemantics() const;
    virtual void paint(PaintContext& context, const Style::ComputedStyle& style, float scale) const;

protected:
    explicit Element(std::string_view elementName);

    virtual bool defaultKeyDown(const KeyEvent& event);
    virtual bool defaultKeyUp(const KeyEvent& event);
    virtual bool defaultCharacterInput(unsigned int codepoint);
    virtual bool defaultScroll(const WheelEvent& event);
    virtual bool beginPointerInteraction(const PointerEvent& event);
    virtual bool updatePointerInteraction(const PointerEvent& event);
    virtual bool endPointerInteraction(const PointerEvent& event);
    virtual void onAttributeSet(std::string_view, const std::optional<std::string>&) {}
    virtual void onAttributeRemoved(std::string_view) {}
    virtual void constrainResolvedStyle(Style::ComputedStyle& style) const {}
    void dispatchEvent(Event& event);
    void translate(const Layout::Vec2& delta);
    void invalidateMeasure();
    void invalidateArrange();
    void invalidateText();
    void invalidatePaint();
    const CSS::StyleSheet* styleSheet() const;
    const System* system() const;
    Surface* surface() const { return mSurface && !mSurfaceLifetime.expired() ? mSurface : nullptr; }
    const TextMeasurer& textMetrics() const;
    virtual void onActivate() {}
    virtual void onLabelActivate() { activate(); }
    virtual void onTreeWillBeDetached() {}
    virtual void onTreeAttached() {}
    virtual void onTreeDetached() {}
    virtual void onChildWillBeRemoved(Element&) {}
    virtual void onChildAdded(Element&) {}
    virtual void onChildRemoved(Element&) {}
    virtual void onDescendantAdded(Element&) {}
    virtual void onDescendantWillBeRemoved(Element&) {}
    virtual void onDescendantRemoved(Element&) {}
    virtual void onChildrenCleared() {}
    virtual void onLocaleChanged(const System&);
    virtual void onArranged(const Style::ComputedStyle&) {}
    virtual Layout::Rect paintBounds() const { return mRect; }
    virtual bool hasLayoutGapBetween(const Element&, const Element&) const { return true; }
    virtual float layoutOverlapBetween(const Element&, const Element&, const Style::ComputedStyle&) const { return 0.f; }
    virtual std::vector<Style::PseudoElement*> generatedPseudoElements() const { return {}; }
    void translateChild(Element& child, const Layout::Vec2& delta);
    bool setPseudoClassMatch(CSS::PseudoClass pseudoClass, bool& ownedValue, bool matches);
    void invalidatePseudoClass(CSS::PseudoClass pseudoClass);
    Element& setDisplayNone(bool displayNone);

private:
    enum class LocalizedContentMode {
        Literal,
        HTML
    };

    struct LocalizedContent {
        LocalizedText text;
        LocalizedContentMode mode;
    };

    Node* insertBefore(NodePtr child, Node* reference);
    Node* insertBefore(FragmentPtr fragment, Node* reference);
    NodePtr replaceNode(Node& child, NodePtr replacement);
    NodePtr replaceNode(Node& child, FragmentPtr replacement);
    Node* replaceRange(Node& first, Node& last, FragmentPtr replacement);
    NodePtr removeNode(Node& child);

    struct EventListener {
        struct State {
            bool removed = false;
        };

        std::string type;
        EventHandler handler;
        bool capture = false;
        std::shared_ptr<State> state;
    };

    using EventListenerSnapshot = std::vector<EventListener>;

    void dispatchListeners(Event& event, bool capture, const std::function<bool()>& isValid = {});
    void translateSubtree(const Layout::Vec2& delta);
    void invalidateArrangeTree();
    void invalidateTextTree();
    void invalidateStyleTree(bool layoutAffecting = true, bool propagateToDescendants = true);
    void invalidateStyleTreesFrom(Node* firstChild, bool layoutAffecting, bool propagateToDescendants);
    void invalidateDirectionalityAncestors();
    void invalidateFollowingSiblingStyleTrees(bool layoutAffecting, bool propagateToDescendants);
    void clearPaintInvalidationTree();
    void notifyTreeAttached();
    void notifyTreeWillBeDetached();
    void notifyTreeDetached();
    void setSurface(Surface* surface);
    void setHovered(bool hovered);
    void setActive(bool active);
    void setFocused(bool focused);
    void setFocusVisible(bool focusVisible);
    void setAttributeValue(std::string name, std::optional<std::string> value);
    void removeAttributeValue(std::string_view name);
    void rebuildTextContent();
    void rebuildResolvedHTML(std::string html);
    void replaceResolvedHTML(std::string html);
    bool refreshTextContentSlots();
    Element& setIdScopeRoot(bool scopeRoot);
    void setScrollMetrics(const ScrollMetrics& metrics, const Layout::Rect& scrollableOverflow, const Layout::Rect& scrollport);

    struct ScrollPosition {
        float inlineOffset = 0.f;
        float blockOffset = 0.f;
    };

    std::string mElementName;
    AttributeList mAttributes;
    std::string mId;
    std::set<std::string> mClasses;
    std::vector<std::string> mClassOrder;
    Layout::Rect mRect;
    Layout::Vec2 mDesiredSize;
    ScrollMetrics mScrollMetrics;
    ScrollPosition mScrollPosition;
    Layout::Rect mScrollableOverflow;
    Layout::Rect mScrollport;
    std::vector<std::unique_ptr<Node>> mChildren;
    std::uint64_t mChildSnapshotRevision = 1;
    std::uint64_t mChildTopologyRevision = 1;
    std::function<void(Element&)> mOnActivate;
    std::unique_ptr<detail::EventHandlerCallStore> mEventHandlerCallStore;
    std::vector<EventListener> mEventListeners;
    Surface* mSurface = nullptr;
    std::weak_ptr<char> mSurfaceLifetime;
    bool mHovered = false;
    bool mActive = false;
    bool mFocused = false;
    bool mFocusVisible = false;
    bool mDisabled = false;
    std::optional<bool> mPointerEvents;
    std::optional<Style::Visibility> mVisibilityOverride;
    std::optional<bool> mDisplayNoneOverride;
    bool mIdScopeRoot = false;
    bool mRectExplicit = false;
    bool mSuppressTextSlots = false;
    std::optional<LocalizedContent> mLocalizedContent;

    struct TextContentSlot {
        LocalizedText text;
        Node* first = nullptr;
        Node* last = nullptr;
    };
    std::vector<TextContentSlot> mTextContentSlots;
    std::uint64_t mStyleRevision = 1;
    std::uint64_t mLayoutInvalidationRevision = 0;
    InvalidationFlags mInvalidationReasons =
        LayoutInvalidationReason::Measure | LayoutInvalidationReason::Arrange | LayoutInvalidationReason::ComputedStyle;
    std::unique_ptr<detail::ElementPrivateData> mPrivate;
};

class Element::ClassList {
public:
    ClassList& add(std::string_view className);
    ClassList& remove(std::string_view className);
    bool toggle(std::string_view className);
    bool replace(std::string_view oldClass, std::string_view newClass);
    bool contains(std::string_view className) const;

private:
    explicit ClassList(Element& element)
        : mElement(element) {}

    friend class Element;

    Element& mElement;
};

class Element::ConstClassList {
public:
    bool contains(std::string_view className) const;

private:
    explicit ConstClassList(const Element& element)
        : mElement(element) {}

    friend class Element;

    const Element& mElement;
};
} // namespace Core
