#ifndef LL_RDUI_LOCALIZATION_H
#define LL_RDUI_LOCALIZATION_H

#include "rduidiagnostic.h"
#include "rduiinlinecontent.h"
#include "rduilocalizationvalue.h"
#include "rduiresourceprovider.h"
#include "rduitypes.h"
#include <memory>
#include <string>
#include <vector>

namespace rdui
{
    struct LocaleInfo
    {
        std::string id;
        std::string name;
        LayoutDirection direction = LayoutDirection::LeftToRight;
        std::string fallback;
    };

    struct LocalizationLoadResult : DiagnosticResult
    {
        bool ok() const { return !hasErrors(); }
    };

    class LocalizationCatalog
    {
        public:
            LocalizationCatalog();
            ~LocalizationCatalog();
            LocalizationCatalog(LocalizationCatalog&&) noexcept;
            LocalizationCatalog& operator=(LocalizationCatalog&&) noexcept;
            LocalizationCatalog(const LocalizationCatalog&) = delete;
            LocalizationCatalog& operator=(const LocalizationCatalog&) = delete;

            LocalizationLoadResult loadYaml(const std::string& yaml, const std::string& source_name = {});
            LocalizationLoadResult loadYamlLayers(const std::vector<ResourceLayer>& layers);

            std::vector<LocaleInfo> locales() const;
            const std::string& defaultLocaleId() const;
            const LocaleInfo* locale(const std::string& id) const;
            bool containsLocale(const std::string& id) const;
            bool containsDefaultString(const std::string& id) const;
            bool pluralCapable(const std::string& id) const;

            InlineContent resolve(const std::string& locale_id, const LocalizationRequest& request) const;
            std::string get(const std::string& locale_id, const LocalizationRequest& request) const;
            std::string get(const std::string& locale_id, const std::string& string_id) const;

        private:
            struct Impl;
            std::unique_ptr<Impl> mImpl;
    };
}

#endif // LL_RDUI_LOCALIZATION_H
