#ifndef LL_RDUI_WIDGET_H
#define LL_RDUI_WIDGET_H

#include "rduiaction.h"
#include "rduievent.h"
#include "rduitypes.h"
#include <chrono>
#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <vector>

namespace rdui
{
    enum class Visibility : uint8_t
    {
        Visible,
        Hidden,
        Collapsed,
    };

    class Widget;
    class LayoutEngine;
    class PaintContext;
    class System;
    class Surface;
    struct Style;
    class StyleSheet;
    class TextMetrics;
    namespace detail
    {
        class WidgetCompilerAccess;
    }

    template<typename WidgetT>
    class WidgetRef
    {
        public:
            WidgetRef() = default;
            WidgetRef(WidgetT* widget) { set(widget); }

            WidgetT* get() const { return mLifetime.expired() ? nullptr : mWidget; }
            WidgetT* operator->() const { return get(); }
            WidgetT& operator*() const { return *get(); }
            explicit operator bool() const { return get() != nullptr; }

            void set(WidgetT* widget)
            {
                mWidget = widget;
                mLifetime = widget ? widget->lifetime() : std::weak_ptr<char>();
            }

        private:
            WidgetT* mWidget = nullptr;
            std::weak_ptr<char> mLifetime;
    };

    class Widget
    {
        template<typename>
        friend class WidgetRef;
        friend class Binder;
        friend class LayoutEngine;
        friend class Surface;
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

            virtual Vec2 intrinsicSize(const StyleSheet& theme, const Style& style, const TextMetrics& text_metrics) const;
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
            struct ActionSlot
            {
                std::string name;
                std::weak_ptr<detail::ActionHandler> handler;
            };

            void bindAction(ActionEventKind kind, const std::shared_ptr<detail::ActionHandler>& handler);
            void dispatchMouseAction(ActionEventKind kind, const PointerEvent& event);
            void dispatchLongClickAction(std::chrono::milliseconds held_for);
            void invalidateArrange();
            void invalidateStyleTree();
            void setSurface(Surface* surface);
            Widget& setStyleElement(std::string style_element);
            Widget& setPart(std::string part);
            Widget& setIdScopeRoot(bool scope_root);
            std::weak_ptr<char> lifetime() const { return mLifetime; }

            std::string mElement;
            std::string mStyleElement;
            std::string mPart;
            std::string mId;
            std::set<std::string> mClasses;
            Rect mRect;
            Vec2 mDesiredSize;
            std::vector<std::unique_ptr<Widget>> mChildren;
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
            bool mMeasureDirty = true;
            bool mArrangeDirty = true;
    };
}

#endif // LL_RDUI_WIDGET_H
