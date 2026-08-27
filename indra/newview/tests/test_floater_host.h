/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <map>
#include <utility>
#include <vector>
#include "componentmanager.h"
#include "elements/document.h"
#include "elements/elementinternal.h"
#include "elements/floater.h"
#include "surface/surface.h"

namespace radia::viewer::ui::test {
struct TestFloaterHost final : ComponentManager::Host {
    using ReplacementRequest = ComponentManager::Host::ReplacementRequest;

    void mount(radia::ui::Document& document) override {
        radia::ui::FloaterElement* result =
            document.documentElement() ? dynamic_cast<radia::ui::FloaterElement*>(document.documentElement()) : nullptr;
        if (!result) return;
        if (surface) surface->mountFloater(document);
        mounted.emplace(result, radia::ui::ElementRef<radia::ui::FloaterElement>(result));
    }

    bool unmount(radia::ui::FloaterElement& root) override {
        const auto found = mounted.find(&root);
        if (found == mounted.end()) return false;
        if (surface && !surface->unmountBorrowedFloater(root)) return false;
        mounted.erase(found);
        return true;
    }

    bool replaceAll(std::vector<ReplacementRequest> requests) override { return commitReplacement(std::move(requests)); }

    bool clearAll(std::vector<radia::ui::FloaterElement*> roots) override {
        ++clearCalls;
        if (rejectClears) return false;
        for (radia::ui::FloaterElement* root : roots)
            if (!root || mounted.find(root) == mounted.end()) return false;
        return commitClear(std::move(roots));
    }

    void present(radia::ui::FloaterElement& root) override {
        root.open();
        ++presentations;
    }

    std::map<radia::ui::FloaterElement*, radia::ui::ElementRef<radia::ui::FloaterElement>> mounted;
    int replacements = 0;
    int presentations = 0;
    int clearCalls = 0;
    bool rejectReplacements = false;
    bool failCommit = false;
    bool rejectClears = false;
    radia::ui::Surface* surface = nullptr;

private:
    bool commitReplacement(std::vector<ReplacementRequest> requests) {
        if (failCommit || rejectReplacements) return false;

        std::vector<std::pair<radia::ui::FloaterElement*, radia::ui::FloaterElement*>> validated;
        validated.reserve(requests.size());
        for (const auto& request : requests) {
            if (mounted.find(request.current) == mounted.end() || !request.replacement || !request.replacement->documentElement()) return false;
            radia::ui::FloaterElement* root = dynamic_cast<radia::ui::FloaterElement*>(request.replacement->documentElement());
            if (!root) return false;
            if (surface && (!surface->ownsFloater(*request.current) || surface->ownsFloater(*root))) return false;
            validated.emplace_back(request.current, root);
        }

        for (const auto& [current, root] : validated) {
            const bool closed = current->closed();
            if (surface && !surface->replaceFloater(*current, *root)) return false;
            mounted.erase(current);
            mounted.emplace(root, radia::ui::ElementRef<radia::ui::FloaterElement>(root));
            if (closed) root->close();
            ++replacements;
        }

        return true;
    }

    bool commitClear(std::vector<radia::ui::FloaterElement*> roots) {
        for (radia::ui::FloaterElement* root : roots)
            if (!root || mounted.find(root) == mounted.end() || (surface && !surface->ownsFloater(*root))) return false;
        for (radia::ui::FloaterElement* root : roots) {
            if (!root->closed()) root->close();
            if (surface && !surface->unmountBorrowedFloater(*root)) return false;
            mounted.erase(root);
        }
        return true;
    }
};
} // namespace radia::viewer::ui::test
