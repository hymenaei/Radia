/**
 * @file compiler.cpp
 * @brief Compiles layered localization catalogs and validates locale and string contracts.
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

#include "linden_common.h"
#include <algorithm>
#include <unordered_set>
#include "localization/internal.h"

namespace rdui {
using namespace localization_detail;

LocalizationLoadResult LocalizationCatalog::Impl::load(const std::vector<ResourceLayer>& layers) {
    LocalizationLoadResult result;
    if (layers.empty()) {
        result.error("localization.layers.empty", "No localization layers were provided.");
        return result;
    }

    ParsedCatalog base;
    result.append(parseYamlCatalog(layers.front().source, layers.front().sourceName, true, base));
    if (result.hasErrors()) return result;

    defaultLocale = *base.defaultLocale;
    const std::string defaultIdentity = localeIdentity(defaultLocale);
    for (ParsedLocale& parsed : base.locales) {
        LocaleRecord locale;
        locale.info.id = parsed.id;
        locale.info.name = parsed.name.value_or(std::string());
        locale.info.direction = parsed.direction.value_or(LayoutDirection::LeftToRight);
        if (parsed.fallback) locale.info.fallback = *parsed.fallback;
        else if (localeIdentity(parsed.id) != defaultIdentity) locale.info.fallback = defaultLocale;

        if (localeIdentity(parsed.id) == defaultIdentity && parsed.fallback)
            result.error("localization.default.fallback_forbidden", "The default locale cannot declare a fallback.", parsed.source, parsed.line);

        locale.strings = std::move(parsed.strings);
        localeIndices.emplace(localeIdentity(locale.info.id), locales.size());
        locales.push_back(std::move(locale));
    }
    if (result.hasErrors()) return result;

    for (std::size_t layerIndex = 1; layerIndex < layers.size(); ++layerIndex) {
        const ResourceLayer& layer = layers[layerIndex];
        ParsedCatalog patch;
        result.append(parseYamlCatalog(layer.source, layer.sourceName, false, patch));
        if (result.hasErrors()) return result;

        for (ParsedLocale& parsed : patch.locales) {
            const std::string identity = localeIdentity(parsed.id);
            if (identity == defaultIdentity && parsed.fallback) {
                result.error("localization.default.fallback_forbidden", "The default locale cannot declare a fallback.", parsed.source, parsed.line);
                continue;
            }

            const auto existing = localeIndices.find(identity);
            if (existing == localeIndices.end()) {
                if (!parsed.name) {
                    result.error("localization.layer.locale.name_missing", "A locale introduced by a layer requires name: " + parsed.id + ".",
                                 parsed.source, parsed.line);
                    continue;
                }

                LocaleRecord locale;
                locale.info.id = parsed.id;
                locale.info.name = *parsed.name;
                locale.info.direction = parsed.direction.value_or(LayoutDirection::LeftToRight);
                locale.info.fallback = parsed.fallback.value_or(defaultLocale);
                locale.strings = std::move(parsed.strings);
                localeIndices.emplace(identity, locales.size());
                locales.push_back(std::move(locale));
                continue;
            }

            LocaleRecord& locale = locales[existing->second];
            if (parsed.name) locale.info.name = *parsed.name;
            if (parsed.direction) locale.info.direction = *parsed.direction;
            if (parsed.fallback) locale.info.fallback = *parsed.fallback;
            for (auto& [key, value] : parsed.strings) locale.strings.insert_or_assign(std::move(key), std::move(value));
        }
        if (result.hasErrors()) return result;
    }

    compile(result);
    return result;
}

void LocalizationCatalog::Impl::compile(LocalizationLoadResult& result) {
    contracts.clear();
    localeIndices.clear();

    std::sort(locales.begin(), locales.end(),
              [](const LocaleRecord& left, const LocaleRecord& right) { return localeIdentity(left.info.id) < localeIdentity(right.info.id); });
    for (std::size_t index = 0; index < locales.size(); ++index) {
        locales[index].fallbackChain.clear();
        localeIndices.emplace(localeIdentity(locales[index].info.id), index);
    }

    const auto defaultFound = localeIndices.find(localeIdentity(defaultLocale));
    if (defaultFound == localeIndices.end()) {
        result.error("localization.default.undefined", "defaultLocale is not defined in locales: " + defaultLocale + ".");
        return;
    }
    const std::size_t defaultIndex = defaultFound->second;

    for (std::size_t start = 0; start < locales.size(); ++start) {
        LocaleRecord& locale = locales[start];
        std::unordered_set<std::size_t> seen;
        std::size_t current = start;
        while (true) {
            if (!seen.insert(current).second) {
                result.error("localization.locale.fallback_cycle", "Locale fallback cycle contains " + locales[current].info.id + ".");
                break;
            }
            locale.fallbackChain.push_back(current);
            if (current == defaultIndex) break;

            const auto fallback = localeIndices.find(localeIdentity(locales[current].info.fallback));
            if (fallback == localeIndices.end()) {
                result.error("localization.locale.fallback_unknown",
                             "Unknown fallback locale " + locales[current].info.fallback + " for " + locales[current].info.id + ".");
                break;
            }
            current = fallback->second;
        }

        UErrorCode status = U_ZERO_ERROR;
        locale.locale = icu::Locale::forLanguageTag(locale.info.id, status);
        locale.pluralRules.reset(U_SUCCESS(status) ? icu::PluralRules::forLocale(locale.locale, status) : nullptr);
        locale.numberFormat.reset(U_SUCCESS(status) ? icu::NumberFormat::createInstance(locale.locale, status) : nullptr);
        if (U_FAILURE(status) || !locale.pluralRules || !locale.numberFormat)
            result.error("localization.locale.runtime_invalid", "Could not initialize locale services for " + locale.info.id + ".");
    }
    if (result.hasErrors()) return;

    const StringMap& defaults = locales[defaultIndex].strings;
    for (const LocaleRecord& locale : locales) {
        for (const auto& [key, value] : locale.strings)
            if (!defaults.contains(key))
                result.error("localization.default.string_missing", "String '" + key + "' exists outside the default locale " + defaultLocale + ".",
                             value.source, value.line);
    }

    for (const auto& [key, defaultValue] : defaults) {
        StringContract contract;
        bool bindingSignatureSet = false;
        const auto inspectDefault = [&](const StringTemplate& value) {
            contract.arguments.insert(value.arguments.begin(), value.arguments.end());
            if (!bindingSignatureSet) {
                contract.bindings = value.bindings;
                bindingSignatureSet = true;
            } else if (value.bindings != contract.bindings)
                result.error("localization.string.bindings_mismatch", "All default variants must use the same kbd bindings: " + key + ".",
                             defaultValue.source, defaultValue.line);
        };
        forEachTemplate(defaultValue, inspectDefault);

        contract.plural = defaultValue.plural();
        std::set<std::string> introducedArguments;
        for (const LocaleRecord& locale : locales) {
            const auto found = locale.strings.find(key);
            if (found == locale.strings.end()) continue;
            const StringValue& localized = found->second;
            contract.plural = contract.plural || localized.plural();
            const auto inspectTranslation = [&](const StringTemplate& value) {
                if (value.bindings != contract.bindings)
                    result.error("localization.string.bindings_mismatch", "Translation must preserve the default kbd bindings: " + key + ".",
                                 localized.source, localized.line);
                if (localeIdentity(locale.info.id) == localeIdentity(defaultLocale)) return;
                for (const std::string& argument : value.arguments)
                    if (!contract.arguments.contains(argument)) introducedArguments.insert(argument);
            };
            forEachTemplate(localized, inspectTranslation);
        }

        if (!introducedArguments.empty()) {
            if (!contract.plural || introducedArguments.size() != 1)
                result.error("localization.string.argument_unknown", "Translations introduce arguments absent from the default String: " + key + ".",
                             defaultValue.source, defaultValue.line);
            else contract.requiredPluralArgument = *introducedArguments.begin();
        }
        contracts.emplace(key, std::move(contract));
    }
}

const LocaleRecord* LocalizationCatalog::Impl::locale(const std::string& id) const {
    const auto found = localeIndices.find(localeIdentity(id));
    return found == localeIndices.end() ? nullptr : &locales[found->second];
}

LocaleRecord* LocalizationCatalog::Impl::locale(const std::string& id) {
    const auto found = localeIndices.find(localeIdentity(id));
    return found == localeIndices.end() ? nullptr : &locales[found->second];
}

const StringValue* LocalizationCatalog::Impl::find(const LocaleRecord& locale, const std::string& key) const {
    const auto found = locale.strings.find(key);
    return found == locale.strings.end() ? nullptr : &found->second;
}
} // namespace rdui
