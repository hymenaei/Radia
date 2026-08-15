/**
 * @file detachedfloatermanager.cpp
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

#include "llviewerprecompiledheaders.h"
#include "detachedfloatermanager.h"
#include <algorithm>
#include <cmath>
#include <utility>
#include <vector>
#include "surface/surface.h"
#include "widgets/floater.h"

namespace radia::viewer::ui {
using radia::ui::Floater;
using radia::ui::Rect;
using radia::ui::Surface;
using radia::ui::Vec2;

DetachedFloaterPresentationResult::DetachedFloaterPresentationResult(std::unique_ptr<DetachedFloaterPresentation> value) : mValue(std::move(value)) {}

DetachedFloaterPresentationResult::DetachedFloaterPresentationResult(std::unique_ptr<Floater> value) : mValue(std::move(value)) {}

DetachedFloaterPresentationResult::~DetachedFloaterPresentationResult() = default;
DetachedFloaterPresentationResult::DetachedFloaterPresentationResult(DetachedFloaterPresentationResult&&) noexcept = default;
DetachedFloaterPresentationResult& DetachedFloaterPresentationResult::operator=(DetachedFloaterPresentationResult&&) noexcept = default;

DetachedFloaterPresentationResult DetachedFloaterPresentationResult::success(std::unique_ptr<DetachedFloaterPresentation> value) {
    return DetachedFloaterPresentationResult(std::move(value));
}

DetachedFloaterPresentationResult DetachedFloaterPresentationResult::failure(std::unique_ptr<Floater> value) {
    return DetachedFloaterPresentationResult(std::move(value));
}

DetachedFloaterPresentationResult::operator bool() const noexcept {
    const auto* presentation = std::get_if<std::unique_ptr<DetachedFloaterPresentation>>(&mValue);
    return presentation && *presentation != nullptr;
}

std::unique_ptr<DetachedFloaterPresentation> DetachedFloaterPresentationResult::takePresentation() && {
    if (auto* presentation = std::get_if<std::unique_ptr<DetachedFloaterPresentation>>(&mValue)) return std::move(*presentation);
    return {};
}

std::unique_ptr<Floater> DetachedFloaterPresentationResult::takeReturnedFloater() && {
    if (auto* floater = std::get_if<std::unique_ptr<Floater>>(&mValue)) return std::move(*floater);
    return {};
}

DetachedFloaterManager::Replacement DetachedFloaterManager::Replacement::success(std::unique_ptr<Floater> retired, Floater* installed) {
    return Replacement(std::move(retired), installed);
}

DetachedFloaterManager::Replacement DetachedFloaterManager::Replacement::failure() {
    return Replacement();
}

struct DetachedFloaterManager::Impl {
    struct DetachedEntry {
        ComponentKey componentKey;
        Floater* floater = nullptr;
        std::unique_ptr<DetachedFloaterPresentation> presentation;
        DetachedFloaterPresentationUpdate state;
    };

    struct PendingDetach {
        ComponentKey componentKey;
        Floater* floater = nullptr;
        Vec2 desired;
        Vec2 dragOffset;
    };

    Impl(Surface& attachedSurface, PresentationFactory presentationFactory, DetachedFloaterEnvironment& environment, PlacementWriter placementWriter)
        : surface(attachedSurface), makePresentation(std::move(presentationFactory)), environment(environment),
          writePlacement(std::move(placementWriter)) {}

    DetachedEntry* findDetachedEntry(const Floater& floater) {
        const auto found =
            std::find_if(detached.begin(), detached.end(), [&floater](const DetachedEntry& entry) { return entry.floater == &floater; });
        return found == detached.end() ? nullptr : &*found;
    }

    const DetachedEntry* findDetachedEntry(const Floater& floater) const {
        const auto found =
            std::find_if(detached.begin(), detached.end(), [&floater](const DetachedEntry& entry) { return entry.floater == &floater; });
        return found == detached.end() ? nullptr : &*found;
    }

    void notifyDetachedPlacement(const DetachedEntry& entry) {
        if (!writePlacement) return;
        const Floater* floater = entry.floater;
        if (!floater) return;
        const AuxiliaryWindowRect rect = entry.state.nativeRect;
        std::optional<FloaterLogicalSize> logicalSize;
        if (floater->canResize()) {
            const Vec2 size = entry.state.logicalSize;
            logicalSize = FloaterLogicalSize{size.x, size.y};
        }
        writePlacement(entry.componentKey, DetachedFloaterPlacement{rect.x, rect.y, logicalSize, floater->minimized()},
                       floater->closed() ? ComponentOpenState::Closed : ComponentOpenState::Open);
    }

    void notifyAttachedPlacement(const ComponentKey& componentKey, const Floater& floater) {
        if (!writePlacement) return;
        std::optional<FloaterLogicalSize> size;
        if (floater.canResize()) size = FloaterLogicalSize{floater.rect().w, floater.rect().h};
        writePlacement(componentKey, AttachedFloaterPlacement{floater.rect().x, floater.rect().y, size, floater.minimized()},
                       floater.closed() ? ComponentOpenState::Closed : ComponentOpenState::Open);
    }

    void restoreAttached(std::unique_ptr<Floater> floater, const Rect& rect) {
        if (!floater) LL_ERRS("UI") << "Detached Floater presentation factory failed without returning the transferred Floater." << LL_ENDL;
        floater->setRect(rect);
        surface.mountFloater(std::move(floater));
    }

    void detachPendingFloater() {
        if (!pending || !pending->floater || findDetachedEntry(*pending->floater)) {
            pending.reset();
            return;
        }

        Floater& floater = *pending->floater;
        const Rect desired{pending->desired.x, pending->desired.y, floater.rect().w, floater.rect().h};
        const Rect attachedRect = floater.rect();
        const std::optional<AuxiliaryScreenPoint> dragScreenPoint = environment.releasePointerForDetach(pending->desired + pending->dragOffset);
        std::unique_ptr<Floater> transferred = surface.unmountFloater(floater);
        if (!transferred) {
            pending.reset();
            return;
        }

        DetachedFloaterPresentationResult creation = makePresentation(std::move(transferred));
        if (!creation) {
            std::unique_ptr<Floater> returnedFloater = std::move(creation).takeReturnedFloater();
            if (!returnedFloater)
                LL_ERRS("UI") << "Detached Floater presentation factory consumed a Floater without returning it on failure." << LL_ENDL;
            restoreAttached(std::move(returnedFloater), attachedRect);
        } else {
            std::unique_ptr<DetachedFloaterPresentation> presentation = std::move(creation).takePresentation();
            const std::optional<DetachedFloaterPresentationUpdate> initial = presentation->open(
                {environment.mainRectToNative(desired), environment.nativeScaleMultiplier(), pending->dragOffset, std::nullopt, dragScreenPoint});
            if (!initial) {
                std::unique_ptr<Floater> restored = presentation->releaseFloater();
                restoreAttached(std::move(restored), attachedRect);
            } else {
                DetachedEntry entry{pending->componentKey, &floater, std::move(presentation)};
                entry.state = *initial;
                notifyDetachedPlacement(entry);
                entry.presentation->setVisible(visible);
                detached.push_back(std::move(entry));
            }
        }
        pending.reset();
    }

    void reattach(std::size_t index, bool preserveMinimized, ReattachMode mode) {
        DetachedEntry& entry = detached[index];
        const AuxiliaryWindowRect nativeRect = entry.state.nativeRect;
        std::unique_ptr<Floater> floater = entry.presentation->releaseFloater();
        if (!floater) {
            detached.erase(detached.begin() + index);
            return;
        }

        const bool minimized = floater->minimized();
        if (minimized) floater->setMinimized(false);
        const Vec2 position = environment.nativeBottomLeftInMain(nativeRect);
        const Vec2 size = floater->canResize() ? Vec2{floater->rect().w, floater->rect().h} : surface.preferredFloaterSize(*floater);
        floater->setRect({position.x, position.y, size.x, size.y});
        Floater& mounted = surface.mountFloater(std::move(floater));
        if (minimized && preserveMinimized) mounted.setMinimized(true);
        if (mode == ReattachMode::PersistPlacement) notifyAttachedPlacement(entry.componentKey, mounted);
        detached.erase(detached.begin() + index);
    }

    void processTransitions() {
        for (std::size_t index = 0; index < detached.size();) {
            DetachedEntry& entry = detached[index];
            if (entry.state.closeRequested) {
                reattach(index, false, ReattachMode::PersistPlacement);
                continue;
            }
            if (entry.state.minimizeRequested) {
                reattach(index, true, ReattachMode::PersistPlacement);
                continue;
            }
            if (entry.state.dragEnded && environment.nativePointInsideMain(entry.state.headerCenterScreen)) {
                reattach(index, false, ReattachMode::PersistPlacement);
                continue;
            }
            if (entry.state.dragEnded || entry.state.resizeEnded) notifyDetachedPlacement(entry);
            ++index;
        }
    }

    Surface& surface;
    PresentationFactory makePresentation;
    DetachedFloaterEnvironment& environment;
    PlacementWriter writePlacement;
    std::optional<PendingDetach> pending;
    std::vector<DetachedEntry> detached;
    bool visible = true;
};

DetachedFloaterManager::DetachedFloaterManager(Surface& attachedSurface, PresentationFactory presentationFactory,
                                               DetachedFloaterEnvironment& environment, PlacementWriter placementWriter)
    : mImpl(std::make_unique<Impl>(attachedSurface, std::move(presentationFactory), environment, std::move(placementWriter))) {}

DetachedFloaterManager::~DetachedFloaterManager() = default;

void DetachedFloaterManager::requestDetach(const ComponentKey& componentKey, Floater& floater, const Vec2& desired, const Vec2& dragOffset) {
    mImpl->pending = Impl::PendingDetach{componentKey, &floater, desired, dragOffset};
}

void DetachedFloaterManager::processPendingDetachment() {
    mImpl->detachPendingFloater();
}

void DetachedFloaterManager::update() {
    for (auto& entry : mImpl->detached) entry.state = entry.presentation->update();
    mImpl->processTransitions();
}

void DetachedFloaterManager::reattachAll(ReattachMode mode) {
    mImpl->pending.reset();
    while (!mImpl->detached.empty()) mImpl->reattach(mImpl->detached.size() - 1, false, mode);
}

void DetachedFloaterManager::setVisible(bool visible) {
    if (mImpl->visible == visible) return;
    mImpl->visible = visible;
    for (auto& entry : mImpl->detached) entry.presentation->setVisible(visible);
}

bool DetachedFloaterManager::beginResize(Floater& floater) {
    Impl::DetachedEntry* entry = mImpl->findDetachedEntry(floater);
    return entry && entry->presentation->beginResize();
}

void DetachedFloaterManager::applyResize(Floater& floater, const Rect& logicalRect) {
    if (Impl::DetachedEntry* entry = mImpl->findDetachedEntry(floater)) entry->presentation->applyResize(logicalRect);
}

bool DetachedFloaterManager::isDetached(const Floater& floater) const {
    return mImpl->findDetachedEntry(floater) != nullptr;
}

Vec2 DetachedFloaterManager::logicalSize(const Floater& floater) const {
    const Impl::DetachedEntry* entry = mImpl->findDetachedEntry(floater);
    return entry ? entry->state.logicalSize : Vec2{};
}

std::optional<Rect> DetachedFloaterManager::prepareReplacement(Floater& current, Floater& replacement) {
    Impl::DetachedEntry* entry = mImpl->findDetachedEntry(current);
    if (!entry || !entry->presentation) return std::nullopt;
    return entry->presentation->prepareReplacement(replacement);
}

DetachedFloaterManager::Replacement DetachedFloaterManager::replace(Floater& current, std::unique_ptr<Floater> replacement,
                                                                    const std::optional<Vec2>& logicalSize) {
    Impl::DetachedEntry* entry = mImpl->findDetachedEntry(current);
    if (!entry || current.closed() || !replacement) return Replacement::failure();
    const bool minimized = current.minimized();
    Floater* replacementFloater = replacement.get();
    std::unique_ptr<Floater> retired = entry->presentation->replaceFloater(std::move(replacement), logicalSize);
    if (!retired) return Replacement::failure();
    Floater* mounted = replacementFloater;
    if (minimized && mounted->canMinimize()) mounted->setMinimized(true);
    entry->presentation->setVisible(mImpl->visible);
    entry->floater = mounted;
    entry->state = entry->presentation->update();
    mImpl->notifyDetachedPlacement(*entry);
    return Replacement::success(std::move(retired), mounted);
}

bool DetachedFloaterManager::restoreDetachedPlacement(const ComponentKey& componentKey, Floater& floater, const DetachedFloaterPlacement& placement) {
    const float scale = std::max(0.25f, mImpl->environment.nativeScaleMultiplier());
    const Vec2 fallbackSize{floater.rect().w, floater.rect().h};
    const Vec2 logicalSize = placement.size ? Vec2{placement.size->width, placement.size->height} : fallbackSize;
    const AuxiliaryWindowRect native{placement.x, placement.y, std::max(1, static_cast<int>(std::lround(logicalSize.x * scale))),
                                     std::max(1, static_cast<int>(std::lround(logicalSize.y * scale)))};
    if (native.width <= 0 || native.height <= 0 || !mImpl->environment.placementVisible(native)) return false;

    const Rect attachedRect = floater.rect();
    std::unique_ptr<Floater> transferred = mImpl->surface.unmountFloater(floater);
    if (!transferred) return false;
    DetachedFloaterPresentationResult creation = mImpl->makePresentation(std::move(transferred));
    if (!creation) {
        std::unique_ptr<Floater> returnedFloater = std::move(creation).takeReturnedFloater();
        if (!returnedFloater) LL_ERRS("UI") << "Detached Floater presentation factory consumed a Floater without returning it on failure." << LL_ENDL;
        mImpl->restoreAttached(std::move(returnedFloater), attachedRect);
        return false;
    }

    std::unique_ptr<DetachedFloaterPresentation> presentation = std::move(creation).takePresentation();

    Floater* detachedFloater = &floater;
    const std::optional<Vec2> requestedSize = floater.canResize() && placement.size ? std::optional<Vec2>(logicalSize) : std::nullopt;
    const std::optional<DetachedFloaterPresentationUpdate> initial =
        presentation->open({native, mImpl->environment.nativeScaleMultiplier(), std::nullopt, requestedSize, std::nullopt});
    if (initial) {
        if (placement.minimized && detachedFloater->canMinimize()) detachedFloater->setMinimized(true);
        Impl::DetachedEntry entry{componentKey, detachedFloater, std::move(presentation)};
        entry.state = *initial;
        mImpl->notifyDetachedPlacement(entry);
        entry.presentation->setVisible(mImpl->visible);
        mImpl->detached.push_back(std::move(entry));
        return true;
    }

    std::unique_ptr<Floater> restored = presentation->releaseFloater();
    mImpl->restoreAttached(std::move(restored), attachedRect);
    return false;
}
} // namespace radia::viewer::ui
