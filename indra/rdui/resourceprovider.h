/**
 * @file resourceprovider.h
 * @brief Defines resource layers, dependency snapshots, and the UI resource-provider interface.
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

#ifndef RD_RESOURCEPROVIDER_H
#define RD_RESOURCEPROVIDER_H

#include <map>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace rdui {
using ResourceDependencyMap = std::map<std::string, std::set<std::string>>;

struct ResourceLayer {
    std::string sourceName;
    std::string source;
    std::string entrypoint;
    std::map<std::string, std::string> modules;

    std::string sourceNameFor(std::string_view resourceId) const {
        if (resourceId == entrypoint || entrypoint.empty()) return sourceName;
        if (sourceName.size() >= entrypoint.size()
            && sourceName.compare(sourceName.size() - entrypoint.size(), entrypoint.size(), entrypoint) == 0) {
            return sourceName.substr(0, sourceName.size() - entrypoint.size()) + std::string(resourceId);
        }
        return std::string(resourceId);
    }

    bool operator==(const ResourceLayer& other) const = default;
};

class ResourceProvider {
public:
    virtual ~ResourceProvider() = default;

    virtual std::optional<std::string> load(const std::string& resourceId) const = 0;

    virtual std::vector<std::string> list(const std::string& prefix) const { return {}; }
};

class ResourceSnapshot final : public ResourceProvider {
public:
    ResourceSnapshot() = default;
    explicit ResourceSnapshot(std::map<std::string, std::string> resources) : mResources(std::move(resources)) {}

    void add(std::string resourceId, std::string source) { mResources.insert_or_assign(std::move(resourceId), std::move(source)); }

    void setLayers(std::string resourceId, std::vector<ResourceLayer> layers) {
        mLayers.insert_or_assign(std::move(resourceId), std::move(layers));
    }

    std::optional<std::string> load(const std::string& resourceId) const override {
        const auto found = mResources.find(resourceId);
        return found == mResources.end() ? std::nullopt : std::optional<std::string>(found->second);
    }

    std::vector<std::string> list(const std::string& prefix) const override {
        std::vector<std::string> result;
        const std::string directory = prefix.empty() ? std::string() : prefix + "/";
        for (const auto& resource : mResources)
            if (directory.empty() || resource.first.rfind(directory, 0) == 0) result.push_back(resource.first);
        return result;
    }

    const std::map<std::string, std::string>& resources() const { return mResources; }
    const std::vector<ResourceLayer>& layers(const std::string& resourceId) const {
        static const std::vector<ResourceLayer> empty;
        const auto found = mLayers.find(resourceId);
        return found == mLayers.end() ? empty : found->second;
    }
    const std::map<std::string, std::vector<ResourceLayer>>& layeredResources() const { return mLayers; }

private:
    std::map<std::string, std::string> mResources;
    std::map<std::string, std::vector<ResourceLayer>> mLayers;
};
} // namespace rdui
#endif // RD_RESOURCEPROVIDER_H
