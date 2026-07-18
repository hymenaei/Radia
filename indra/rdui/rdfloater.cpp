#include "linden_common.h"
#include "rdfloater.h"
#include "rdbutton.h"
#include "rdicon.h"
#include "rdlabel.h"
#include "rduilayout.h"
#include "rduifloaterresize.h"
#include "rduilocalization.h"
#include "rduistyle.h"
#include "rduisurface.h"
#include "rduisystem.h"
#include "rduiviewcontract.h"
#include "rdpanel.h"
#include <algorithm>
#include <cmath>

namespace rdui
{
    Floater::Floater() : Widget(ELEMENT)
    {
        detail::instantiateCompositeParts(*this, detail::floaterContract());
        configureCompositeParts();
    }

    void Floater::configureCompositeParts()
    {
        mHeaderIcon->setName(mIcon);
        mHeaderTitle->setText(mTitle.value());
        mMinimizeButton->setVisibility(mCanMinimize ? Visibility::Visible : Visibility::Collapsed)
                       .setOnActivate([this](Widget&) { toggleMinimized(); });
        mMinimizeButtonIcon->setName(mMinimizeIcon);
        mCloseButton->setVisibility(mCanClose ? Visibility::Visible : Visibility::Collapsed)
                    .setOnActivate([this](Widget&) { close(); });
        mCloseButtonIcon->setName(mCloseIcon);
        updateHeaderPresentation();
    }

    WidgetContract detail::floaterContract()
    {
        return defineWidget<Floater>(Floater::ELEMENT)
            .attributes({
                localizedStringAttribute("title", &Floater::setResolvedTitle),
                stringAttribute("icon", &Floater::setIcon),
                stringAttribute("closeIcon", &Floater::setCloseIcon),
                stringAttribute("minimizeIcon", &Floater::setMinimizeIcon),
                booleanAttribute("canClose", &Floater::setCanClose),
                booleanAttribute("canMinimize", &Floater::setCanMinimize),
                booleanAttribute("canResize", &Floater::setCanResize),
                booleanAttribute("canDetach", &Floater::setCanDetach),
                booleanAttribute("showHeaderIdentity", &Floater::setShowHeaderIdentity),
            })
            .validate([](const LayoutElement&, Floater& floater, ViewBuildResult& result, const std::string& source, const ViewBuildContext*)
            {
                if (floater.canMinimize() && floater.title().empty())
                    result.error("view.floater.title_required", "A minimizable floater requires a non-empty title.", source);
            })
            .childContainer("header", {},
                [](const LayoutElement& child, Floater& floater, ViewBuildResult& result, const std::string& source) -> Widget*
            {
                Panel* header = floater.claimCustomHeader();
                if (!header) result.error("view.part.duplicate", "A floater may declare only one <header>.", source);
                else applyCommonViewAttributes(child, *header, result, source);
                return header;
            })
            .state(WidgetState::Minimized)
            .part("header", &Floater::mHeader)
            .part("header::icon", &Floater::mHeaderIcon)
            .part("header::title", &Floater::mHeaderTitle)
            .part("header::custom", &Floater::mCustomHeader)
            .part("header::minimize", &Floater::mMinimizeButton)
            .part("header::minimize::icon", &Floater::mMinimizeButtonIcon)
            .part("header::close", &Floater::mCloseButton)
            .part("header::close::icon", &Floater::mCloseButtonIcon)
            .part("content", &Floater::mContent)
            .build();
    }

    Floater& Floater::setTitle(std::string localization_key)
    {
        const System* system = attachedSystem();
        const std::string value = system ? system->resolveText(localization_key) : localization_key;
        return setResolvedTitle(std::move(localization_key), value);
    }

    Floater& Floater::setResolvedTitle(std::string localization_key, std::string value)
    {
        mTitle = TextValue::fromLocalization(std::move(localization_key), std::move(value));
        if (mHeaderTitle) mHeaderTitle->setText(mTitle.value());
        updateHeaderPresentation();
        return *this;
    }

    void Floater::onLocaleChanged(const System& system)
    {
        if (mTitle.localized())
        {
            mTitle.updateLocalizedValue(system.resolveText(mTitle.localizationKey()));
            if (mHeaderTitle) mHeaderTitle->setText(mTitle.value());
            updateHeaderPresentation();
        }
    }

    Floater& Floater::setIcon(std::string icon)
    {
        mIcon = std::move(icon);
        if (mHeaderIcon) mHeaderIcon->setName(mIcon);
        updateHeaderPresentation();
        return *this;
    }

    Floater& Floater::setCloseIcon(std::string icon)
    {
        mCloseIcon = std::move(icon);
        if (mCloseButtonIcon) mCloseButtonIcon->setName(mCloseIcon);
        return *this;
    }

    Floater& Floater::setMinimizeIcon(std::string icon)
    {
        mMinimizeIcon = std::move(icon);
        if (mMinimizeButtonIcon) mMinimizeButtonIcon->setName(mMinimizeIcon);
        return *this;
    }

    Floater& Floater::setShowHeaderIdentity(bool value)
    {
        mShowHeaderIdentity = value;
        updateHeaderPresentation();
        return *this;
    }

    Floater& Floater::setCanClose(bool value)
    {
        mCanClose = value;
        if (mCloseButton) mCloseButton->setVisibility(value ? Visibility::Visible : Visibility::Collapsed);
        return *this;
    }

    Floater& Floater::setCanMinimize(bool value)
    {
        if (!value && mMinimized) setMinimized(false);
        mCanMinimize = value;
        if (mMinimizeButton) mMinimizeButton->setVisibility(value ? Visibility::Visible : Visibility::Collapsed);
        return *this;
    }
    Floater& Floater::setCanResize(bool value) { mCanResize = value; return *this; }
    Floater& Floater::setCanDetach(bool value) { mCanDetach = value; return *this; }
    void Floater::setMovementBounds(const Rect& bounds) { mMovementBounds = bounds; }

    Floater& Floater::addChild(std::unique_ptr<Widget> child)
    {
        if (mContent) mContent->addChild(std::move(child));
        else Widget::addChild(std::move(child));
        return *this;
    }

    Floater& Floater::prependChild(std::unique_ptr<Widget> child)
    {
        if (mContent) mContent->prependChild(std::move(child));
        else Widget::prependChild(std::move(child));
        return *this;
    }

    void Floater::clearChildren()
    {
        if (mContent) mContent->clearChildren();
        if (mCustomHeader) mCustomHeader->clearChildren();
        mCustomHeaderClaimed = false;
    }

    Panel* Floater::claimCustomHeader()
    {
        if (mCustomHeaderClaimed) return nullptr;
        mCustomHeaderClaimed = true;
        return mCustomHeader.get();
    }

    void Floater::updateHeaderPresentation()
    {
        const bool show_identity = mMinimized || mShowHeaderIdentity;
        if (mHeaderIcon) mHeaderIcon->setVisibility(show_identity && !mIcon.empty() ? Visibility::Visible : Visibility::Collapsed);
        if (mHeaderTitle) mHeaderTitle->setVisibility(show_identity && !mTitle.value().empty() ? Visibility::Visible : Visibility::Collapsed);
    }

    void Floater::open()
    {
        mClosed = false;
        setVisibility(Visibility::Visible);
    }

    void Floater::close()
    {
        if (mClosed || !mCanClose) return;
        mClosed = true;
        mInteraction = FloaterInteraction::None;
        setVisibility(Visibility::Collapsed);
        if (Surface* surface = attachedSurface()) surface->floaterClosed(*this);
    }

    void Floater::setMinimized(bool minimized)
    {
        if ((minimized && !mCanMinimize) || minimized == mMinimized || !mHeader) return;

        if (minimized)
        {
            mExpandedRect = rect();
            if (mContent) mContentVisibility = mContent->visibility();
            if (mCustomHeader) mCustomHeaderVisibility = mCustomHeader->visibility();
            mMinimized = true;
            setState(WidgetState::Minimized, true);
            if (mContent) mContent->setVisibility(Visibility::Collapsed);
            if (mCustomHeader) mCustomHeader->setVisibility(Visibility::Collapsed);
            updateHeaderPresentation();

            float width = rect().w;
            float height = mHeader->rect().h;
            if (const StyleSheet* style_sheet = attachedStyleSheet())
            {
                const Vec2 header_size = measureWidget(*mHeader, *style_sheet, attachedTextMetrics());
                const Style floater_style = resolveWidgetStyle(*style_sheet, *this);
                const Style header_style = resolveWidgetStyle(*style_sheet, *mHeader);
                width = header_size.x + header_style.margin.horizontal() + floater_style.padding.horizontal();
                height = header_size.y + header_style.margin.vertical() + floater_style.padding.vertical();
            }
            if (mMovementBounds.w > 0.f) width = std::min(width, mMovementBounds.w);
            setRect({rect().x, rect().top() - height, width, height});
        }
        else
        {
            mMinimized = false;
            setState(WidgetState::Minimized, false);
            setRect(mExpandedRect);
            if (mContent) mContent->setVisibility(mContentVisibility);
            if (mCustomHeader) mCustomHeader->setVisibility(mCustomHeaderVisibility);
            updateHeaderPresentation();
            clampToMovementBounds();
        }
        if (Surface* surface = attachedSurface()) surface->floaterMinimizedChanged(*this, minimized);
    }

    void Floater::toggleMinimized()
    {
        setMinimized(!mMinimized);
    }

    void Floater::clampToMovementBounds()
    {
        const Vec2 position = clampedPosition({rect().x, rect().y});
        const Vec2 delta = position - Vec2{rect().x, rect().y};
        if (delta.x == 0.f && delta.y == 0.f) return;
        translate(delta);
        if (mMinimized)
        {
            mExpandedRect.x += delta.x;
            mExpandedRect.y += delta.y;
        }
        if (Surface* surface = attachedSurface()) surface->floaterMoved(*this);
    }

    bool Floater::overChromeButton(const Vec2& point) const
    {
        return (mCloseButton && mCloseButton->visibility() == Visibility::Visible && mCloseButton->rect().contains(point))
            || (mMinimizeButton && mMinimizeButton->visibility() == Visibility::Visible && mMinimizeButton->rect().contains(point));
    }

    Vec2 Floater::clampedPosition(const Vec2& position) const
    {
        if (mMovementBounds.w <= 0.f || mMovementBounds.h <= 0.f) return position;
        return {
            std::clamp(position.x, mMovementBounds.left(), std::max(mMovementBounds.left(), mMovementBounds.right() - rect().w)),
            std::clamp(position.y, mMovementBounds.bottom(), std::max(mMovementBounds.bottom(), mMovementBounds.top() - rect().h))
        };
    }

    void Floater::setOriginalSize(const Vec2& size)
    {
        mOriginalSize = {std::max(0.f, size.x), std::max(0.f, size.y)};
        mOriginalSizeCaptured = true;
    }

    Vec2 Floater::originalSize() const
    {
        return mOriginalSizeCaptured ? mOriginalSize : Vec2{rect().w, rect().h};
    }

    bool Floater::beginResizeInteraction(const PointerEvent& event, std::uint8_t edges,
                                         const Vec2& minimum, const std::optional<Rect>& bounds)
    {
        if (event.button != PointerButton::Left || !mCanResize || mMinimized || edges == 0) return false;
        mInteraction = FloaterInteraction::Resize;
        mDetachRequested = false;
        mResizeInteraction = {edges, event.position, rect(), minimum, bounds};
        return true;
    }

    bool Floater::beginPointerInteraction(const PointerEvent& event)
    {
        if (event.button != PointerButton::Left || !mHeader || !mHeader->rect().contains(event.position) || overChromeButton(event.position)) return false;
        if (event.clickCount >= 2 && mCanMinimize)
        {
            mInteraction = FloaterInteraction::None;
            toggleMinimized();
            return true;
        }
        mInteraction = FloaterInteraction::Move;
        mDetachRequested = false;
        mDragOffset = event.position - Vec2{rect().x, rect().y};
        return true;
    }

    bool Floater::updatePointerInteraction(const PointerEvent& event)
    {
        if (mInteraction == FloaterInteraction::Resize)
        {
            const Rect resized = detail::resizedRect(
                mResizeInteraction.initialRect, mResizeInteraction.initialPointer, event.position,
                static_cast<detail::ResizeEdges>(mResizeInteraction.edges),
                {mResizeInteraction.minimum, mResizeInteraction.bounds});
            if (resized.x != rect().x || resized.y != rect().y
                || resized.w != rect().w || resized.h != rect().h)
            {
                setRect(resized);
                if (Surface* surface = attachedSurface()) surface->floaterResized(*this, false);
            }
            return true;
        }
        if (mInteraction != FloaterInteraction::Move) return false;
        const Vec2 desired_position = event.position - mDragOffset;
        const Vec2 position = clampedPosition(desired_position);
        constexpr float BREAKAWAY_DISTANCE = 100.f;
        const float pointer_overshoot = std::max({mMovementBounds.left() - event.position.x,
                                                  event.position.x - mMovementBounds.right(),
                                                  mMovementBounds.bottom() - event.position.y,
                                                  event.position.y - mMovementBounds.top(), 0.f});
        Surface* surface = attachedSurface();
        if (!mDetachRequested && !mMinimized && mCanDetach && surface && surface->canDetachFloater(*this) && pointer_overshoot >= BREAKAWAY_DISTANCE)
        {
            mDetachRequested = true;
            surface->floaterDetachRequested(*this, desired_position, mDragOffset);
        }
        const Vec2 delta = position - Vec2{rect().x, rect().y};
        if (delta.x != 0.f || delta.y != 0.f)
        {
            translate(delta);
            if (mMinimized)
            {
                mExpandedRect.x += delta.x;
                mExpandedRect.y += delta.y;
            }
            if (surface) surface->floaterMoved(*this);
        }
        return true;
    }

    bool Floater::endPointerInteraction(const PointerEvent&)
    {
        const FloaterInteraction interaction = mInteraction;
        const bool handled = interaction != FloaterInteraction::None;
        mInteraction = FloaterInteraction::None;
        mDetachRequested = false;
        if (interaction == FloaterInteraction::Resize)
        {
            if (Surface* surface = attachedSurface()) surface->floaterResized(*this, true);
        }
        return handled;
    }

    void Floater::onChildrenCleared()
    {
        mHeader.set(nullptr);
        mCustomHeader.set(nullptr);
        mContent.set(nullptr);
        mHeaderIcon.set(nullptr);
        mHeaderTitle.set(nullptr);
        mCloseButton.set(nullptr);
        mCloseButtonIcon.set(nullptr);
        mMinimizeButton.set(nullptr);
        mMinimizeButtonIcon.set(nullptr);
        mCustomHeaderClaimed = false;
        mInteraction = FloaterInteraction::None;
        mDetachRequested = false;
        mMinimized = false;
        setState(WidgetState::Minimized, false);
        detail::instantiateCompositeParts(*this, detail::floaterContract());
        configureCompositeParts();
    }
}
