/**
 * @file internal.h
 * @brief Defines the private parsed localization model and catalog compilation state.
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

#ifndef RD_LOCALIZATION_INTERNAL_H
#define RD_LOCALIZATION_INTERNAL_H

#include <cstddef>
#include <cstdint>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <type_traits>
#include <unordered_map>
#include <utility>
#include <variant>
#include <vector>
#include <unicode/locid.h>
#include <unicode/numfmt.h>
#include <unicode/plurrule.h>
#include "localization/localization.h"

namespace rdui::localization_detail {
enum class TemplateKind : std::uint8_t { Text, Argument, B, I, S, Kbd, Br };

struct TemplateNode {
    TemplateKind kind = TemplateKind::Text;
    std::string value;
    std::vector<TemplateNode> children;
};

struct StringTemplate {
    std::vector<TemplateNode> nodes;
    std::set<std::string> arguments;
    std::multiset<std::string> bindings;
};

using PluralTemplates = std::unordered_map<std::string, StringTemplate>;

class StringValue {
public:
    using Templates = std::variant<StringTemplate, PluralTemplates>;

    StringValue(StringTemplate value, std::string sourceName, std::size_t sourceLine)
        : mTemplates(std::move(value)), source(std::move(sourceName)), line(sourceLine) {}

    StringValue(PluralTemplates value, std::string sourceName, std::size_t sourceLine)
        : mTemplates(std::move(value)), source(std::move(sourceName)), line(sourceLine) {}

    bool plural() const { return std::holds_alternative<PluralTemplates>(mTemplates); }
    const Templates& templates() const { return mTemplates; }
    const StringTemplate* scalar() const { return std::get_if<StringTemplate>(&mTemplates); }
    const PluralTemplates* plurals() const { return std::get_if<PluralTemplates>(&mTemplates); }

    std::string source;
    std::size_t line = 0;

private:
    Templates mTemplates;
};

template<typename Visitor> void forEachTemplate(const StringValue& value, Visitor&& visitor) {
    std::visit(
        [&](const auto& templates) {
            using Templates = std::decay_t<decltype(templates)>;
            if constexpr (std::is_same_v<Templates, StringTemplate>) {
                visitor(templates);
            } else {
                for (const auto& [category, value] : templates) {
                    (void)category;
                    visitor(value);
                }
            }
        },
        value.templates());
}

using StringMap = std::unordered_map<std::string, StringValue>;

struct ParsedLocale {
    std::string id;
    std::string source;
    std::size_t line = 0;
    std::optional<std::string> name;
    std::optional<LayoutDirection> direction;
    std::optional<std::string> fallback;
    bool stringsPresent = false;
    StringMap strings;
};

struct ParsedCatalog {
    std::optional<std::string> defaultLocale;
    std::vector<ParsedLocale> locales;
};

struct StringContract {
    std::set<std::string> arguments;
    std::multiset<std::string> bindings;
    bool plural = false;
    std::string requiredPluralArgument;
};

struct LocaleRecord {
    LocaleInfo info;
    StringMap strings;
    std::vector<std::size_t> fallbackChain;
    icu::Locale locale;
    std::unique_ptr<icu::PluralRules> pluralRules;
    std::unique_ptr<icu::NumberFormat> numberFormat;
};

std::string localeIdentity(const std::string& id);
bool parseRichString(const std::string& source, StringTemplate& parsed, LocalizationLoadResult& result, const std::string& sourceName,
                     std::size_t line);
LocalizationLoadResult parseYamlCatalog(const std::string& yaml, const std::string& sourceName, bool base, ParsedCatalog& catalog);
} // namespace rdui::localization_detail

namespace rdui {
struct LocalizationCatalog::Impl {
    LocalizationLoadResult load(const std::vector<ResourceLayer>& layers);
    void compile(LocalizationLoadResult& result);

    const localization_detail::LocaleRecord* locale(const std::string& id) const;
    localization_detail::LocaleRecord* locale(const std::string& id);
    const localization_detail::StringValue* find(const localization_detail::LocaleRecord& locale, const std::string& key) const;

    std::vector<localization_detail::LocaleRecord> locales;
    std::unordered_map<std::string, std::size_t> localeIndices;
    std::unordered_map<std::string, localization_detail::StringContract> contracts;
    std::string defaultLocale;
};
} // namespace rdui
#endif // RD_LOCALIZATION_INTERNAL_H
