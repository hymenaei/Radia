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

namespace radia::ui {
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

LocalizationLoadResult LocalizationCatalog::loadYaml(const std::string& yaml, const std::string& sourceName) {
    return loadYamlLayers({ResourceLayer{sourceName, yaml}});
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
    return mImpl->defaultLocale;
}

const LocaleInfo* LocalizationCatalog::locale(const std::string& id) const {
    const LocaleRecord* locale = mImpl->locale(id);
    return locale ? &locale->info : nullptr;
}

bool LocalizationCatalog::containsLocale(const std::string& id) const {
    return mImpl->locale(id) != nullptr;
}

bool LocalizationCatalog::containsDefaultString(const std::string& id) const {
    const LocaleRecord* locale = mImpl->locale(mImpl->defaultLocale);
    return locale && mImpl->find(*locale, id);
}

bool LocalizationCatalog::pluralCapable(const std::string& id) const {
    const auto found = mImpl->contracts.find(id);
    return found != mImpl->contracts.end() && found->second.plural;
}

InlineContent LocalizationCatalog::resolve(const std::string& localeId, const LocalizationRequest& request) const {
    const LocaleRecord* active = mImpl->locale(localeId);
    if (!active) active = mImpl->locale(mImpl->defaultLocale);
    if (!active) return rawKey(request.key());

    const StringValue* value = nullptr;
    for (const std::size_t localeIndex : active->fallbackChain) {
        value = mImpl->find(mImpl->locales[localeIndex], request.key());
        if (value) break;
    }
    if (!value) {
        LL_WARNS("RadiaUI") << "Unknown localization String Key: " << request.key() << LL_ENDL;
        return rawKey(request.key());
    }

    const auto contractFound = mImpl->contracts.find(request.key());
    if (contractFound == mImpl->contracts.end()) return rawKey(request.key());
    const StringContract& contract = contractFound->second;

    const LocalizationArgument* pluralArgument = nullptr;
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
        if (!contract.requiredPluralArgument.empty() && contract.requiredPluralArgument != request.pluralArgument()) {
            LL_WARNS("RadiaUI") << "Plural selector for " << request.key() << " must be '" << contract.requiredPluralArgument << "'." << LL_ENDL;
            return rawKey(request.key());
        }
        pluralArgument = &argument->second;
    }

    const StringTemplate* selected = nullptr;
    if (const StringTemplate* scalar = value->scalar()) selected = scalar;
    else {
        if (!pluralArgument || !active->pluralRules) {
            LL_WARNS("RadiaUI") << "Could not select plural form for " << request.key() << LL_ENDL;
            return rawKey(request.key());
        }
        const std::string category = unicodeToUtf8(active->pluralRules->select(pluralArgument->number()));
        const PluralTemplates& plurals = *value->plurals();
        const auto variant = plurals.find(category);
        selected = variant == plurals.end() ? &plurals.at("other") : &variant->second;
    }

    const auto formatArgument = [&](const LocalizationArgument& argument) {
        std::string formatted;
        if (const auto* text = std::get_if<std::string>(&argument.value())) formatted = *text;
        else if (active->numberFormat) {
            icu::UnicodeString output;
            if (const auto* integer = std::get_if<std::int64_t>(&argument.value())) active->numberFormat->format(*integer, output);
            else active->numberFormat->format(std::get<double>(argument.value()), output);
            formatted = unicodeToUtf8(output);
        }
        return std::string(FIRST_STRONG_ISOLATE) + formatted + POP_DIRECTIONAL_ISOLATE;
    };

    const auto resolveNodes = [&](auto&& self, const std::vector<TemplateNode>& nodes) -> std::vector<InlineContentNode> {
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
                    } else resolved.push_back(InlineContentNode::text(formatArgument(argument->second)));
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

    return InlineContent(resolveNodes(resolveNodes, selected->nodes));
}

std::string LocalizationCatalog::get(const std::string& localeId, const LocalizationRequest& request) const {
    return resolve(localeId, request).plainText();
}

std::string LocalizationCatalog::get(const std::string& localeId, const std::string& stringId) const {
    return get(localeId, LocalizationRequest::text(stringId));
}
} // namespace radia::ui
