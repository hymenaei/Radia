/**
 * @file test_floater_host.h
 * @brief Provides a test host for viewer-owned Floaters.
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

#ifndef RD_TEST_FLOATER_HOST_H
#define RD_TEST_FLOATER_HOST_H

#include <map>
#include <memory>
#include <utility>
#include <vector>
#include "componentmanager.h"
#include "widgets/floater.h"

namespace radia::viewer::ui::test {
struct TestFloaterHost final : ComponentManager::Host {
    using ReplacementRequest = ComponentManager::Host::ReplacementRequest;

    void mount(std::unique_ptr<radia::ui::Floater> root) override {
        radia::ui::Floater* result = root.get();
        mounted.emplace(result, std::move(root));
    }

    std::unique_ptr<radia::ui::Floater> unmount(radia::ui::Floater& root) override {
        const auto found = mounted.find(&root);
        if (found == mounted.end()) return {};
        std::unique_ptr<radia::ui::Floater> result = std::move(found->second);
        mounted.erase(found);
        return result;
    }

    bool replaceAll(std::vector<ReplacementRequest> requests) override { return commitReplacement(std::move(requests)); }

    bool clearAll(std::vector<radia::ui::Floater*> roots) override {
        if (rejectClears) return false;
        for (radia::ui::Floater* root : roots)
            if (!root || mounted.find(root) == mounted.end()) return false;
        return commitClear(std::move(roots));
    }

    void present(radia::ui::Floater& root) override {
        root.open();
        ++presentations;
    }

    std::map<radia::ui::Floater*, std::unique_ptr<radia::ui::Floater>> mounted;
    int replacements = 0;
    int presentations = 0;
    bool rejectReplacements = false;
    bool failCommit = false;
    bool failAfterFirst = false;
    bool rejectClears = false;
    bool rollbackInvariantViolated = false;

private:
    bool commitReplacement(std::vector<ReplacementRequest> requests) {
        if (failCommit || rejectReplacements) return false;

        for (const auto& request : requests)
            if (mounted.find(request.current) == mounted.end() || !request.replacement) return false;

        std::vector<Applied> applied;
        applied.reserve(requests.size());

        for (auto& request : requests) {
            if (failAfterFirst && !applied.empty()) {
                rollback(applied);
                return false;
            }

            const auto found = mounted.find(request.current);
            if (found == mounted.end()) {
                rollback(applied);
                return false;
            }

            radia::ui::Floater* root = request.replacement.get();
            const bool closed = request.current->closed();
            std::unique_ptr<radia::ui::Floater> retired = std::move(found->second);
            mounted.erase(found);
            mounted.emplace(root, std::move(request.replacement));
            if (closed) root->close();
            applied.push_back({request.current, root, std::move(retired)});
            ++replacements;
        }

        return true;
    }

    bool commitClear(std::vector<radia::ui::Floater*> roots) {
        for (radia::ui::Floater* root : roots)
            if (!root || mounted.find(root) == mounted.end()) return false;
        for (radia::ui::Floater* root : roots) {
            if (!root->closed()) root->close();
            mounted.erase(root);
        }
        return true;
    }

    struct Applied {
        radia::ui::Floater* current;
        radia::ui::Floater* installed;
        std::unique_ptr<radia::ui::Floater> retired;
    };

    void rollback(std::vector<Applied>& applied) {
        for (auto current = applied.rbegin(); current != applied.rend(); ++current) {
            const auto restored = mounted.find(current->installed);
            if (restored == mounted.end()) {
                rollbackInvariantViolated = true;
                continue;
            }
            std::unique_ptr<radia::ui::Floater> retired = std::move(current->retired);
            mounted.erase(restored);
            mounted.emplace(current->current, std::move(retired));
            --replacements;
        }
    }
};
} // namespace radia::viewer::ui::test
#endif // RD_TEST_FLOATER_HOST_H
