#ifndef LL_RDUI_FLOATER_H
#define LL_RDUI_FLOATER_H

#include "rduilocalization.h"
#include "rduiwidget.h"
#include <cstdint>
#include <optional>

namespace rdui
{
    class Button;
    class Icon;
    class Label;
    class Panel;

    class Floater : public Widget
    {
        friend class Surface;
        friend class detail::WidgetContractRegistry;

        public:
            static constexpr const char* ELEMENT = "floater";

            Floater();

            Floater& setTitle(std::string localization_key);
            Floater& setIcon(std::string icon);
            Floater& setCloseIcon(std::string icon);
            Floater& setMinimizeIcon(std::string icon);
            Floater& setShowHeaderIdentity(bool value);
            Floater& setCanClose(bool value);
            Floater& setCanMinimize(bool value);
            Floater& setCanResize(bool value);
            Floater& setCanDetach(bool value);

            const std::string& title() const { return mTitle.value(); }
            const std::string& icon() const { return mIcon; }
            const std::string& closeIcon() const { return mCloseIcon; }
            const std::string& minimizeIcon() const { return mMinimizeIcon; }
            bool canClose() const { return mCanClose; }
            bool canMinimize() const { return mCanMinimize; }
            bool canResize() const { return mCanResize; }
            bool canDetach() const { return mCanDetach; }
            bool showHeaderIdentity() const { return mShowHeaderIdentity; }
            bool closed() const { return mClosed; }
            bool minimized() const { return mMinimized; }
            bool dragging() const { return mInteraction == FloaterInteraction::Move; }
            const Rect& expandedRect() const { return mExpandedRect; }

            Panel* header() { return mHeader.get(); }
            const Panel* header() const { return mHeader.get(); }
            Panel* content() { return mContent.get(); }
            const Panel* content() const { return mContent.get(); }
            Button* closeButton() { return mCloseButton.get(); }
            const Button* closeButton() const { return mCloseButton.get(); }
            Button* minimizeButton() { return mMinimizeButton.get(); }
            const Button* minimizeButton() const { return mMinimizeButton.get(); }

            Floater& addChild(std::unique_ptr<Widget> child) override;
            Floater& prependChild(std::unique_ptr<Widget> child) override;
            void clearChildren() override;
            void open();
            void close();
            void setMinimized(bool minimized);
            void toggleMinimized();

            bool defaultPointerEvents() const override { return true; }
        protected:
            bool beginPointerInteraction(const PointerEvent& event) override;
            bool updatePointerInteraction(const PointerEvent& event) override;
            bool endPointerInteraction(const PointerEvent& event) override;
            void onChildrenCleared() override;
            void onLocaleChanged(const System& system) override;

        private:
            enum class FloaterInteraction : std::uint8_t { None, Move, Resize };

            struct ResizeInteraction
            {
                std::uint8_t edges = 0;
                Vec2 initialPointer;
                Rect initialRect;
                Vec2 minimum;
                std::optional<Rect> bounds;
            };

            bool overChromeButton(const Vec2& point) const;
            Vec2 clampedPosition(const Vec2& position) const;
            bool beginResizeInteraction(const PointerEvent& event, std::uint8_t edges,
                                        const Vec2& minimum, const std::optional<Rect>& bounds);
            void setOriginalSize(const Vec2& size);
            Vec2 originalSize() const;
            void configureCompositeParts();
            Panel* claimCustomHeader();
            Floater& setResolvedTitle(std::string localization_key, std::string value);
            void setMovementBounds(const Rect& bounds);
            void clampToMovementBounds();
            void updateHeaderPresentation();

            TextValue mTitle;
            std::string mIcon;
            std::string mCloseIcon;
            std::string mMinimizeIcon;
            Rect mMovementBounds;
            Rect mExpandedRect;
            Vec2 mDragOffset;
            Vec2 mOriginalSize;
            ResizeInteraction mResizeInteraction;
            WidgetRef<Panel> mHeader;
            WidgetRef<Panel> mCustomHeader;
            WidgetRef<Panel> mContent;
            WidgetRef<Icon> mHeaderIcon;
            WidgetRef<Label> mHeaderTitle;
            WidgetRef<Button> mCloseButton;
            WidgetRef<Icon> mCloseButtonIcon;
            WidgetRef<Button> mMinimizeButton;
            WidgetRef<Icon> mMinimizeButtonIcon;
            bool mCanClose = true;
            bool mCanMinimize = false;
            bool mCanResize = false;
            bool mCanDetach = true;
            bool mShowHeaderIdentity = true;
            bool mClosed = false;
            bool mMinimized = false;
            FloaterInteraction mInteraction = FloaterInteraction::None;
            bool mOriginalSizeCaptured = false;
            bool mDetachRequested = false;
            bool mCustomHeaderClaimed = false;
            Visibility mCustomHeaderVisibility = Visibility::Visible;
            Visibility mContentVisibility = Visibility::Visible;
    };
}

#endif // LL_RDUI_FLOATER_H
