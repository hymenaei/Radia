/**
 * @file rduidetachedfloatermanager.cpp
 * @brief
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
#include "rduidetachedfloatermanager.h"
#include <algorithm>
#include <utility>
#include <vector>
#include "rdfloater.h"
#include "rduisurface.h"

namespace rdui::viewer {
struct DetachedFloaterManager::Impl {
    struct DetachedEntry {
        FloaterInstanceId identity;
        std::unique_ptr<DetachedFloaterPresentation> presentation;
    };

    struct PendingDetach {
        FloaterInstanceId identity;
        Floater* floater = nullptr;
        Vec2 desired;
        Vec2 dragOffset;
    };

    Impl(Surface& attached_surface, FloaterPlacementStore& placement_store, PresentationFactory presentation_factory, Environment& environment)
        : surface(attached_surface), placements(placement_store), makePresentation(std::move(presentation_factory)), environment(environment) {}

    DetachedEntry* owner(const Floater& floater) {
        const auto found = std::find_if(detached.begin(), detached.end(),
                                        [&floater](const DetachedEntry& entry) { return entry.presentation->floater() == &floater; });
        return found == detached.end() ? nullptr : &*found;
    }

    const DetachedEntry* owner(const Floater& floater) const {
        const auto found = std::find_if(detached.begin(), detached.end(),
                                        [&floater](const DetachedEntry& entry) { return entry.presentation->floater() == &floater; });
        return found == detached.end() ? nullptr : &*found;
    }

    void saveAttached(const FloaterInstanceId& identity, const Floater& floater) {
        std::optional<FloaterPlacementSize> size;
        if (floater.canResize()) size = FloaterPlacementSize{floater.rect().w, floater.rect().h};
        placements.save(identity, AttachedFloaterPlacement{floater.rect().x, floater.rect().y, size});
    }

    void saveDetached(const DetachedEntry& entry) {
        const DetachedFloaterPresentation& presentation = *entry.presentation;
        const Floater* floater = presentation.floater();
        if (!floater) return;
        const NativeRect rect = presentation.nativeRect();
        std::optional<FloaterPlacementSize> logical_size;
        if (floater->canResize()) logical_size = FloaterPlacementSize{presentation.logicalSize().x, presentation.logicalSize().y};
        placements.save(entry.identity, DetachedFloaterPlacement{rect.x, rect.y, rect.width, rect.height, presentation.monitorId(), logical_size});
    }

    void detachPendingFloater() {
        if (!pending || !pending->floater || owner(*pending->floater)) {
            pending.reset();
            return;
        }

        Floater& floater = *pending->floater;
        const Rect desired{pending->desired.x, pending->desired.y, floater.rect().w, floater.rect().h};
        const Rect attached_rect = floater.rect();
        const std::optional<NativePoint> drag_cursor = environment.releasePointerForDetach(pending->desired + pending->dragOffset);
        std::unique_ptr<Floater> transferred = surface.unmountFloater(floater);
        if (!transferred) {
            pending.reset();
            return;
        }

        std::unique_ptr<DetachedFloaterPresentation> presentation = makePresentation(transferred);
        if (!presentation) {
            if (transferred) {
                transferred->setRect(attached_rect);
                surface.mountFloater(std::move(transferred));
            }
        } else if (!presentation->open(environment.mainRectToNative(desired), environment.nativeScaleMultiplier(), pending->dragOffset, std::nullopt,
                                       drag_cursor)) {
            std::unique_ptr<Floater> restored = presentation->releaseFloater();
            if (restored) {
                restored->setRect(attached_rect);
                surface.mountFloater(std::move(restored));
            }
        } else {
            DetachedEntry entry{pending->identity, std::move(presentation)};
            saveDetached(entry);
            entry.presentation->setVisible(visible);
            detached.push_back(std::move(entry));
        }
        pending.reset();
    }

    void reattach(std::size_t index, bool preserve_minimized) {
        DetachedEntry& entry = detached[index];
        DetachedFloaterPresentation& presentation = *entry.presentation;
        const NativeRect native_rect = presentation.nativeRect();
        std::unique_ptr<Floater> floater = presentation.releaseFloater();
        if (!floater) {
            detached.erase(detached.begin() + index);
            return;
        }

        const bool minimized = floater->minimized();
        if (minimized) floater->setMinimized(false);
        const Vec2 position = environment.nativeBottomLeftInMain(native_rect);
        const Vec2 size = floater->canResize() ? Vec2{floater->rect().w, floater->rect().h} : surface.preferredFloaterSize(*floater);
        floater->setRect({position.x, position.y, size.x, size.y});
        Floater& mounted = surface.mountFloater(std::move(floater));
        saveAttached(entry.identity, mounted);
        if (minimized && preserve_minimized) mounted.setMinimized(true);
        detached.erase(detached.begin() + index);
    }

    void processTransitions() {
        for (std::size_t index = 0; index < detached.size();) {
            DetachedEntry& entry = detached[index];
            DetachedFloaterPresentation& presentation = *entry.presentation;
            if (presentation.closeRequested()) {
                reattach(index, false);
                continue;
            }
            if (presentation.minimizeRequested()) {
                reattach(index, true);
                continue;
            }
            const bool drag_ended = presentation.takeDragEnded();
            if (drag_ended && environment.nativePointInsideMain(presentation.headerCenterScreen())) {
                reattach(index, false);
                continue;
            }
            if (drag_ended) saveDetached(entry);
            if (presentation.takeResizeEnded()) saveDetached(entry);
            ++index;
        }
    }

    Surface& surface;
    FloaterPlacementStore& placements;
    PresentationFactory makePresentation;
    Environment& environment;
    std::optional<PendingDetach> pending;
    std::vector<DetachedEntry> detached;
    bool visible = true;
};

DetachedFloaterManager::DetachedFloaterManager(Surface& attached_surface, FloaterPlacementStore& placement_store,
                                               PresentationFactory presentation_factory, Environment& environment)
    : mImpl(std::make_unique<Impl>(attached_surface, placement_store, std::move(presentation_factory), environment)) {}

DetachedFloaterManager::~DetachedFloaterManager() = default;

void DetachedFloaterManager::requestDetach(const FloaterInstanceId& identity, Floater& floater, const Vec2& desired, const Vec2& drag_offset) {
    mImpl->pending = Impl::PendingDetach{identity, &floater, desired, drag_offset};
}

void DetachedFloaterManager::processPendingDetach() {
    mImpl->detachPendingFloater();
}

void DetachedFloaterManager::update() {
    for (auto& entry : mImpl->detached) entry.presentation->tick();
    mImpl->processTransitions();
}

void DetachedFloaterManager::setVisible(bool visible) {
    if (mImpl->visible == visible) return;
    mImpl->visible = visible;
    for (auto& entry : mImpl->detached) entry.presentation->setVisible(visible);
}

bool DetachedFloaterManager::beginResize(Floater& floater) {
    Impl::DetachedEntry* entry = mImpl->owner(floater);
    return entry && entry->presentation->beginResize();
}

void DetachedFloaterManager::applyResize(Floater& floater, const Rect& logical_rect) {
    if (Impl::DetachedEntry* entry = mImpl->owner(floater)) entry->presentation->applyResize(logical_rect);
}

bool DetachedFloaterManager::contains(const Floater& floater) const {
    return mImpl->owner(floater) != nullptr;
}

Vec2 DetachedFloaterManager::logicalSize(const Floater& floater) const {
    const Impl::DetachedEntry* entry = mImpl->owner(floater);
    return entry ? entry->presentation->logicalSize() : Vec2{};
}

Floater* DetachedFloaterManager::replace(Floater& current, std::unique_ptr<Floater> replacement, const std::optional<Vec2>& logical_size) {
    Impl::DetachedEntry* entry = mImpl->owner(current);
    if (!entry || !replacement) return nullptr;
    Floater& mounted = entry->presentation->replaceFloater(std::move(replacement), logical_size);
    mImpl->saveDetached(*entry);
    return &mounted;
}

bool DetachedFloaterManager::restore(const FloaterInstanceId& identity, Floater& floater, const DetachedFloaterPlacement& placement) {
    const NativeRect native{placement.x, placement.y, placement.width, placement.height};
    if (native.width <= 0 || native.height <= 0 || !mImpl->environment.placementVisible(native, placement.monitor)) return false;

    const Rect attached_rect = floater.rect();
    std::unique_ptr<Floater> transferred = mImpl->surface.unmountFloater(floater);
    if (!transferred) return false;
    std::unique_ptr<DetachedFloaterPresentation> presentation = mImpl->makePresentation(transferred);
    if (!presentation) {
        if (transferred) {
            transferred->setRect(attached_rect);
            mImpl->surface.mountFloater(std::move(transferred));
        }
        return false;
    }

    Floater* detached_floater = presentation->floater();
    Vec2 logical_size{detached_floater->rect().w, detached_floater->rect().h};
    if (detached_floater->canResize() && placement.logicalSize) logical_size = {placement.logicalSize->width, placement.logicalSize->height};
    if (presentation->open(native, mImpl->environment.nativeScaleMultiplier(), std::nullopt, logical_size)) {
        Impl::DetachedEntry entry{identity, std::move(presentation)};
        mImpl->saveDetached(entry);
        entry.presentation->setVisible(mImpl->visible);
        mImpl->detached.push_back(std::move(entry));
        return true;
    }

    std::unique_ptr<Floater> restored = presentation->releaseFloater();
    if (restored) {
        restored->setRect(attached_rect);
        mImpl->surface.mountFloater(std::move(restored));
    }
    return false;
}

void DetachedFloaterManager::savePlacement(const Floater& floater) {
    if (const Impl::DetachedEntry* entry = mImpl->owner(floater)) mImpl->saveDetached(*entry);
}
} // namespace rdui::viewer
