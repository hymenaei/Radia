/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <algorithm>
#include <map>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace radia::ui {
using ResourceDependencyMap = std::map<std::string, std::set<std::string>>;

class ResourceId final {
public:
    ResourceId() = default;
    explicit ResourceId(std::string_view raw) : mValue(canonicalize(raw)) {}

    bool valid() const { return !mValue.empty(); }
    const std::string& value() const { return mValue; }

    static ResourceId resolve(const ResourceId& base, std::string_view reference) {
        if (reference.empty()) return {};
        if (!reference.empty() && (reference.front() == '/' || reference.front() == '\\')) return ResourceId(reference);
        if (!base.valid()) return ResourceId(reference);

        const std::size_t slash = base.mValue.find_last_of('/');
        const std::string directory = slash == std::string::npos ? std::string() : base.mValue.substr(0, slash + 1);
        return ResourceId(directory + std::string(reference));
    }

    friend bool operator==(const ResourceId&, const ResourceId&) = default;
    friend bool operator<(const ResourceId& left, const ResourceId& right) { return left.mValue < right.mValue; }

private:
    static std::string canonicalize(std::string_view raw) {
        std::string resource(raw);
        std::replace(resource.begin(), resource.end(), '\\', '/');
        while (resource.rfind("./", 0) == 0) resource.erase(0, 2);
        if (resource.empty()) return {};

        std::vector<std::string> segments;
        std::size_t start = 0;
        while (start <= resource.size()) {
            const std::size_t slash = resource.find('/', start);
            const std::string segment = resource.substr(start, slash == std::string::npos ? std::string::npos : slash - start);
            if (segment == "..") {
                if (segments.empty()) return {};
                segments.pop_back();
            } else if (!segment.empty() && segment != ".") {
                segments.push_back(segment);
            }
            if (slash == std::string::npos) break;
            start = slash + 1;
        }

        std::string result;
        for (const std::string& segment : segments) {
            if (!result.empty()) result += '/';
            result += segment;
        }
        return result;
    }

    std::string mValue;
};

struct ResourceSource {
    std::string content;
    std::string provenance;

    bool operator==(const ResourceSource& other) const = default;
};

struct ResourceLayer {
    std::string provenance;
    std::string content;
    std::string entrypoint;
    std::map<std::string, std::string> modules;

    std::string provenanceFor(std::string_view id) const {
        if (id == entrypoint || entrypoint.empty()) return provenance;
        if (provenance.size() < entrypoint.size()) return std::string(id);
        const std::size_t suffixStart = provenance.size() - entrypoint.size();
        if (provenance.compare(suffixStart, entrypoint.size(), entrypoint) != 0 || (suffixStart != 0 && provenance[suffixStart - 1] != '/'))
            return std::string(id);
        return provenance.substr(0, suffixStart) + std::string(id);
    }

    bool operator==(const ResourceLayer& other) const = default;
};

class ResourceProvider {
public:
    virtual ~ResourceProvider() = default;

    virtual std::optional<ResourceSource> load(const ResourceId& id) const = 0;

    virtual ResourceId canonicalId(const ResourceId& id) const { return id; }
    virtual ResourceId resolve(const ResourceId& base, std::string_view reference) const { return ResourceId::resolve(base, reference); }
    virtual std::vector<ResourceId> list(const ResourceId&) const { return {}; }
};

class ResourceSnapshot final : public ResourceProvider {
public:
    ResourceSnapshot() = default;
    explicit ResourceSnapshot(std::map<std::string, std::string> resources) {
        for (auto& [id, content] : resources) add(std::move(id), std::move(content));
    }

    bool add(std::string rawId, std::string content, std::string provenance = {}) {
        return add(ResourceId(rawId), std::move(content), std::move(provenance));
    }

    bool add(ResourceId id, std::string content, std::string provenance = {}) {
        if (!id.valid()) return false;
        if (provenance.empty()) provenance = id.value();
        mResources.insert_or_assign(std::move(id), ResourceSource{std::move(content), std::move(provenance)});
        return true;
    }

    bool addPrefixAlias(std::string rawPhysicalPrefix, ResourceId logicalPrefix = {}) {
        const ResourceId physicalPrefix(rawPhysicalPrefix);
        if (!physicalPrefix.valid()) return false;
        mPrefixAliases.push_back({physicalPrefix.value(), std::move(logicalPrefix)});
        return true;
    }

    bool setLayers(std::string rawId, std::vector<ResourceLayer> layers) { return setLayers(ResourceId(rawId), std::move(layers)); }

    bool setLayers(ResourceId id, std::vector<ResourceLayer> layers) {
        if (!id.valid()) return false;
        mLayers.insert_or_assign(std::move(id), std::move(layers));
        return true;
    }

    std::optional<ResourceSource> load(const ResourceId& id) const override {
        const auto found = mResources.find(id);
        if (found == mResources.end()) return std::nullopt;
        return found->second;
    }

    ResourceId canonicalId(const ResourceId& id) const override {
        const PrefixAlias* match = nullptr;
        for (const PrefixAlias& alias : mPrefixAliases)
            if (hasPathPrefix(id.value(), alias.physicalPrefix) && (!match || alias.physicalPrefix.size() > match->physicalPrefix.size()))
                match = &alias;
        if (!match) return id;

        const std::string suffix = id.value().substr(match->physicalPrefix.size());
        const std::string logical = match->logicalPrefix.valid() ? match->logicalPrefix.value() + suffix : suffix;
        return ResourceId(logical);
    }

    ResourceId resolve(const ResourceId& base, std::string_view reference) const override {
        const ResourceId rawReference(reference);
        if (rawReference.valid()) {
            const ResourceId logicalReference = canonicalId(rawReference);
            if (logicalReference != rawReference) return logicalReference;
        }
        return canonicalId(ResourceId::resolve(base, reference));
    }

    std::vector<ResourceId> list(const ResourceId& prefix) const override {
        std::vector<ResourceId> result;
        const std::string directory = prefix.valid() ? prefix.value() + "/" : std::string();
        for (const auto& entry : mResources)
            if (directory.empty() || entry.first.value().rfind(directory, 0) == 0) result.push_back(entry.first);
        return result;
    }

    const std::map<ResourceId, ResourceSource>& resources() const { return mResources; }
    const std::vector<ResourceLayer>& layers(const ResourceId& id) const {
        static const std::vector<ResourceLayer> sEmpty;
        const auto found = mLayers.find(id);
        return found == mLayers.end() ? sEmpty : found->second;
    }
    const std::map<ResourceId, std::vector<ResourceLayer>>& layeredResources() const { return mLayers; }

private:
    struct PrefixAlias {
        std::string physicalPrefix;
        ResourceId logicalPrefix;

        bool operator==(const PrefixAlias& other) const = default;
    };

    friend bool operator==(const ResourceSnapshot& left, const ResourceSnapshot& right) {
        return left.mResources == right.mResources && left.mLayers == right.mLayers && left.mPrefixAliases == right.mPrefixAliases;
    }

    static bool hasPathPrefix(std::string_view id, std::string_view prefix) {
        return id == prefix || (id.size() > prefix.size() && id.rfind(prefix, 0) == 0 && id[prefix.size()] == '/');
    }

    std::map<ResourceId, ResourceSource> mResources;
    std::map<ResourceId, std::vector<ResourceLayer>> mLayers;
    std::vector<PrefixAlias> mPrefixAliases;
};
} // namespace radia::ui
