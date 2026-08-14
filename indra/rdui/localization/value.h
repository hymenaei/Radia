/**
 * @file value.h
 * @brief Defines typed localization arguments and text and plural requests.
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

#ifndef RD_LOCALIZATION_VALUE_H
#define RD_LOCALIZATION_VALUE_H

#include <cstdint>
#include <string>
#include <unordered_map>
#include <utility>
#include <variant>

namespace rdui {
class LocalizationArgument {
public:
    using Value = std::variant<std::string, std::int64_t, double>;

    LocalizationArgument(std::string value) : mValue(std::move(value)) {}
    LocalizationArgument(const char* value) : mValue(std::string(value ? value : "")) {}
    LocalizationArgument(int value) : mValue(static_cast<std::int64_t>(value)) {}
    LocalizationArgument(std::int64_t value) : mValue(value) {}
    LocalizationArgument(double value) : mValue(value) {}

    const Value& value() const { return mValue; }
    bool numeric() const { return std::holds_alternative<std::int64_t>(mValue) || std::holds_alternative<double>(mValue); }
    double number() const;

private:
    Value mValue;
};

using LocalizationArguments = std::unordered_map<std::string, LocalizationArgument>;

class LocalizationRequest {
public:
    static LocalizationRequest text(std::string key, LocalizationArguments arguments = {}) {
        return LocalizationRequest(std::move(key), std::move(arguments), {});
    }

    static LocalizationRequest plural(std::string key, std::string selector, int value, LocalizationArguments arguments = {}) {
        return plural(std::move(key), std::move(selector), static_cast<std::int64_t>(value), std::move(arguments));
    }

    static LocalizationRequest plural(std::string key, std::string selector, std::int64_t value, LocalizationArguments arguments = {}) {
        arguments.insert_or_assign(selector, LocalizationArgument(value));
        return LocalizationRequest(std::move(key), std::move(arguments), std::move(selector));
    }

    static LocalizationRequest plural(std::string key, std::string selector, double value, LocalizationArguments arguments = {}) {
        arguments.insert_or_assign(selector, LocalizationArgument(value));
        return LocalizationRequest(std::move(key), std::move(arguments), std::move(selector));
    }

    const std::string& key() const { return mKey; }
    const LocalizationArguments& arguments() const { return mArguments; }
    bool selectsPlural() const { return !mPluralArgument.empty(); }
    const std::string& pluralArgument() const { return mPluralArgument; }

private:
    LocalizationRequest(std::string key, LocalizationArguments arguments, std::string pluralArgument)
        : mKey(std::move(key)), mArguments(std::move(arguments)), mPluralArgument(std::move(pluralArgument)) {}

    std::string mKey;
    LocalizationArguments mArguments;
    std::string mPluralArgument;
};
} // namespace rdui
#endif // RD_LOCALIZATION_VALUE_H
