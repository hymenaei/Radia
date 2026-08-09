/**
 * @file localization.cpp
 * @brief Provides locale catalog loading, fallback, plural resolution, and localized content.
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
#include "localization/localization.h"
#include <algorithm>
#include <cmath>
#include <limits>
#include <unicode/unistr.h>
#include "localization/internal.h"

namespace rdui {
using namespace localization_detail;

namespace {
constexpr const char* FIRST_STRONG_ISOLATE = "\xE2\x81\xA8";
constexpr const char* POP_DIRECTIONAL_ISOLATE = "\xE2\x81\xA9";

std::string unicodeToUtf8(const icu::UnicodeString& value) {
    std::string result;
    value.toUTF8String(result);
    return result;
}

InlineContent rawKey(const std::string& key) {
    return InlineContent::text(key);
}
} // namespace

double LocalizationArgument::number() const {
    if (const auto* integer = std::get_if<std::int64_t>(&mValue)) return static_cast<double>(*integer);
    if (const auto* real = std::get_if<double>(&mValue)) return *real;
    return std::numeric_limits<double>::quiet_NaN();
}

LocalizationCatalog::LocalizationCatalog() : mImpl(std::make_unique<Impl>()) {}

LocalizationCatalog::~LocalizationCatalog() = default;
LocalizationCatalog::LocalizationCatalog(LocalizationCatalog&&) noexcept = default;
LocalizationCatalog& LocalizationCatalog::operator=(LocalizationCatalog&&) noexcept = default;

LocalizationLoadResult LocalizationCatalog::loadYaml(const std::string& yaml, const std::string& source_name) {
    return loadYamlLayers({ResourceLayer{source_name, yaml}});
}

LocalizationLoadResult LocalizationCatalog::loadYamlLayers(const std::vector<ResourceLayer>& layers) {
    auto candidate = std::make_unique<Impl>();
    LocalizationLoadResult result = candidate->load(layers);
    if (!result.hasErrors()) mImpl = std::move(candidate);
    return result;
}

std::vector<LocaleInfo> LocalizationCatalog::locales() const {
    std::vector<LocaleInfo> result;
    result.reserve(mImpl->locales.size());
    for (const LocaleRecord& locale : mImpl->locales) result.push_back(locale.info);
    return result;
}

const std::string& LocalizationCatalog::defaultLocaleId() const {
    return mImpl->default_locale;
}

const LocaleInfo* LocalizationCatalog::locale(const std::string& id) const {
    const LocaleRecord* locale = mImpl->locale(id);
    return locale ? &locale->info : nullptr;
}

bool LocalizationCatalog::containsLocale(const std::string& id) const {
    return mImpl->locale(id) != nullptr;
}

bool LocalizationCatalog::containsDefaultString(const std::string& id) const {
    const LocaleRecord* locale = mImpl->locale(mImpl->default_locale);
    return locale && mImpl->find(*locale, id);
}

bool LocalizationCatalog::pluralCapable(const std::string& id) const {
    const auto found = mImpl->contracts.find(id);
    return found != mImpl->contracts.end() && found->second.plural;
}

InlineContent LocalizationCatalog::resolve(const std::string& locale_id, const LocalizationRequest& request) const {
    const LocaleRecord* active = mImpl->locale(locale_id);
    if (!active) active = mImpl->locale(mImpl->default_locale);
    if (!active) return rawKey(request.key());

    const StringValue* value = nullptr;
    for (const std::size_t locale_index : active->fallback_chain) {
        value = mImpl->find(mImpl->locales[locale_index], request.key());
        if (value) break;
    }
    if (!value) {
        LL_WARNS("RadiaUI") << "Unknown localization String Key: " << request.key() << LL_ENDL;
        return rawKey(request.key());
    }

    const auto contract_found = mImpl->contracts.find(request.key());
    if (contract_found == mImpl->contracts.end()) return rawKey(request.key());
    const StringContract& contract = contract_found->second;

    const LocalizationArgument* plural_argument = nullptr;
    if (contract.plural) {
        if (!request.selectsPlural()) {
            LL_WARNS("RadiaUI") << "Plural-capable String Key requires a selector: " << request.key() << LL_ENDL;
            return rawKey(request.key());
        }
        const auto argument = request.arguments().find(request.pluralArgument());
        if (argument == request.arguments().end() || !argument->second.numeric() || !std::isfinite(argument->second.number())) {
            LL_WARNS("RadiaUI") << "Plural selector must name a finite numeric argument for " << request.key() << LL_ENDL;
            return rawKey(request.key());
        }
        if (!contract.required_plural_argument.empty() && contract.required_plural_argument != request.pluralArgument()) {
            LL_WARNS("RadiaUI") << "Plural selector for " << request.key() << " must be '" << contract.required_plural_argument << "'." << LL_ENDL;
            return rawKey(request.key());
        }
        plural_argument = &argument->second;
    }

    const StringTemplate* selected = nullptr;
    if (const StringTemplate* scalar = value->scalar()) selected = scalar;
    else {
        if (!plural_argument || !active->plural_rules) {
            LL_WARNS("RadiaUI") << "Could not select plural form for " << request.key() << LL_ENDL;
            return rawKey(request.key());
        }
        const std::string category = unicodeToUtf8(active->plural_rules->select(plural_argument->number()));
        const PluralTemplates& plurals = *value->plurals();
        const auto variant = plurals.find(category);
        selected = variant == plurals.end() ? &plurals.at("other") : &variant->second;
    }

    const auto format_argument = [&](const LocalizationArgument& argument) {
        std::string formatted;
        if (const auto* text = std::get_if<std::string>(&argument.value())) formatted = *text;
        else if (active->number_format) {
            icu::UnicodeString output;
            if (const auto* integer = std::get_if<std::int64_t>(&argument.value())) active->number_format->format(*integer, output);
            else active->number_format->format(std::get<double>(argument.value()), output);
            formatted = unicodeToUtf8(output);
        }
        return std::string(FIRST_STRONG_ISOLATE) + formatted + POP_DIRECTIONAL_ISOLATE;
    };

    const auto resolve_nodes = [&](auto&& self, const std::vector<TemplateNode>& nodes) -> std::vector<InlineContentNode> {
        std::vector<InlineContentNode> resolved;
        resolved.reserve(nodes.size());
        for (const TemplateNode& node : nodes) {
            switch (node.kind) {
                case TemplateKind::Text: resolved.push_back(InlineContentNode::text(node.value)); break;
                case TemplateKind::Argument: {
                    const auto argument = request.arguments().find(node.value);
                    if (argument == request.arguments().end()) {
                        LL_WARNS("RadiaUI") << "Missing localization argument '" << node.value << "' for " << request.key() << LL_ENDL;
                        resolved.push_back(InlineContentNode::text("{" + node.value + "}"));
                    } else resolved.push_back(InlineContentNode::text(format_argument(argument->second)));
                    break;
                }
                case TemplateKind::B: resolved.push_back(InlineContentNode::container(InlineContentKind::B, self(self, node.children))); break;
                case TemplateKind::I: resolved.push_back(InlineContentNode::container(InlineContentKind::I, self(self, node.children))); break;
                case TemplateKind::S: resolved.push_back(InlineContentNode::container(InlineContentKind::S, self(self, node.children))); break;
                case TemplateKind::Kbd: resolved.push_back(InlineContentNode::kbd(node.value)); break;
                case TemplateKind::Br: resolved.push_back(InlineContentNode::br()); break;
            }
        }
        return resolved;
    };

    return InlineContent(resolve_nodes(resolve_nodes, selected->nodes));
}

std::string LocalizationCatalog::get(const std::string& locale_id, const LocalizationRequest& request) const {
    return resolve(locale_id, request).plainText();
}

std::string LocalizationCatalog::get(const std::string& locale_id, const std::string& string_id) const {
    return get(locale_id, LocalizationRequest::text(string_id));
}
} // namespace rdui
