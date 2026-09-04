/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <functional>
#include <map>
#include <set>
#include <utility>
#include <vector>
#include "componentmanager.h"
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "html/floater.h"
#include "surface/surface.h"

namespace radia::viewer::ui::test {
using radia::ui::Document;
using radia::ui::ElementRef;
using radia::ui::HTMLFloaterElement;
using radia::ui::Surface;

struct TestFloaterHost final : ComponentManager::Host {
    using ReplacementRequest = ComponentManager::Host::ReplacementRequest;

    bool mount(Document& document) override {
        HTMLFloaterElement* result = document.documentElement() ? dynamic_cast<HTMLFloaterElement*>(document.documentElement()) : nullptr;
        if (!result) return false;
        if (surface) surface->mountFloater(document);
        mounted.emplace(result, ElementRef<HTMLFloaterElement>(result));
        if (onMount) onMount(*result);
        return true;
    }

    bool unmount(HTMLFloaterElement& root) override {
        if (rejectUnmounts) return false;
        const auto found = mounted.find(&root);
        if (found == mounted.end()) return false;
        if (surface && !surface->unmountBorrowedFloater(root)) return false;
        mounted.erase(found);
        return true;
    }

    bool replaceAll(std::vector<ReplacementRequest> requests) override { return commitReplacement(std::move(requests)); }

    bool clearAll(std::vector<HTMLFloaterElement*> roots) override {
        ++clearCalls;
        if (rejectClears) return false;
        for (HTMLFloaterElement* root : roots)
            if (!root || mounted.find(root) == mounted.end()) return false;
        return commitClear(std::move(roots));
    }

    void present(HTMLFloaterElement& root) override {
        root.open();
        ++presentations;
    }

    std::map<HTMLFloaterElement*, ElementRef<HTMLFloaterElement>> mounted;
    int replacements = 0;
    int presentations = 0;
    int clearCalls = 0;
    bool rejectReplacements = false;
    bool failCommit = false;
    bool rejectClears = false;
    bool rejectUnmounts = false;
    std::function<void(HTMLFloaterElement&)> onMount;
    std::function<void(HTMLFloaterElement&)> afterReplacement;
    Surface* surface = nullptr;

private:
    struct AppliedReplacement {
        HTMLFloaterElement* current = nullptr;
        HTMLFloaterElement* installed = nullptr;
    };

    bool commitReplacement(std::vector<ReplacementRequest> requests) {
        if (failCommit || rejectReplacements) return false;

        std::vector<ReplacementRequest*> validated;
        validated.reserve(requests.size());
        std::set<HTMLFloaterElement*> currents;
        std::set<HTMLFloaterElement*> candidates;
        for (ReplacementRequest& request : requests) {
            if (mounted.find(request.current) == mounted.end() || !request.replacement || !request.replacement->documentElement()) return false;
            HTMLFloaterElement* root = dynamic_cast<HTMLFloaterElement*>(request.replacement->documentElement());
            if (!root) return false;
            if (surface && (!surface->ownsFloater(*request.current) || surface->ownsFloater(*root))) return false;
            if (!currents.emplace(request.current).second || !candidates.emplace(root).second) return false;
            validated.push_back(&request);
        }

        std::vector<AppliedReplacement> applied;
        applied.reserve(validated.size());
        for (ReplacementRequest* request : validated) {
            HTMLFloaterElement* current = request->current;
            HTMLFloaterElement* root = dynamic_cast<HTMLFloaterElement*>(request->replacement->documentElement());
            const bool closed = current->closed();
            if (surface) {
                if (!surface->replaceFloater(*current, *root)) {
                    if (!rollbackReplacement(applied)) return false;
                    return false;
                }
            }
            mounted.erase(current);
            mounted.emplace(root, ElementRef<HTMLFloaterElement>(root));
            if (afterReplacement) afterReplacement(*root);
            if (closed) root->close();
            applied.push_back({current, root});
            ++replacements;
        }

        return true;
    }

    bool rollbackReplacement(std::vector<AppliedReplacement>& applied) {
        for (auto current = applied.rbegin(); current != applied.rend(); ++current) {
            if (!current->current || !current->installed) return false;
            if (surface) {
                if (!surface->replaceFloater(*current->installed, *current->current)) return false;
            }
            mounted.erase(current->installed);
            mounted.emplace(current->current, ElementRef<HTMLFloaterElement>(current->current));
        }
        replacements -= static_cast<int>(applied.size());
        return true;
    }

    bool commitClear(std::vector<HTMLFloaterElement*> roots) {
        for (HTMLFloaterElement* root : roots)
            if (!root || mounted.find(root) == mounted.end() || (surface && !surface->ownsFloater(*root))) return false;
        for (HTMLFloaterElement* root : roots) {
            if (!root->closed()) root->close();
            if (surface && !surface->unmountBorrowedFloater(*root)) return false;
            mounted.erase(root);
        }
        return true;
    }
};
} // namespace radia::viewer::ui::test
