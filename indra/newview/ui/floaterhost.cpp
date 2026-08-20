/**
 * @file floaterhost.cpp
 * @brief Hosts viewer-owned Floaters for the component manager.
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

#include "linden_common.h"
#include "floaterhost.h"
#include <optional>
#include <utility>
#include "detachedfloatermanager.h"
#include "surface/floaterresize.h"
#include "surface/surface.h"
#include "widgets/floater.h"

namespace radia::viewer::ui {
using radia::ui::Floater;
using radia::ui::Rect;
using radia::ui::Surface;
using radia::ui::Vec2;

namespace {
class FloaterReplacement final {
public:
    using ReplacementRequest = ComponentManager::Host::ReplacementRequest;
    FloaterReplacement(Surface& attachedSurface, DetachedFloaterManager& detachedManager, std::vector<ReplacementRequest> replacements)
        : mAttachedSurface(attachedSurface), mDetachedManager(detachedManager) {
        mPlanned.reserve(replacements.size());
        for (auto& request : replacements) {
            Floater* current = request.current;
            Floater* candidate = request.replacement.get();
            if (!current || !candidate) return;

            const bool wasDetached = mDetachedManager.isDetached(*current);
            if (!wasDetached && !mAttachedSurface.ownsFloater(*current)) return;

            const bool wasMinimized = current->minimized();
            std::optional<Vec2> previousDetachedLogicalSize;
            if (wasDetached) previousDetachedLogicalSize = mDetachedManager.logicalSize(*current);
            std::optional<Rect> authoredRect;
            if (wasDetached) authoredRect = mDetachedManager.prepareReplacement(*current, *candidate);
            else authoredRect = mAttachedSurface.prepareFloater(*candidate);
            if (!authoredRect) return;
            const Vec2 authoredSize{authoredRect->w, authoredRect->h};
            Rect replacementRect = wasMinimized ? current->expandedRect() : current->rect();
            const bool preserveSize = radia::ui::detail::preserveUserResizeOnReload(current->canResize(), candidate->canResize(),
                                                                                    {current->authoredSize(), current->authoredContentSize()},
                                                                                    {authoredSize, candidate->authoredContentSize()});
            std::optional<Vec2> replacementLogicalSize;
            if (!preserveSize) {
                replacementLogicalSize = authoredSize;
                replacementRect.w = replacementLogicalSize->x;
                replacementRect.h = replacementLogicalSize->y;
            } else if (wasDetached) {
                replacementLogicalSize = previousDetachedLogicalSize;
            }
            mPlanned.push_back({std::move(request), wasDetached, wasMinimized, replacementRect, replacementLogicalSize, previousDetachedLogicalSize});
        }

        mValid = true;
    }

    ~FloaterReplacement() {
        if (!mFinalized && !mApplied.empty()) rollbackOrDie();
    }

    bool commit() {
        if (!mValid || mFinalized) return false;
        mApplied.reserve(mPlanned.size());
        for (std::size_t index = 0; index < mPlanned.size(); ++index) {
            AppliedReplacement applied;
            if (!replaceOne(index, applied)) {
                rollbackOrDie();
                return false;
            }
            mApplied.push_back(std::move(applied));
        }

        mFinalized = true;
        return true;
    }

private:
    struct PlannedReplacement {
        ReplacementRequest request;
        bool wasDetached = false;
        bool wasMinimized = false;
        Rect replacementRect;
        std::optional<Vec2> replacementLogicalSize;
        std::optional<Vec2> previousDetachedLogicalSize;
    };

    struct AppliedReplacement {
        std::size_t index = 0;
        Floater* installed = nullptr;
        std::unique_ptr<Floater> retired;
        bool wasDetached = false;
    };

    bool replaceOne(std::size_t index, AppliedReplacement& applied) {
        PlannedReplacement& replacement = mPlanned[index];
        ReplacementRequest& request = replacement.request;
        if (replacement.wasDetached) {
            if (!request.current || !request.replacement) return false;
            Floater* candidate = request.replacement.get();
            DetachedFloaterManager::Replacement detachedReplacement =
                mDetachedManager.replace(*request.current, std::move(request.replacement), replacement.replacementLogicalSize);
            if (!detachedReplacement || detachedReplacement.installed() != candidate) return false;
            applied = {index, detachedReplacement.installed(), std::move(detachedReplacement).takeRetired(), true};
            return true;
        }

        Floater* current = request.current;
        Floater* candidate = request.replacement.get();
        if (!current || !candidate) return false;
        candidate->setRect(replacement.replacementRect);
        Floater* mounted = candidate;
        std::unique_ptr<Floater> retired = mAttachedSurface.replaceFloater(*current, std::move(request.replacement));
        if (!retired) return false;

        mAttachedSurface.placeFloater(*mounted, replacement.replacementRect);
        if (replacement.wasMinimized && mounted->canMinimize()) mounted->setMinimized(true);
        mAttachedSurface.updateLayout();
        applied = {index, mounted, std::move(retired), false};
        return true;
    }

    bool rollback() {
        for (auto current = mApplied.rbegin(); current != mApplied.rend(); ++current) {
            const PlannedReplacement& replacement = mPlanned[current->index];
            if (current->wasDetached) {
                DetachedFloaterManager::Replacement restored =
                    mDetachedManager.replace(*current->installed, std::move(current->retired), replacement.previousDetachedLogicalSize);
                if (!restored || restored.installed() != replacement.request.current || restored.retired() != current->installed) {
                    mFinalized = true;
                    return false;
                }
            } else {
                if (!current->installed) {
                    mFinalized = true;
                    return false;
                }
                std::unique_ptr<Floater> restored = mAttachedSurface.replaceFloater(*current->installed, std::move(current->retired));
                if (!restored || restored.get() != current->installed || !replacement.request.current) {
                    mFinalized = true;
                    return false;
                }
            }
        }
        mAttachedSurface.updateLayout();
        mFinalized = true;
        return true;
    }

    void rollbackOrDie() {
        if (!rollback()) LL_ERRS("UI") << "Component Floater replacement could not be rolled back." << LL_ENDL;
    }

    Surface& mAttachedSurface;
    DetachedFloaterManager& mDetachedManager;
    std::vector<PlannedReplacement> mPlanned;
    std::vector<AppliedReplacement> mApplied;
    bool mValid = false;
    bool mFinalized = false;
};
} // namespace

FloaterHost::FloaterHost(Surface& attachedSurface, DetachedFloaterManager& detachedManager)
    : mAttachedSurface(attachedSurface), mDetachedManager(detachedManager) {}

void FloaterHost::mount(std::unique_ptr<Floater> root) {
    mAttachedSurface.mountFloater(std::move(root));
}

std::unique_ptr<Floater> FloaterHost::unmount(Floater& root) {
    if (mDetachedManager.isDetached(root)) return {};
    return mAttachedSurface.unmountFloater(root);
}

bool FloaterHost::replaceAll(std::vector<ReplacementRequest> replacements) {
    FloaterReplacement transaction(mAttachedSurface, mDetachedManager, std::move(replacements));
    return transaction.commit();
}

bool FloaterHost::clearAll(std::vector<Floater*> roots) {
    for (Floater* root : roots)
        if (!root || mDetachedManager.isDetached(*root) || !mAttachedSurface.ownsFloater(*root)) return false;
    for (Floater* root : roots) {
        if (!root->closed()) root->close();
        std::unique_ptr<Floater> retired = mAttachedSurface.unmountFloater(*root);
        if (!retired || retired.get() != root) LL_ERRS("UI") << "Component host lost a Floater during account teardown." << LL_ENDL;
    }
    return true;
}

void FloaterHost::present(Floater& root) {
    root.open();
    if (!mDetachedManager.isDetached(root)) mAttachedSurface.raise(root);
}
} // namespace radia::viewer::ui
