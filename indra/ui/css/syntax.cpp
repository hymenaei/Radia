/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "css/syntax.h"
#include <algorithm>
#include <cctype>
#include <cstdint>

namespace radia::ui::detail {
bool isCSSWhitespace(char value) {
    return value == '\t' || value == '\n' || value == '\f' || value == '\r' || value == ' ';
}

std::string trim(const std::string& value) {
    std::size_t begin = 0;
    while (begin < value.size() && isCSSWhitespace(value[begin])) ++begin;
    std::size_t end = value.size();
    while (end > begin && isCSSWhitespace(value[end - 1])) --end;
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

std::string decodeCSSIdentifier(std::string_view value) {
    const auto isHexDigit = [](char character) {
        const auto value = static_cast<unsigned char>(character);
        return (value >= '0' && value <= '9') || (value >= 'a' && value <= 'f') || (value >= 'A' && value <= 'F');
    };
    const auto appendCodePoint = [](std::string& result, std::uint32_t codePoint) {
        if (codePoint == 0 || codePoint > 0x10ffff || (codePoint >= 0xd800 && codePoint <= 0xdfff)) codePoint = 0xfffd;
        if (codePoint <= 0x7f) result.push_back(static_cast<char>(codePoint));
        else if (codePoint <= 0x7ff) {
            result.push_back(static_cast<char>(0xc0 | (codePoint >> 6)));
            result.push_back(static_cast<char>(0x80 | (codePoint & 0x3f)));
        } else if (codePoint <= 0xffff) {
            result.push_back(static_cast<char>(0xe0 | (codePoint >> 12)));
            result.push_back(static_cast<char>(0x80 | ((codePoint >> 6) & 0x3f)));
            result.push_back(static_cast<char>(0x80 | (codePoint & 0x3f)));
        } else {
            result.push_back(static_cast<char>(0xf0 | (codePoint >> 18)));
            result.push_back(static_cast<char>(0x80 | ((codePoint >> 12) & 0x3f)));
            result.push_back(static_cast<char>(0x80 | ((codePoint >> 6) & 0x3f)));
            result.push_back(static_cast<char>(0x80 | (codePoint & 0x3f)));
        }
    };

    std::string result;
    result.reserve(value.size());
    for (std::size_t position = 0; position < value.size();) {
        if (value[position] != '\\') {
            result.push_back(value[position++]);
            continue;
        }
        ++position;
        if (position >= value.size()) {
            result.push_back('\\');
            continue;
        }
        if (value[position] == '\n' || value[position] == '\f') {
            ++position;
            continue;
        }
        if (value[position] == '\r') {
            ++position;
            if (position < value.size() && value[position] == '\n') ++position;
            continue;
        }
        if (!isHexDigit(value[position])) {
            result.push_back(value[position++]);
            continue;
        }
        std::uint32_t codePoint = 0;
        std::size_t digits = 0;
        while (position < value.size() && digits < 6 && isHexDigit(value[position])) {
            codePoint *= 16;
            const auto digit = static_cast<unsigned char>(value[position++]);
            codePoint += digit <= '9' ? digit - '0' : (digit <= 'F' ? digit - 'A' + 10 : digit - 'a' + 10);
            ++digits;
        }
        if (position < value.size() && isCSSWhitespace(value[position])) ++position;
        appendCodePoint(result, codePoint);
    }
    return result;
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
        const bool separator = atEnd || (depth == 0 && isCSSWhitespace(character));
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
