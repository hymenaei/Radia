#ifndef LL_RDUI_RESOURCE_PROVIDER_H
#define LL_RDUI_RESOURCE_PROVIDER_H

#include <map>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace rdui
{
    using ResourceDependencyMap = std::map<std::string, std::set<std::string>>;

    struct ResourceLayer
    {
        std::string source_name;
        std::string source;
        // Layer-local resource identity and immutable sidecar sources. Stylesheet
        // compilation uses these to resolve @import without performing I/O or
        // consulting another Skin layer.
        std::string entrypoint;
        std::map<std::string, std::string> modules;

        std::string sourceNameFor(std::string_view resource_id) const
        {
            if (resource_id == entrypoint || entrypoint.empty()) return source_name;
            if (source_name.size() >= entrypoint.size()
                && source_name.compare(source_name.size() - entrypoint.size(),
                                       entrypoint.size(), entrypoint) == 0)
            {
                return source_name.substr(0, source_name.size() - entrypoint.size())
                    + std::string(resource_id);
            }
            return std::string(resource_id);
        }

        bool operator==(const ResourceLayer& other) const = default;
    };

    class ResourceProvider
    {
        public:
            virtual ~ResourceProvider() = default;

            virtual std::optional<std::string> load(const std::string& resource_id) const = 0;

            virtual std::vector<std::string> list(const std::string& prefix) const { return {}; }
    };

    class ResourceSnapshot final : public ResourceProvider
    {
        public:
            ResourceSnapshot() = default;
            explicit ResourceSnapshot(std::map<std::string, std::string> resources)
                           : mResources(std::move(resources)) {}

            void add(std::string resource_id, std::string source)
            {
                mResources.insert_or_assign(std::move(resource_id), std::move(source));
            }

            void setLayers(std::string resource_id, std::vector<ResourceLayer> layers)
            {
                mLayers.insert_or_assign(std::move(resource_id), std::move(layers));
            }

            std::optional<std::string> load(const std::string& resource_id) const override
            {
                const auto found = mResources.find(resource_id);
                return found == mResources.end() ? std::nullopt : std::optional<std::string>(found->second);
            }

            std::vector<std::string> list(const std::string& prefix) const override
            {
                std::vector<std::string> result;
                const std::string directory = prefix.empty() ? std::string() : prefix + "/";
                for (const auto& resource : mResources)
                    if (directory.empty() || resource.first.rfind(directory, 0) == 0) result.push_back(resource.first);
                return result;
            }

            const std::map<std::string, std::string>& resources() const { return mResources; }
            const std::vector<ResourceLayer>& layers(const std::string& resource_id) const
            {
                static const std::vector<ResourceLayer> empty;
                const auto found = mLayers.find(resource_id);
                return found == mLayers.end() ? empty : found->second;
            }
            const std::map<std::string, std::vector<ResourceLayer>>& layeredResources() const { return mLayers; }

        private:
            std::map<std::string, std::string> mResources;
            std::map<std::string, std::vector<ResourceLayer>> mLayers;
    };
}

#endif // LL_RDUI_RESOURCE_PROVIDER_H
