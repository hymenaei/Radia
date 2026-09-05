/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <string>
#include <unordered_map>
#include <utility>
#include <variant>

namespace radia::ui {
class LocalizationArgument {
public:
    using Value = std::variant<std::string, std::int64_t, double>;

    LocalizationArgument(std::string value) : mValue(std::move(value)) {}
    LocalizationArgument(const char* value) : mValue(std::string(value ? value : "")) {}
    LocalizationArgument(int value) : mValue(static_cast<std::int64_t>(value)) {}
    LocalizationArgument(std::int64_t value) : mValue(value) {}
    LocalizationArgument(double value) : mValue(value) {}

    const Value& value() const { return mValue; }

private:
    Value mValue;
};

using LocalizationArguments = std::unordered_map<std::string, LocalizationArgument>;

class LocalizedText {
public:
    LocalizedText(std::string key, LocalizationArguments arguments = {}) : mKey(std::move(key)), mArguments(std::move(arguments)) {}

    const std::string& key() const { return mKey; }
    const LocalizationArguments& arguments() const { return mArguments; }

private:
    std::string mKey;
    LocalizationArguments mArguments;
};
} // namespace radia::ui
