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
    FloaterReplacement(Surface& attachedSurface, std::vector<ReplacementRequest> replacements) : mAttachedSurface(attachedSurface) {
        mPlanned.reserve(replacements.size());
        for (auto& request : replacements) {
            Floater* current = request.current;
            Floater* candidate = request.replacement.get();
            if (!current || !candidate) return;

            if (!mAttachedSurface.ownsFloater(*current)) return;

            const bool wasMinimized = current->minimized();
            const std::optional<Rect> authoredRect = mAttachedSurface.prepareFloater(*candidate);
            if (!authoredRect) return;
            const Vec2 authoredSize{authoredRect->w, authoredRect->h};
            Rect replacementRect = wasMinimized ? current->expandedRect() : current->rect();
            const bool preserveSize = radia::ui::detail::preserveUserResizeOnReload(current->canResize(), candidate->canResize(),
                                                                                    {current->authoredSize(), current->authoredContentSize()},
                                                                                    {authoredSize, candidate->authoredContentSize()});
            if (!preserveSize) {
                replacementRect.w = authoredSize.x;
                replacementRect.h = authoredSize.y;
            }
            mPlanned.push_back({std::move(request), wasMinimized, replacementRect});
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
        bool wasMinimized = false;
        Rect replacementRect;
    };

    struct AppliedReplacement {
        std::size_t index = 0;
        Floater* installed = nullptr;
        std::unique_ptr<Floater> retired;
    };

    bool replaceOne(std::size_t index, AppliedReplacement& applied) {
        PlannedReplacement& replacement = mPlanned[index];
        ReplacementRequest& request = replacement.request;
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
        applied = {index, mounted, std::move(retired)};
        return true;
    }

    bool rollback() {
        for (auto current = mApplied.rbegin(); current != mApplied.rend(); ++current) {
            const PlannedReplacement& replacement = mPlanned[current->index];
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
        mAttachedSurface.updateLayout();
        mFinalized = true;
        return true;
    }

    void rollbackOrDie() {
        if (!rollback()) LL_ERRS("UI") << "Component Floater replacement could not be rolled back." << LL_ENDL;
    }

    Surface& mAttachedSurface;
    std::vector<PlannedReplacement> mPlanned;
    std::vector<AppliedReplacement> mApplied;
    bool mValid = false;
    bool mFinalized = false;
};
} // namespace

FloaterHost::FloaterHost(Surface& attachedSurface) : mAttachedSurface(attachedSurface) {}

void FloaterHost::mount(std::unique_ptr<Floater> root) {
    mAttachedSurface.mountFloater(std::move(root));
}

std::unique_ptr<Floater> FloaterHost::unmount(Floater& root) {
    return mAttachedSurface.unmountFloater(root);
}

bool FloaterHost::replaceAll(std::vector<ReplacementRequest> replacements) {
    FloaterReplacement transaction(mAttachedSurface, std::move(replacements));
    return transaction.commit();
}

bool FloaterHost::clearAll(std::vector<Floater*> roots) {
    for (Floater* root : roots)
        if (!root || !mAttachedSurface.ownsFloater(*root)) return false;
    for (Floater* root : roots) {
        if (!root->closed()) root->close();
        std::unique_ptr<Floater> retired = mAttachedSurface.unmountFloater(*root);
        if (!retired || retired.get() != root) LL_ERRS("UI") << "Component host lost a Floater during account teardown." << LL_ENDL;
    }
    return true;
}

void FloaterHost::present(Floater& root) {
    root.open();
    mAttachedSurface.raise(root);
}
} // namespace radia::viewer::ui
