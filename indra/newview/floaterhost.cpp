/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "floaterhost.h"
#include <optional>
#include <utility>
#include "dom/document.h"
#include "html/floater.h"
#include "surface/floaterresize.h"
#include "surface/surface.h"

namespace radia::viewer::ui {
using radia::ui::Document;
using radia::ui::Element;
using radia::ui::HTMLFloaterElement;
using radia::ui::Rect;
using radia::ui::Surface;
using radia::ui::SurfaceLayer;
using radia::ui::Vec2;
using radia::ui::detail::preserveUserResizeOnReload;

namespace {
class FloaterReplacement final {
public:
    using ReplacementRequest = ComponentManager::Host::ReplacementRequest;
    FloaterReplacement(Surface& surface, std::vector<ReplacementRequest> replacements) : mSurface(surface) {
        mPlanned.reserve(replacements.size());
        for (auto& request : replacements) {
            HTMLFloaterElement* current = request.current;
            Document* document = request.replacement;
            Element* documentElement = document ? document->documentElement() : nullptr;
            HTMLFloaterElement* candidate = documentElement ? dynamic_cast<HTMLFloaterElement*>(documentElement) : nullptr;
            if (!current || !candidate) return;

            if (!mSurface.ownsFloater(*current)) return;

            const bool wasMinimized = current->minimized();
            const std::optional<Rect> authoredRect = mSurface.prepareFloater(*candidate);
            if (!authoredRect) return;
            const Vec2 authoredSize{authoredRect->w, authoredRect->h};
            Rect replacementRect = wasMinimized ? current->expandedRect() : current->rect();
            const bool preserveSize =
                preserveUserResizeOnReload(current->resizeable(), candidate->resizeable(), {current->authoredSize(), current->authoredContentSize()},
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
        if (!mFinalized && !mApplied.empty()) rollback();
    }

    bool commit() {
        if (!mValid || mFinalized) return false;
        mApplied.reserve(mPlanned.size());
        for (std::size_t index = 0; index < mPlanned.size(); ++index) {
            AppliedReplacement applied;
            if (!replaceOne(index, applied)) {
                rollback();
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
        HTMLFloaterElement* installed = nullptr;
    };

    bool replaceOne(std::size_t index, AppliedReplacement& applied) {
        PlannedReplacement& replacement = mPlanned[index];
        ReplacementRequest& request = replacement.request;
        HTMLFloaterElement* current = request.current;
        HTMLFloaterElement* candidate = request.replacement && request.replacement->documentElement()
            ? dynamic_cast<HTMLFloaterElement*>(request.replacement->documentElement())
            : nullptr;
        if (!current || !candidate) return false;
        candidate->setRect(replacement.replacementRect);
        HTMLFloaterElement* mounted = candidate;
        if (!mSurface.replaceFloater(*current, *candidate)) return false;

        mSurface.placeFloater(*mounted, replacement.replacementRect);
        if (replacement.wasMinimized && mounted->minimizable()) mounted->setMinimized(true);
        mSurface.updateLayout();
        applied.index = index;
        applied.installed = mounted;
        return true;
    }

    bool rollback() {
        for (auto current = mApplied.rbegin(); current != mApplied.rend(); ++current) {
            AppliedReplacement& applied = *current;
            const PlannedReplacement& replacement = mPlanned[applied.index];
            if (!applied.installed || !replacement.request.current || !mSurface.replaceFloater(*applied.installed, *replacement.request.current))
                LL_ERRS("UI") << "Component HTMLFloaterElement replacement violated its rollback contract." << LL_ENDL;
            applied.installed = nullptr;
        }
        mSurface.updateLayout();
        mFinalized = true;
        return true;
    }

    Surface& mSurface;
    std::vector<PlannedReplacement> mPlanned;
    std::vector<AppliedReplacement> mApplied;
    bool mValid = false;
    bool mFinalized = false;
};
} // namespace

FloaterHost::FloaterHost(Surface& surface) : mSurface(surface) {}

bool FloaterHost::mount(Document& document) {
    mSurface.mountFloater(document);
    return true;
}

bool FloaterHost::unmount(HTMLFloaterElement& root) {
    return mSurface.unmountBorrowedFloater(root);
}

bool FloaterHost::replaceAll(std::vector<ReplacementRequest> replacements) {
    FloaterReplacement transaction(mSurface, std::move(replacements));
    return transaction.commit();
}

bool FloaterHost::clearAll(std::vector<HTMLFloaterElement*> roots) {
    struct PendingRoot {
        HTMLFloaterElement* root = nullptr;
        Document* document = nullptr;
        bool wasClosed = false;
    };

    std::vector<PendingRoot> pending;
    pending.reserve(roots.size());
    for (HTMLFloaterElement* root : roots) {
        if (!root || !mSurface.ownsFloater(*root)) return false;
        Document* document = root->parentNode() ? root->parentNode()->asDocument() : nullptr;
        if (!document) return false;
        pending.push_back({root, document, root->closed()});
    }

    const auto restore = [&](const PendingRoot& entry) {
        bool restored = true;
        if (!mSurface.ownsFloater(*entry.root)) {
            mSurface.mount(*entry.document, SurfaceLayer::Floater);
            restored = mSurface.ownsFloater(*entry.root);
        }
        if (!entry.wasClosed && entry.root->closed()) entry.root->open();
        return restored;
    };

    std::vector<PendingRoot> detached;
    detached.reserve(pending.size());
    for (const PendingRoot& entry : pending) {
        if (!entry.wasClosed) entry.root->close();
        if (!mSurface.unmountBorrowedFloater(*entry.root)) {
            bool restored = restore(entry);
            for (auto detachedRoot = detached.rbegin(); detachedRoot != detached.rend(); ++detachedRoot)
                restored = restore(*detachedRoot) && restored;
            if (!restored) LL_ERRS("UI") << "Component host violated the clear rollback contract." << LL_ENDL;
            return false;
        }
        detached.push_back(entry);
    }
    return true;
}

void FloaterHost::present(HTMLFloaterElement& root) {
    root.open();
    mSurface.raise(root);
}
} // namespace radia::viewer::ui
