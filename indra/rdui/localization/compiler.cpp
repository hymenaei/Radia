/**
 * @file compiler.cpp
 * @brief
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
    result.append(parseYamlCatalog(layers.front().source, layers.front().source_name, true, base));
    if (result.hasErrors()) return result;

    default_locale = *base.default_locale;
    const std::string default_identity = localeIdentity(default_locale);
    for (ParsedLocale& parsed : base.locales) {
        LocaleRecord locale;
        locale.info.id = parsed.id;
        locale.info.name = parsed.name.value_or(std::string());
        locale.info.direction = parsed.direction.value_or(LayoutDirection::LeftToRight);
        if (parsed.fallback) locale.info.fallback = *parsed.fallback;
        else if (localeIdentity(parsed.id) != default_identity) locale.info.fallback = default_locale;

        if (localeIdentity(parsed.id) == default_identity && parsed.fallback)
            result.error("localization.default.fallback_forbidden", "The default locale cannot declare a fallback.", parsed.source, parsed.line);

        locale.strings = std::move(parsed.strings);
        locale_indices.emplace(localeIdentity(locale.info.id), locales.size());
        locales.push_back(std::move(locale));
    }
    if (result.hasErrors()) return result;

    for (std::size_t layer_index = 1; layer_index < layers.size(); ++layer_index) {
        const ResourceLayer& layer = layers[layer_index];
        ParsedCatalog patch;
        result.append(parseYamlCatalog(layer.source, layer.source_name, false, patch));
        if (result.hasErrors()) return result;

        for (ParsedLocale& parsed : patch.locales) {
            const std::string identity = localeIdentity(parsed.id);
            if (identity == default_identity && parsed.fallback) {
                result.error("localization.default.fallback_forbidden", "The default locale cannot declare a fallback.", parsed.source, parsed.line);
                continue;
            }

            const auto existing = locale_indices.find(identity);
            if (existing == locale_indices.end()) {
                if (!parsed.name) {
                    result.error("localization.layer.locale.name_missing", "A locale introduced by a layer requires name: " + parsed.id + ".",
                                 parsed.source, parsed.line);
                    continue;
                }

                LocaleRecord locale;
                locale.info.id = parsed.id;
                locale.info.name = *parsed.name;
                locale.info.direction = parsed.direction.value_or(LayoutDirection::LeftToRight);
                locale.info.fallback = parsed.fallback.value_or(default_locale);
                locale.strings = std::move(parsed.strings);
                locale_indices.emplace(identity, locales.size());
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
    locale_indices.clear();

    std::sort(locales.begin(), locales.end(),
              [](const LocaleRecord& left, const LocaleRecord& right) { return localeIdentity(left.info.id) < localeIdentity(right.info.id); });
    for (std::size_t index = 0; index < locales.size(); ++index) {
        locales[index].fallback_chain.clear();
        locale_indices.emplace(localeIdentity(locales[index].info.id), index);
    }

    const auto default_found = locale_indices.find(localeIdentity(default_locale));
    if (default_found == locale_indices.end()) {
        result.error("localization.default.undefined", "defaultLocale is not defined in locales: " + default_locale + ".");
        return;
    }
    const std::size_t default_index = default_found->second;

    for (std::size_t start = 0; start < locales.size(); ++start) {
        LocaleRecord& locale = locales[start];
        std::unordered_set<std::size_t> seen;
        std::size_t current = start;
        while (true) {
            if (!seen.insert(current).second) {
                result.error("localization.locale.fallback_cycle", "Locale fallback cycle contains " + locales[current].info.id + ".");
                break;
            }
            locale.fallback_chain.push_back(current);
            if (current == default_index) break;

            const auto fallback = locale_indices.find(localeIdentity(locales[current].info.fallback));
            if (fallback == locale_indices.end()) {
                result.error("localization.locale.fallback_unknown",
                             "Unknown fallback locale " + locales[current].info.fallback + " for " + locales[current].info.id + ".");
                break;
            }
            current = fallback->second;
        }

        UErrorCode status = U_ZERO_ERROR;
        locale.locale = icu::Locale::forLanguageTag(locale.info.id, status);
        locale.plural_rules.reset(U_SUCCESS(status) ? icu::PluralRules::forLocale(locale.locale, status) : nullptr);
        locale.number_format.reset(U_SUCCESS(status) ? icu::NumberFormat::createInstance(locale.locale, status) : nullptr);
        if (U_FAILURE(status) || !locale.plural_rules || !locale.number_format)
            result.error("localization.locale.runtime_invalid", "Could not initialize locale services for " + locale.info.id + ".");
    }
    if (result.hasErrors()) return;

    const StringMap& defaults = locales[default_index].strings;
    for (const LocaleRecord& locale : locales) {
        for (const auto& [key, value] : locale.strings)
            if (!defaults.contains(key))
                result.error("localization.default.string_missing", "String '" + key + "' exists outside the default locale " + default_locale + ".",
                             value.source, value.line);
    }

    for (const auto& [key, default_value] : defaults) {
        StringContract contract;
        bool binding_signature_set = false;
        const auto inspect_default = [&](const StringTemplate& value) {
            contract.arguments.insert(value.arguments.begin(), value.arguments.end());
            if (!binding_signature_set) {
                contract.bindings = value.bindings;
                binding_signature_set = true;
            } else if (value.bindings != contract.bindings)
                result.error("localization.string.bindings_mismatch", "All default variants must use the same kbd bindings: " + key + ".",
                             default_value.source, default_value.line);
        };
        forEachTemplate(default_value, inspect_default);

        contract.plural = default_value.plural();
        std::set<std::string> introduced_arguments;
        for (const LocaleRecord& locale : locales) {
            const auto found = locale.strings.find(key);
            if (found == locale.strings.end()) continue;
            const StringValue& localized = found->second;
            contract.plural = contract.plural || localized.plural();
            const auto inspect_translation = [&](const StringTemplate& value) {
                if (value.bindings != contract.bindings)
                    result.error("localization.string.bindings_mismatch", "Translation must preserve the default kbd bindings: " + key + ".",
                                 localized.source, localized.line);
                if (localeIdentity(locale.info.id) == localeIdentity(default_locale)) return;
                for (const std::string& argument : value.arguments)
                    if (!contract.arguments.contains(argument)) introduced_arguments.insert(argument);
            };
            forEachTemplate(localized, inspect_translation);
        }

        if (!introduced_arguments.empty()) {
            if (!contract.plural || introduced_arguments.size() != 1)
                result.error("localization.string.argument_unknown", "Translations introduce arguments absent from the default String: " + key + ".",
                             default_value.source, default_value.line);
            else contract.required_plural_argument = *introduced_arguments.begin();
        }
        contracts.emplace(key, std::move(contract));
    }
}

const LocaleRecord* LocalizationCatalog::Impl::locale(const std::string& id) const {
    const auto found = locale_indices.find(localeIdentity(id));
    return found == locale_indices.end() ? nullptr : &locales[found->second];
}

LocaleRecord* LocalizationCatalog::Impl::locale(const std::string& id) {
    const auto found = locale_indices.find(localeIdentity(id));
    return found == locale_indices.end() ? nullptr : &locales[found->second];
}

const StringValue* LocalizationCatalog::Impl::find(const LocaleRecord& locale, const std::string& key) const {
    const auto found = locale.strings.find(key);
    return found == locale.strings.end() ? nullptr : &found->second;
}
} // namespace rdui
