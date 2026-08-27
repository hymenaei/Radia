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
#include "event.h"
#include "eventcall.h"
#include "localizedtext.h"
#include "types.h"

namespace radia::ui {
enum class LayoutInvalidationReason : uint8_t {
    NoInvalidation = 0,
    Measure = 1 << 0,
    Arrange = 1 << 1,
    Style = 1 << 2,
    Text = 1 << 3,
    Paint = 1 << 4
};

class InvalidationFlags {
public:
    constexpr InvalidationFlags() = default;
    constexpr InvalidationFlags(LayoutInvalidationReason reason) : mBits(static_cast<uint8_t>(reason)) {}
    constexpr explicit InvalidationFlags(uint8_t bits) : mBits(bits) {}

    constexpr bool intersects(InvalidationFlags other) const { return (mBits & other.mBits) != 0; }
    constexpr bool contains(LayoutInvalidationReason reason) const { return (mBits & static_cast<uint8_t>(reason)) != 0; }
    constexpr uint8_t value() const { return mBits; }

    constexpr void add(InvalidationFlags other) { mBits = static_cast<uint8_t>(mBits | other.mBits); }
    constexpr void remove(InvalidationFlags other) { mBits = static_cast<uint8_t>(mBits & static_cast<uint8_t>(~other.mBits)); }

private:
    uint8_t mBits = 0;
};

struct IntrinsicSizeConstraints {
    std::optional<float> width;
    std::optional<float> height;

    IntrinsicSizeConstraints() = default;
    IntrinsicSizeConstraints(std::optional<float> constrainedWidth, std::optional<float> constrainedHeight)
        : width(constrainedWidth), height(constrainedHeight) {}
};

inline constexpr InvalidationFlags layoutInvalidationMask(LayoutInvalidationReason reason) {
    return InvalidationFlags(reason);
}

inline constexpr InvalidationFlags layoutInvalidationMask(InvalidationFlags flags) {
    return flags;
}

inline constexpr InvalidationFlags operator|(LayoutInvalidationReason left, LayoutInvalidationReason right) {
    return InvalidationFlags(static_cast<uint8_t>(left) | static_cast<uint8_t>(right));
}

inline constexpr InvalidationFlags operator|(InvalidationFlags left, LayoutInvalidationReason right) {
    return InvalidationFlags(static_cast<uint8_t>(left.value() | static_cast<uint8_t>(right)));
}

inline constexpr InvalidationFlags operator|(LayoutInvalidationReason left, InvalidationFlags right) {
    return right | left;
}

inline constexpr InvalidationFlags kMeasureInvalidationReasons =
    layoutInvalidationMask(LayoutInvalidationReason::Measure | LayoutInvalidationReason::Style | LayoutInvalidationReason::Text);
inline constexpr InvalidationFlags kArrangeInvalidationReasons = kMeasureInvalidationReasons | LayoutInvalidationReason::Arrange;
inline constexpr InvalidationFlags kTextInvalidationReasons =
    layoutInvalidationMask(LayoutInvalidationReason::Measure | LayoutInvalidationReason::Arrange | LayoutInvalidationReason::Text);
inline constexpr InvalidationFlags kPaintStyleInvalidationReasons = layoutInvalidationMask(LayoutInvalidationReason::Paint);
inline constexpr InvalidationFlags kLayoutStyleInvalidationReasons = kArrangeInvalidationReasons | LayoutInvalidationReason::Paint;

class Element;
class FloaterElement;
class Document;
class Text;
class Node;
enum class NodeType : uint8_t { Document, Element, Text };

using NodePtr = std::unique_ptr<Node>;
using ElementPtr = std::unique_ptr<Element>;
using NodeList = std::vector<Node*>;
using ConstNodeList = std::vector<const Node*>;
using ElementList = std::vector<Element*>;
using ConstElementList = std::vector<const Element*>;

namespace detail {
struct DocumentIdentity;
struct ElementPrivateData;
class NodeAccess;
class ElementInternalAccess;
}

class Node {
public:
    virtual ~Node() = default;

    NodeType nodeType() const noexcept { return mNodeType; }
    Node* parentNode() noexcept { return mParentNode; }
    const Node* parentNode() const noexcept { return mParentNode; }
    Element* parentElement() noexcept { return mParent; }
    const Element* parentElement() const noexcept { return mParent; }
    virtual Node* firstChild() noexcept { return nullptr; }
    virtual const Node* firstChild() const noexcept { return nullptr; }
    virtual NodeList childNodes() { return {}; }
    virtual ConstNodeList childNodes() const { return {}; }
    Node* nextSibling() noexcept;
    const Node* nextSibling() const noexcept;
    Node* before(NodePtr node);
    Node* after(NodePtr node);
    NodePtr replaceWith(NodePtr node);
    NodePtr remove();
    virtual Document* asDocument() noexcept { return nullptr; }
    virtual const Document* asDocument() const noexcept { return nullptr; }
    virtual Element* asElement() noexcept { return nullptr; }
    virtual const Element* asElement() const noexcept { return nullptr; }
    virtual Text* asText() noexcept { return nullptr; }
    virtual const Text* asText() const noexcept { return nullptr; }

protected:
    explicit Node(NodeType nodeType) : mNodeType(nodeType) {}

    Element* mParent = nullptr;
    Node* mParentNode = nullptr;
    bool mFlowBreakBefore = false;

private:
    friend class detail::NodeAccess;
    NodeType mNodeType;
    std::shared_ptr<char> mLifetime = std::make_shared<char>(0);
    std::shared_ptr<detail::DocumentIdentity> mDocumentIdentity;
};

namespace detail {
class ElementCompilerAccess;
class ElementDefinitionFactory;
template<typename ElementT> class ElementVisit;
class NodeChildren;
class ConstNodeChildren;
const std::string* styleAttribute(const Element& element, std::string_view name);
Node& appendText(Element& parent, std::string text);
Node& appendLocalizedText(Element& parent, LocalizedText text, std::string markup);
NodeChildren nodes(Element& element);
ConstNodeChildren nodes(const Element& element);
} // namespace detail

class TreeTraversalCache;
class LayoutEngine;
class PaintContext;
class System;
class Surface;
struct Style;
class StyleSheet;
class TextMetrics;
enum class VerticalAlign;

struct ScrollMetrics {
    float scrollWidth = 0.f;
    float scrollHeight = 0.f;
    float clientWidth = 0.f;
    float clientHeight = 0.f;
    float maxScrollLeft = 0.f;
    float maxScrollTop = 0.f;
};

namespace layout_detail {
class ElementLayoutAccess;
struct ChildLayout;
Rect positionedRect(const ChildLayout& child, const Rect& parent, VerticalAlign verticalAlignment);
void setArrangedRect(Element& node, const Rect& rect);
} // namespace layout_detail

class Element : public Node {
    template<typename> friend class detail::ElementVisit;
    friend class detail::ElementInternalAccess;
    friend class Node;
    friend class Text;
    friend class Document;
    friend class TreeTraversalCache;
    friend class Binder;
    friend class FloaterElement;
    friend class LayoutEngine;
    friend class StylePass;
    friend class Surface;
    friend Rect layout_detail::positionedRect(const layout_detail::ChildLayout&, const Rect&, VerticalAlign);
    friend void layout_detail::setArrangedRect(Element&, const Rect&);
    friend class layout_detail::ElementLayoutAccess;
    friend class detail::ElementCompilerAccess;
    friend const std::string* detail::styleAttribute(const Element&, std::string_view);
    friend Node& detail::appendText(Element&, std::string);
    friend Node& detail::appendLocalizedText(Element&, LocalizedText, std::string);
    friend detail::NodeChildren detail::nodes(Element&);
    friend detail::ConstNodeChildren detail::nodes(const Element&);
    friend Style resolveElementStyle(const StyleSheet& styleSheet, const Element& node);

public:
    explicit Element(const char* elementName);
    virtual ~Element();

    Element* asElement() noexcept override { return this; }
    const Element* asElement() const noexcept override { return this; }

    Node* firstChild() noexcept override;
    const Node* firstChild() const noexcept override;
    NodeList childNodes() override;
    ConstNodeList childNodes() const override;
    Element& setId(std::string id);
    Element& addClass(std::string className);
    Element& setRect(const Rect& rect);
    Element& setPointerEvents(bool pointerEvents);
    Element& disabled(bool disabled);
    Element& setVisibility(Visibility visibility);
    Element& setHidden(bool hidden);
    Node* append(NodePtr child);
    Node* prepend(NodePtr child);
    void replaceChildren();
    Node* replaceChildren(NodePtr child);
    std::string textContent() const;
    Element& textContent(std::string text);
    Element& textContent(LocalizedText text);
    Element& content(std::string text);
    Element& content(LocalizedText text);
    Element& setOnActivate(std::function<void(Element&)> callback);
    Element& setEventCall(std::string_view type, EventCall call);
    void addEventListener(std::string_view type, EventHandler handler, bool capture = false);
    void removeEventListener(std::string_view type, const EventHandler& handler, bool capture = false);

    const std::string& elementName() const { return mElementName; }
    const std::string& styleElement() const { return mStyleElement.empty() ? mElementName : mStyleElement; }
    const std::string& part() const { return mPart; }
    const std::string& id() const { return mId; }
    const std::set<std::string>& classes() const { return mClasses; }
    const Rect& rect() const { return mRect; }
    const Vec2& desiredSize() const { return mDesiredSize; }
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
    uint16_t states() const { return mStates; }
    std::uint64_t styleContextRevision() const;
    bool pointerEvents() const { return mPointerEvents.value_or(defaultPointerEvents()); }
    Visibility visibility() const { return mVisibilityOverride.value_or(Visibility::Visible); }
    bool isDisplayed(const Style& style) const;
    bool isVisible(const Style& style) const;
    bool disabled() const { return radia::ui::hasState(mStates, ElementState::Disabled); }
    bool idScopeRoot() const { return mIdScopeRoot; }
    bool flowBreakBefore() const;
    const EventCall* eventCall(std::string_view type) const;

    bool hasState(ElementState state) const { return radia::ui::hasState(mStates, state); }
    void activate();
    void activateFromLabel();

    virtual Vec2 intrinsicSize(const StyleSheet& styleSheet, const Style& style, const TextMetrics& textMetrics,
                               const IntrinsicSizeConstraints& constraints = IntrinsicSizeConstraints()) const;
    virtual bool defaultPointerEvents() const { return false; }
    virtual bool focusable() const { return false; }
    virtual void paint(PaintContext& context, const Style& style, float scale) const;

protected:
    virtual bool defaultKeyDown(const KeyEvent& event);
    virtual bool defaultKeyUp(const KeyEvent& event);
    virtual bool defaultCharacterInput(unsigned int codepoint);
    virtual bool defaultScroll(const WheelEvent& event);
    virtual bool beginPointerInteraction(const PointerEvent& event);
    virtual bool updatePointerInteraction(const PointerEvent& event);
    virtual bool endPointerInteraction(const PointerEvent& event);
    virtual void constrainResolvedStyle(Style& style) const {}
    void dispatchEvent(Event& event);
    void translate(const Vec2& delta);
    void invalidateMeasure();
    void invalidateArrange();
    void invalidateText();
    void invalidatePaint();
    const StyleSheet* styleSheet() const;
    const System* system() const;
    Surface* surface() const { return mSurface; }
    const TextMetrics& textMetrics() const;
    virtual void onActivate() {}
    virtual void onLabelActivate() { activate(); }
    virtual void onTreeAttached() {}
    virtual void onTreeDetached() {}
    virtual bool shouldPaintChild(const Element&, const Style&) const { return true; }
    virtual void onChildAdded(Element&) {}
    virtual void onChildrenCleared() {}
    virtual void onLocaleChanged(const System&);
    virtual bool onKeybindingsChanged(const System&);
    virtual void onArranged(const Style&) {}
    virtual Rect paintBounds() const { return mRect; }
    virtual bool hasLayoutGapBetween(const Element&, const Element&) const { return true; }
    virtual float layoutOverlapBetween(const Element&, const Element&, const Style&) const { return 0.f; }
    void clearDirectTextContent();
    void translateChild(Element& child, const Vec2& delta);
    void setState(ElementState state, bool enabled);
    Element& setDisplayNone(bool displayNone);

private:
    enum class LocalizedContentMode { Literal, Markup };

    struct LocalizedContent {
        LocalizedText text;
        LocalizedContentMode mode;
    };

    virtual void appendNode(NodePtr child);
    virtual void prependNode(NodePtr child);
    virtual void replaceChildrenInternal();
    Node* insertBefore(NodePtr child, Node* reference);
    NodePtr replaceNode(Node& child, NodePtr replacement);
    NodePtr removeNode(Node& child);

    struct EventListener {
        std::string type;
        EventHandler handler;
        bool capture = false;
    };

    void dispatchListeners(Event& event, bool capture);
    void translateSubtree(const Vec2& delta);
    void invalidateArrangeTree();
    void invalidateTextTree();
    void invalidateStyleTree(bool layoutAffecting = true, bool propagateToDescendants = true);
    void clearPaintInvalidationTree();
    void notifyTreeAttached();
    void notifyTreeDetached();
    void setSurface(Surface* surface);
    void rebuildTextContent();
    void rebuildResolvedMarkup(std::string markup);
    void rebuildKeybindingContent(const System& system);
    bool refreshTextContentSlots();
    Element& setStyleElement(std::string styleElement);
    Element& setPart(std::string part);
    Element& setIdScopeRoot(bool scopeRoot);
    void setKeybinding(std::string keybindingId);
    void setScrollMetrics(const ScrollMetrics& metrics, const Rect& scrollableOverflow, const Rect& scrollport);

    struct ScrollPosition {
        float inlineOffset = 0.f;
        float blockOffset = 0.f;
    };

    std::string mElementName;
    std::string mStyleElement;
    std::string mPart;
    std::map<std::string, std::string> mStyleAttributes;
    std::string mId;
    std::set<std::string> mClasses;
    Rect mRect;
    Vec2 mDesiredSize;
    ScrollMetrics mScrollMetrics;
    ScrollPosition mScrollPosition;
    Rect mScrollableOverflow;
    Rect mScrollport;
    std::vector<std::unique_ptr<Node>> mChildren;
    std::uint64_t mChildSnapshotRevision = 1;
    std::function<void(Element&)> mOnActivate;
    std::map<std::string, EventCall, std::less<>> mEventCalls;
    std::vector<EventListener> mEventListeners;
    Surface* mSurface = nullptr;
    uint16_t mStates = 0;
    std::optional<bool> mPointerEvents;
    std::optional<Visibility> mVisibilityOverride;
    std::optional<bool> mDisplayNoneOverride;
    bool mIdScopeRoot = false;
    bool mRectExplicit = false;
    bool mBuildingTextContent = false;
    bool mSuppressTextSlots = false;
    std::optional<LocalizedContent> mLocalizedContent;
    std::string mKeybindingId;

    struct TextContentSlot {
        LocalizedText text;
        Node* first = nullptr;
        Node* last = nullptr;
    };
    std::vector<TextContentSlot> mTextContentSlots;
    std::uint64_t mStyleRevision = 1;
    std::uint64_t mLayoutInvalidationRevision = 0;
    InvalidationFlags mInvalidationReasons =
        layoutInvalidationMask(LayoutInvalidationReason::Measure | LayoutInvalidationReason::Arrange | LayoutInvalidationReason::Style);
    std::unique_ptr<detail::ElementPrivateData> mPrivate;
};
} // namespace radia::ui
