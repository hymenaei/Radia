#include "linden_common.h"
#include "rduilocalization.h"
#include "llxmlnode.h"
#include <initializer_list>

namespace rdui
{
    namespace
    {
        std::string elementName(LLXMLNode* node)
        {
            return node && node->getName() && node->getName()->mString ? node->getName()->mString : std::string();
        }

        std::size_t lineOf(LLXMLNode* node)
        {
            const S32 line = node ? node->getLineNumber() : -1;
            return line > 0 ? static_cast<std::size_t>(line) : 0;
        }

        bool attribute(LLXMLNode* node, const char* name, std::string& value)
        {
            return node && node->getAttributeString(name, value);
        }

        bool allowedAttribute(const std::string& name, std::initializer_list<const char*> allowed)
        {
            for (const char* candidate : allowed) if (name == candidate) return true;
            return false;
        }

        void validateAttributes(LLXMLNode* node, std::initializer_list<const char*> allowed, LocalizationLoadResult& result, const std::string& source)
        {
            if (!node) return;
            for (const auto& entry : node->mAttributes)
            {
                const std::string name = elementName(entry.second.get());
                if (!allowedAttribute(name, allowed))
                    result.error("localization.attribute.unknown",
                                 "Unknown attribute '" + name + "' on <" + elementName(node) + ">.",
                                 source, lineOf(node));
            }
        }
    }

    bool LocalizationCatalog::validLanguageId(const std::string& id)
    {
        if (id.empty()) return false;
        for (const unsigned char character : id)
        {
            const bool alphanumeric = (character >= 'a' && character <= 'z')
                                   || (character >= 'A' && character <= 'Z')
                                   || (character >= '0' && character <= '9');
            if (!alphanumeric && character != '.' && character != '_' && character != '-') return false;
        }
        return true;
    }

    LocalizationLoadResult LocalizationCatalog::loadXml(const std::string& xml, const std::string& source_name)
    {
        return loadXmlDocument(xml, source_name, true);
    }

    LocalizationLoadResult LocalizationCatalog::loadXmlDocument(
        const std::string& xml, const std::string& source_name, bool require_complete_default)
    {
        LocalizationLoadResult result;
        LLXMLNodePtr root;
        if (!LLXMLNode::parseBuffer(xml.data(), xml.size(), root, nullptr) || root.isNull())
        {
            result.error("localization.xml.invalid", "Could not parse Radia UI localization XML.", source_name);
            return result;
        }
        if (elementName(root.get()) != "localizations")
        {
            result.error("localization.root.invalid", "Localization root must be <localizations>.", source_name, lineOf(root.get()));
            return result;
        }
        validateAttributes(root.get(), {"default"}, result, source_name);

        std::vector<LanguageInfo> languages;
        std::unordered_map<std::string, std::size_t> language_indices;
        std::unordered_map<std::string, StringMap> strings;
        std::unordered_map<std::string, std::unordered_map<std::string, std::size_t>> string_lines;
        std::string default_id;
        if (!attribute(root.get(), "default", default_id) || !validLanguageId(default_id))
            result.error("localization.default.invalid", "Localization XML requires a valid default language ID.", source_name, lineOf(root.get()));

        for (LLXMLNodePtr language = root->getFirstChild(); language.notNull(); language = language->getNextSibling())
        {
            if (elementName(language.get()) != "localization")
            {
                result.error("localization.element.unknown",
                             "Unsupported <" + elementName(language.get()) + "> in Radia UI localizations.",
                             source_name, lineOf(language.get()));
                continue;
            }
            validateAttributes(language.get(), {"id", "lang", "direction"}, result, source_name);

            std::string id;
            std::string name;
            std::string direction_name;
            attribute(language.get(), "id", id);
            attribute(language.get(), "lang", name);
            attribute(language.get(), "direction", direction_name);
            if (!validLanguageId(id))
            {
                result.error("localization.language.id_invalid", "Localization has a missing or invalid ID.", source_name, lineOf(language.get()));
                continue;
            }
            if (name.empty())
            {
                result.error("localization.language.name_missing", "Localization has no display name: " + id + ".", source_name, lineOf(language.get()));
                continue;
            }
            LayoutDirection direction = LayoutDirection::LeftToRight;
            if (direction_name == "ltr") direction = LayoutDirection::LeftToRight;
            else if (direction_name == "rtl") direction = LayoutDirection::RightToLeft;
            else
            {
                result.error("localization.language.direction_invalid",
                             "Localization direction must be exactly 'ltr' or 'rtl': " + id + ".",
                             source_name, lineOf(language.get()));
                continue;
            }
            if (language_indices.find(id) != language_indices.end())
            {
                result.error("localization.language.duplicate", "Duplicate localization language: " + id + ".", source_name, lineOf(language.get()));
                continue;
            }

            language_indices.emplace(id, languages.size());
            languages.push_back({id, name, direction});
            StringMap& language_strings = strings[id];
            for (LLXMLNodePtr entry = language->getFirstChild(); entry.notNull(); entry = entry->getNextSibling())
            {
                if (elementName(entry.get()) != "string")
                {
                    result.error("localization.string.element_unknown",
                                 "Unsupported <" + elementName(entry.get()) + "> in localization " + id + ".",
                                 source_name, lineOf(entry.get()));
                    continue;
                }
                validateAttributes(entry.get(), {"id"}, result, source_name);
                std::string string_id;
                attribute(entry.get(), "id", string_id);
                if (string_id.empty())
                {
                    result.error("localization.string.id_missing", "Localization string has no ID in language " + id + ".", source_name, lineOf(entry.get()));
                    continue;
                }
                if (!language_strings.emplace(string_id, entry->getTextContents()).second)
                    result.error("localization.string.duplicate", "Duplicate localization string: " + id + "/" + string_id + ".",
                                 source_name, lineOf(entry.get()));
                else string_lines[id][string_id] = lineOf(entry.get());
            }
        }

        if (languages.empty())
            result.error("localization.languages.empty", "Localization XML defines no languages.", source_name, lineOf(root.get()));
        if (require_complete_default && !default_id.empty() && language_indices.find(default_id) == language_indices.end())
            result.error("localization.default.missing", "Default localization is not defined: " + default_id + ".", source_name, lineOf(root.get()));

        const auto default_strings = strings.find(default_id);
        if (require_complete_default && default_strings != strings.end())
        {
            for (const auto& language : strings)
            {
                if (language.first == default_id) continue;
                for (const auto& entry : language.second)
                {
                    if (default_strings->second.find(entry.first) == default_strings->second.end())
                        result.error("localization.default.string_missing",
                                     "String '" + entry.first + "' exists in " + language.first
                                     + " but not in the default language " + default_id + ".",
                                     source_name, string_lines[language.first][entry.first]);
                }
            }
        }

        if (result.hasErrors()) return result;
        mLanguages = std::move(languages);
        mLanguageIndices = std::move(language_indices);
        mStrings = std::move(strings);
        mDefaultLanguageId = std::move(default_id);
        return result;
    }

    LocalizationLoadResult LocalizationCatalog::loadXmlLayers(const std::vector<ResourceLayer>& layers)
    {
        LocalizationLoadResult result;
        LocalizationCatalog candidate;
        bool initialized = false;

        for (const ResourceLayer& layer : layers)
        {
            LocalizationCatalog parsed;
            LocalizationLoadResult loaded = parsed.loadXmlDocument(layer.source, layer.source_name, false);
            result.append(std::move(loaded));
            if (loaded.hasErrors()) continue;

            if (!initialized)
            {
                candidate = std::move(parsed);
                initialized = true;
                continue;
            }
            if (parsed.mDefaultLanguageId != candidate.mDefaultLanguageId)
            {
                result.error("localization.layer.default_mismatch",
                             "Localization layer default language '" + parsed.mDefaultLanguageId
                             + "' does not match base default language '" + candidate.mDefaultLanguageId + "'.",
                             layer.source_name);
                continue;
            }

            for (LanguageInfo& language : parsed.mLanguages)
            {
                const auto existing = candidate.mLanguageIndices.find(language.id);
                if (existing == candidate.mLanguageIndices.end())
                {
                    candidate.mLanguageIndices.emplace(language.id, candidate.mLanguages.size());
                    candidate.mLanguages.push_back(std::move(language));
                }
                else candidate.mLanguages[existing->second] = std::move(language);
            }
            for (auto& [language_id, strings] : parsed.mStrings)
            {
                StringMap& destination = candidate.mStrings[language_id];
                for (auto& [string_id, value] : strings)
                    destination.insert_or_assign(std::move(string_id), std::move(value));
            }
        }

        if (!initialized && !result.hasErrors())
            result.error("localization.layers.empty", "No localization layers were provided.");
        if (result.hasErrors()) return result;

        if (!candidate.containsLanguage(candidate.mDefaultLanguageId))
        {
            result.error("localization.default.missing",
                         "Default localization is not defined: " + candidate.mDefaultLanguageId + ".");
            return result;
        }

        const StringMap* defaults = candidate.stringsFor(candidate.mDefaultLanguageId);
        for (const auto& [language_id, strings] : candidate.mStrings)
        {
            if (language_id == candidate.mDefaultLanguageId) continue;
            for (const auto& [string_id, value] : strings)
            {
                (void)value;
                if (!defaults || defaults->find(string_id) == defaults->end())
                    result.error("localization.default.string_missing",
                                 "String '" + string_id + "' exists in " + language_id
                                 + " but not in the default language " + candidate.mDefaultLanguageId + ".");
            }
        }
        if (!result.hasErrors()) *this = std::move(candidate);
        return result;
    }

    const LanguageInfo* LocalizationCatalog::language(const std::string& id) const
    {
        const auto found = mLanguageIndices.find(id);
        return found == mLanguageIndices.end() ? nullptr : &mLanguages[found->second];
    }

    bool LocalizationCatalog::containsLanguage(const std::string& id) const
    {
        return language(id) != nullptr;
    }

    const LocalizationCatalog::StringMap* LocalizationCatalog::stringsFor(const std::string& language_id) const
    {
        const auto found = mStrings.find(language_id);
        return found == mStrings.end() ? nullptr : &found->second;
    }

    bool LocalizationCatalog::containsDefaultString(const std::string& id) const
    {
        const StringMap* strings = stringsFor(mDefaultLanguageId);
        return strings && strings->find(id) != strings->end();
    }

    std::string LocalizationCatalog::get(const std::string& language_id, const std::string& string_id) const
    {
        if (const StringMap* active = stringsFor(language_id))
        {
            const auto found = active->find(string_id);
            if (found != active->end()) return found->second;
        }
        if (language_id != mDefaultLanguageId)
        {
            if (const StringMap* fallback = stringsFor(mDefaultLanguageId))
            {
                const auto found = fallback->find(string_id);
                if (found != fallback->end()) return found->second;
            }
        }
        return string_id;
    }
}
