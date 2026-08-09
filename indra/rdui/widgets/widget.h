/**
 * @file widget.h
 * @brief Base retained Widget type, identity, state, and tree ownership.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#ifndef RD_WIDGETS_WIDGET_H
#define RD_WIDGETS_WIDGET_H

#include <chrono>
#include <cstdint>
#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <type_traits>
#include <vector>
#include "action.h"
#include "event.h"
#include "types.h"

namespace rdui {
enum class Visibility : uint8_t { Visible, Hidden, Collapsed };

enum class LayoutInvalidationReason : uint8_t { None = 0, Measure = 1 << 0, Arrange = 1 << 1, Style = 1 << 2, Text = 1 << 3, Paint = 1 << 4 };

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
    IntrinsicSizeConstraints(std::optional<float> constrained_width, std::optional<float> constrained_height)
        : width(constrained_width), height(constrained_height) {}
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

class Widget;
namespace detail { template<typename WidgetT> class WidgetVisit; }
using WidgetVisit = detail::WidgetVisit<Widget>;
using ConstWidgetVisit = detail::WidgetVisit<const Widget>;
class TreeTraversalCache;
class LayoutEngine;
class PaintContext;
class System;
class Surface;
struct Style;
class StyleSheet;
class TextMetrics;
enum class VerticalAlign;

struct LayoutContextKey {
    const StyleSheet* style_sheet = nullptr;
    const TextMetrics* text_metrics = nullptr;
    std::uint64_t style_generation = 0;
    std::uint64_t text_metrics_generation = 0;

    constexpr bool operator==(const LayoutContextKey& other) const {
        return style_sheet == other.style_sheet && text_metrics == other.text_metrics && style_generation == other.style_generation
            && text_metrics_generation == other.text_metrics_generation;
    }
};

namespace detail { class WidgetCompilerAccess; }
namespace layout_detail {
class WidgetLayoutAccess;
struct ChildLayout;
Rect positionedRect(const ChildLayout& child, const Rect& parent, VerticalAlign vertical_alignment);
void setArrangedRect(Widget& node, const Rect& rect);
}

template<typename WidgetT> class WidgetRef {
public:
    WidgetRef() = default;
    WidgetRef(WidgetT* widget) { set(widget); }

    WidgetT* get() const { return mLifetime.expired() ? nullptr : mWidget; }
    WidgetT* operator->() const { return get(); }
    WidgetT& operator*() const { return *get(); }
    explicit operator bool() const { return get() != nullptr; }

    void set(WidgetT* widget) {
        mWidget = widget;
        mLifetime = widget ? widget->lifetime() : std::weak_ptr<char>();
    }

private:
    WidgetT* mWidget = nullptr;
    std::weak_ptr<char> mLifetime;
};

class Widget {
    template<typename> friend class WidgetRef;
    template<typename> friend class detail::WidgetVisit;
    friend class TreeTraversalCache;
    friend class Binder;
    friend class LayoutEngine;
    friend class StylePass;
    friend class Surface;
    friend Rect layout_detail::positionedRect(const layout_detail::ChildLayout&, const Rect&, VerticalAlign);
    friend void layout_detail::setArrangedRect(Widget&, const Rect&);
    friend class layout_detail::WidgetLayoutAccess;
    friend class detail::WidgetCompilerAccess;
    friend Style resolveWidgetStyle(const StyleSheet& theme, const Widget& node);

public:
    virtual ~Widget();

    Widget& setId(std::string id);
    Widget& addClass(std::string class_name);
    Widget& setRect(const Rect& rect);
    Widget& setPointerEvents(bool pointer_events);
    Widget& setDisabled(bool disabled);
    Widget& setVisibility(Visibility visibility);
    Widget& setOnActivate(std::function<void(Widget&)> callback);
    Widget& setAction(ActionEventKind kind, std::string action);
    Widget& setLongClickDelay(std::chrono::milliseconds delay);

    virtual Widget& addChild(std::unique_ptr<Widget> child);
    virtual Widget& prependChild(std::unique_ptr<Widget> child);
    virtual void clearChildren();

    const std::string& element() const { return mElement; }
    const std::string& styleElement() const { return mStyleElement.empty() ? mElement : mStyleElement; }
    const std::string& part() const { return mPart; }
    const std::string& id() const { return mId; }
    const std::set<std::string>& classes() const { return mClasses; }
    const Rect& rect() const { return mRect; }
    const Vec2& desiredSize() const { return mDesiredSize; }
    Widget* parent() { return mParent; }
    const Widget* parent() const { return mParent; }
    const std::vector<std::unique_ptr<Widget>>& children() const { return mChildren; }
    uint8_t states() const { return mStates; }
    std::uint64_t styleContextRevision() const;
    bool pointerEvents() const { return mPointerEvents.value_or(defaultPointerEvents()); }
    Visibility visibility() const { return mVisibility; }
    bool disabled() const { return has_state(mStates, WidgetState::Disabled); }
    bool idScopeRoot() const { return mIdScopeRoot; }
    bool flowBreakBefore() const { return mFlowBreakBefore; }
    const std::string& action(ActionEventKind kind) const;
    const std::optional<std::chrono::milliseconds>& longClickDelay() const { return mLongClickDelay; }

    bool hasState(WidgetState state) const { return has_state(mStates, state); }
    void activate();
    void activateFromLabel();

    virtual Vec2 intrinsicSize(const StyleSheet& theme, const Style& style, const TextMetrics& text_metrics,
                               const IntrinsicSizeConstraints& constraints = IntrinsicSizeConstraints()) const;
    virtual bool defaultPointerEvents() const { return false; }
    virtual bool focusable() const { return false; }
    virtual void paint(PaintContext& context, const Style& style, float scale) const;

protected:
    explicit Widget(const char* element);
    virtual void onEvent(RoutedEvent&) {}
    virtual bool defaultKeyDown(const KeyEvent& event);
    virtual bool defaultKeyUp(const KeyEvent& event);
    virtual bool defaultCharacterInput(unsigned int codepoint);
    virtual bool defaultScroll(const ScrollEvent& event);
    virtual bool beginPointerInteraction(const PointerEvent& event);
    virtual bool updatePointerInteraction(const PointerEvent& event);
    virtual bool endPointerInteraction(const PointerEvent& event);
    virtual void constrainResolvedStyle(Style& style) const {}
    void emitAction(const ActionEvent& event);
    void translate(const Vec2& delta);
    void invalidateMeasure();
    void invalidateText();
    void invalidatePaint();
    const StyleSheet* attachedStyleSheet() const;
    const System* attachedSystem() const;
    Surface* attachedSurface() const { return mSurface; }
    const TextMetrics& attachedTextMetrics() const;
    virtual void onActivate() {}
    virtual void onLabelActivate() { activate(); }
    virtual void onChildAdded(Widget&) {}
    virtual void onChildrenCleared() {}
    virtual void onLocaleChanged(const System&) {}
    virtual bool onKeybindingsChanged(const System&) { return false; }
    virtual void onArranged(const Style&) {}
    virtual Rect paintBounds() const { return mRect; }
    virtual bool hasLayoutGapBetween(const Widget&, const Widget&) const { return true; }
    virtual float layoutOverlapBetween(const Widget&, const Widget&, const Style&) const { return 0.f; }
    void translateChild(Widget& child, const Vec2& delta);
    void setState(WidgetState state, bool enabled);

private:
    struct ActionSlot {
        std::string name;
        std::weak_ptr<detail::ActionHandler> handler;
    };

    void bindAction(ActionEventKind kind, const std::shared_ptr<detail::ActionHandler>& handler);
    void dispatchMouseAction(ActionEventKind kind, const PointerEvent& event);
    void dispatchLongClickAction(std::chrono::milliseconds held_for);
    void translateSubtree(const Vec2& delta);
    void invalidateArrange();
    void invalidateArrangeTree();
    void invalidateTextTree();
    void invalidateStyleTree(bool layout_affecting = true, bool descendants = true);
    void clearPaintInvalidationTree();
    void setSurface(Surface* surface);
    Widget& setStyleElement(std::string style_element);
    Widget& setPart(std::string part);
    Widget& setIdScopeRoot(bool scope_root);
    std::weak_ptr<char> lifetime() const { return mLifetime; }

    struct LayoutCache {
        Vec2 measured_size;
        Vec2 intrinsic_size;
        float measured_width = 0.f;
        float measured_height = 0.f;
        LayoutContextKey layout_context;
        LayoutDirection direction = LayoutDirection::LeftToRight;
        bool measured_width_set = false;
        bool measured_height_set = false;
        bool measured_rect_explicit = false;
        bool measured_rect_constraint_set = false;
        float measured_rect_width = 0.f;
        float measured_rect_height = 0.f;
        bool measure_valid = false;
        bool intrinsic_valid = false;
        bool arrange_valid = false;
    };

    std::string mElement;
    std::string mStyleElement;
    std::string mPart;
    std::string mId;
    std::set<std::string> mClasses;
    Rect mRect;
    Vec2 mDesiredSize;
    std::vector<std::unique_ptr<Widget>> mChildren;
    std::uint64_t mChildSnapshotRevision = 1;
    std::function<void(Widget&)> mOnActivate;
    std::map<ActionEventKind, ActionSlot> mActions;
    std::optional<std::chrono::milliseconds> mLongClickDelay;
    std::shared_ptr<char> mLifetime = std::make_shared<char>(0);
    Widget* mParent = nullptr;
    Surface* mSurface = nullptr;
    uint8_t mStates = 0;
    std::optional<bool> mPointerEvents;
    Visibility mVisibility = Visibility::Visible;
    bool mIdScopeRoot = false;
    bool mFlowBreakBefore = false;
    bool mRectExplicit = false;
    std::uint64_t mStyleRevision = 1;
    std::uint64_t mLayoutInvalidationRevision = 0;
    InvalidationFlags mInvalidationReasons =
        layoutInvalidationMask(LayoutInvalidationReason::Measure | LayoutInvalidationReason::Arrange | LayoutInvalidationReason::Style);
    LayoutCache mLayoutCache;
};

namespace detail {
template<typename WidgetT>
class WidgetVisit {
public:
    static_assert(std::is_same_v<WidgetT, rdui::Widget> || std::is_same_v<WidgetT, const rdui::Widget>);

    WidgetVisit() = default;
    explicit WidgetVisit(WidgetT& widget)
        : lifetime(&widget), surface(widget.mSurface), parent(widget.mParent), layout_revision(widget.mLayoutInvalidationRevision),
          style_revision(widget.mStyleRevision) {}

    WidgetT* get() const { return lifetime.get(); }
    bool valid() const {
        const WidgetT* widget = lifetime.get();
        return widget && widget->mSurface == surface && widget->mParent == parent && widget->mLayoutInvalidationRevision == layout_revision;
    }
    bool validChildOf(const Widget& expected_parent) const {
        const WidgetT* widget = lifetime.get();
        return valid() && widget->mParent == &expected_parent;
    }
    bool styleValid() const {
        const WidgetT* widget = lifetime.get();
        return valid() && widget->mStyleRevision == style_revision;
    }

    WidgetRef<WidgetT> lifetime;
    const Surface* surface = nullptr;
    const Widget* parent = nullptr;
    std::uint64_t layout_revision = 0;
    std::uint64_t style_revision = 0;
};
} // namespace detail
} // namespace rdui
#endif // RD_WIDGETS_WIDGET_H
