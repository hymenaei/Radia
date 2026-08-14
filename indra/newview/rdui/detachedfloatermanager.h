/**
 * @file detachedfloatermanager.h
 * @brief Manages detachment, native presentation, placement, and lifecycle of UI Floaters.
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

#ifndef RD_DETACHEDFLOATERMANAGER_H
#define RD_DETACHEDFLOATERMANAGER_H

#include <functional>
#include <memory>
#include <optional>
#include <variant>
#include "auxiliarywindow.h"
#include "componentpersistence.h"
#include "types.h"

namespace rdui {
class Floater;
class Surface;
} // namespace rdui

namespace rdui::viewer {
struct DetachedFloaterPresentationUpdate {
    bool closeRequested = false;
    bool minimizeRequested = false;
    bool dragEnded = false;
    bool resizeEnded = false;
    AuxiliaryWindowRect nativeRect;
    Vec2 logicalSize;
    Vec2 headerCenterScreen;
};

struct DetachedFloaterPresentationOpenRequest {
    AuxiliaryWindowRect rect;
    float scaleMultiplier = 1.f;
    std::optional<Vec2> dragOffset;
    std::optional<Vec2> logicalSize;
    std::optional<AuxiliaryWindowPoint> dragCursor;
};

class DetachedFloaterPresentation {
public:
    virtual ~DetachedFloaterPresentation() = default;

    virtual std::optional<DetachedFloaterPresentationUpdate> open(const DetachedFloaterPresentationOpenRequest& request) = 0;
    virtual bool beginResize() = 0;
    virtual void applyResize(const Rect& logicalRect) = 0;
    virtual DetachedFloaterPresentationUpdate update() = 0;
    virtual void setVisible(bool visible) = 0;
    virtual std::optional<Rect> prepareReplacement(Floater& replacement) = 0;
    virtual std::unique_ptr<Floater> releaseFloater() = 0;
    virtual std::unique_ptr<Floater> replaceFloater(std::unique_ptr<Floater> replacement, const std::optional<Vec2>& logicalSize) = 0;
};

class DetachedFloaterPresentationResult final {
public:
    static DetachedFloaterPresentationResult success(std::unique_ptr<DetachedFloaterPresentation> value);
    static DetachedFloaterPresentationResult failure(std::unique_ptr<Floater> value);

    ~DetachedFloaterPresentationResult();
    DetachedFloaterPresentationResult(DetachedFloaterPresentationResult&&) noexcept;
    DetachedFloaterPresentationResult& operator=(DetachedFloaterPresentationResult&&) noexcept;
    DetachedFloaterPresentationResult(const DetachedFloaterPresentationResult&) = delete;
    DetachedFloaterPresentationResult& operator=(const DetachedFloaterPresentationResult&) = delete;

    explicit operator bool() const noexcept;
    std::unique_ptr<DetachedFloaterPresentation> takePresentation() &&;
    std::unique_ptr<Floater> takeReturnedFloater() &&;

private:
    using Value = std::variant<std::unique_ptr<DetachedFloaterPresentation>, std::unique_ptr<Floater>>;

    explicit DetachedFloaterPresentationResult(std::unique_ptr<DetachedFloaterPresentation> value);
    explicit DetachedFloaterPresentationResult(std::unique_ptr<Floater> value);

    Value mValue;
};

class DetachedFloaterManager {
public:
    enum class ReattachMode { PersistPlacement, PreservePlacement };

    class Replacement final {
    public:
        static Replacement success(std::unique_ptr<Floater> retired, Floater* installed);
        static Replacement failure();

        explicit operator bool() const noexcept { return mRetired != nullptr && mInstalled != nullptr; }
        Floater* installed() const noexcept { return mInstalled; }
        Floater* retired() const noexcept { return mRetired.get(); }
        std::unique_ptr<Floater> takeRetired() && { return std::move(mRetired); }

    private:
        Replacement() = default;
        Replacement(std::unique_ptr<Floater> retired, Floater* installed) : mRetired(std::move(retired)), mInstalled(installed) {}

        std::unique_ptr<Floater> mRetired;
        Floater* mInstalled = nullptr;
    };

    class DetachedFloaterEnvironment {
    public:
        virtual ~DetachedFloaterEnvironment() = default;
        virtual AuxiliaryWindowRect mainRectToNative(const Rect& rect) const = 0;
        virtual float nativeScaleMultiplier() const = 0;
        virtual Vec2 nativeBottomLeftInMain(const AuxiliaryWindowRect& rect) const = 0;
        virtual bool nativePointInsideMain(const Vec2& point) const = 0;
        virtual bool placementVisible(const AuxiliaryWindowRect& rect) const = 0;
        virtual std::optional<AuxiliaryWindowPoint> releasePointerForDetach(const Vec2& mainPosition) = 0;
    };

    using PresentationFactory = std::function<DetachedFloaterPresentationResult(std::unique_ptr<Floater>)>;
    using PlacementWriter = std::function<void(const ComponentKey&, FloaterPlacement, ComponentOpenState)>;

    DetachedFloaterManager(Surface& attachedSurface, PresentationFactory presentationFactory, DetachedFloaterEnvironment& environment,
                           PlacementWriter placementWriter);
    ~DetachedFloaterManager();
    DetachedFloaterManager(const DetachedFloaterManager&) = delete;
    DetachedFloaterManager& operator=(const DetachedFloaterManager&) = delete;

    void requestDetach(const ComponentKey& componentKey, Floater& floater, const Vec2& desired, const Vec2& dragOffset);
    void processPendingDetachment();
    void update();
    void reattachAll(ReattachMode mode = ReattachMode::PersistPlacement);
    void setVisible(bool visible);
    bool beginResize(Floater& floater);
    void applyResize(Floater& floater, const Rect& logicalRect);

    bool isDetached(const Floater& floater) const;
    Vec2 logicalSize(const Floater& floater) const;
    std::optional<Rect> prepareReplacement(Floater& current, Floater& replacement);
    Replacement replace(Floater& current, std::unique_ptr<Floater> replacement, const std::optional<Vec2>& logicalSize);
    bool restoreDetachedPlacement(const ComponentKey& componentKey, Floater& floater, const DetachedFloaterPlacement& placement);

private:
    struct Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace rdui::viewer
#endif // RD_DETACHEDFLOATERMANAGER_H
