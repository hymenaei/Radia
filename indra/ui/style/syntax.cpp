/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "style/syntax.h"
#include <algorithm>
#include <cctype>

namespace radia::ui::detail {
std::string trim(const std::string& value) {
    std::size_t begin = 0;
    while (begin < value.size() && std::isspace(static_cast<unsigned char>(value[begin]))) ++begin;
    std::size_t end = value.size();
    while (end > begin && std::isspace(static_cast<unsigned char>(value[end - 1]))) --end;
    return value.substr(begin, end - begin);
}

std::string lower(std::string value) {
    std::transform(value.begin(), value.end(), value.begin(), [](unsigned char character) { return static_cast<char>(std::tolower(character)); });
    return value;
}

bool startsWith(const std::string& value, const std::string& prefix) {
    return value.rfind(prefix, 0) == 0;
}

bool endsWith(const std::string& value, const std::string& suffix) {
    return value.size() >= suffix.size() && value.compare(value.size() - suffix.size(), suffix.size(), suffix) == 0;
}

std::vector<std::string> tokenizeTopLevel(const std::string& value, bool splitSlash) {
    std::vector<std::string> result;
    std::size_t start = std::string::npos;
    int depth = 0;
    const auto finish = [&](std::size_t end) {
        if (start == std::string::npos) return;
        result.push_back(value.substr(start, end - start));
        start = std::string::npos;
    };
    for (std::size_t index = 0; index <= value.size(); ++index) {
        const bool atEnd = index == value.size();
        const char character = atEnd ? ' ' : value[index];
        if (!atEnd && character == '(') ++depth;
        else if (!atEnd && character == ')') --depth;
        if (depth < 0) return {};

        const bool punctuation = splitSlash && !atEnd && depth == 0 && character == '/';
        const bool separator = atEnd || (depth == 0 && std::isspace(static_cast<unsigned char>(character)));
        if (separator || punctuation) {
            finish(index);
            if (punctuation) result.emplace_back("/");
        } else if (start == std::string::npos) start = index;
    }
    return depth == 0 ? result : std::vector<std::string>();
}

std::vector<std::string> splitTopLevel(const std::string& value, char delimiter) {
    std::vector<std::string> result;
    std::size_t start = 0;
    int depth = 0;
    for (std::size_t index = 0; index < value.size(); ++index) {
        if (value[index] == '(') ++depth;
        else if (value[index] == ')') --depth;
        else if (value[index] == delimiter && depth == 0) {
            result.push_back(trim(value.substr(start, index - start)));
            start = index + 1;
        }
        if (depth < 0) return {};
    }
    if (depth != 0) return {};
    result.push_back(trim(value.substr(start)));
    return result;
}

std::optional<std::size_t> matchingBlock(const std::string& value, std::size_t open) {
    if (open >= value.size() || value[open] != '{') return std::nullopt;
    int depth = 0;
    for (std::size_t index = open; index < value.size(); ++index)
        if (value[index] == '{') ++depth;
        else if (value[index] == '}' && --depth == 0) return index;
    return std::nullopt;
}
} // namespace radia::ui::detail
