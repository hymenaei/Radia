#ifndef LL_RDUI_LOCALIZATION_H
#define LL_RDUI_LOCALIZATION_H

#include "rduidiagnostic.h"
#include "rduiresourceprovider.h"
#include "rduitypes.h"
#include <cstddef>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>

namespace rdui
{
    struct LanguageInfo
    {
        std::string id;
        std::string name;
        LayoutDirection direction = LayoutDirection::LeftToRight;
    };

    class TextValue
    {
        public:
            TextValue() = default;

            static TextValue literal(std::string value)
            {
                TextValue result;
                result.mValue = std::move(value);
                return result;
            }

            static TextValue fromLocalization(std::string key, std::string value)
            {
                TextValue result;
                result.mLocalizationKey = std::move(key);
                result.mValue = std::move(value);
                return result;
            }

            bool localized() const { return !mLocalizationKey.empty(); }
            const std::string& localizationKey() const { return mLocalizationKey; }
            const std::string& value() const { return mValue; }
            void updateLocalizedValue(std::string value) { if (localized()) mValue = std::move(value); }

        private:
            std::string mLocalizationKey;
            std::string mValue;
    };

    struct LocalizationLoadResult : DiagnosticResult
    {
        bool ok() const { return !hasErrors(); }
    };

    class LocalizationCatalog
    {
        public:
            LocalizationLoadResult loadXml(const std::string& xml, const std::string& source_name = {});
            LocalizationLoadResult loadXmlLayers(const std::vector<ResourceLayer>& layers);

            const std::vector<LanguageInfo>& languages() const { return mLanguages; }
            const std::string& defaultLanguageId() const { return mDefaultLanguageId; }
            const LanguageInfo* language(const std::string& id) const;
            bool containsLanguage(const std::string& id) const;
            bool containsDefaultString(const std::string& id) const;
            std::string get(const std::string& language_id, const std::string& string_id) const;

        private:
            using StringMap = std::unordered_map<std::string, std::string>;

            LocalizationLoadResult loadXmlDocument(const std::string& xml, const std::string& source_name,
                                                     bool require_complete_default);
            static bool validLanguageId(const std::string& id);
            const StringMap* stringsFor(const std::string& language_id) const;

            std::vector<LanguageInfo> mLanguages;
            std::unordered_map<std::string, std::size_t> mLanguageIndices;
            std::unordered_map<std::string, StringMap> mStrings;
            std::string mDefaultLanguageId;
    };
}

#endif // LL_RDUI_LOCALIZATION_H
