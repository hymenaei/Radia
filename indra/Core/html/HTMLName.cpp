/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "HTMLName.h"
#include <cstdint>
#include <limits>

namespace Core {
bool isHTMLNameCharacter(char character) {
    return (character >= 'a' && character <= 'z') || (character >= 'A' && character <= 'Z') || (character >= '0' && character <= '9')
        || character == '-' || character == '_' || character == ':';
}

bool isHTMLWhitespace(char character) {
    return character == '\t' || character == '\n' || character == '\f' || character == '\r' || character == ' ';
}

bool containsHTMLWhitespace(std::string_view value) {
    for (const char character : value)
        if (isHTMLWhitespace(character))
            return true;
    return false;
}

std::string canonicalizeHTMLName(std::string_view name) {
    std::string result;
    result.reserve(name.size());
    for (const char character : name)
        result.push_back(character >= 'A' && character <= 'Z' ? static_cast<char>(character + ('a' - 'A')) : character);
    return result;
}

namespace {
bool appendNumericReference(std::string& result, std::string_view entity) {
    if (entity.size() < 2 || entity.front() != '#')
        return false;

    const bool hexadecimal = entity.size() > 2 && (entity[1] == 'x' || entity[1] == 'X');
    const std::size_t begin = hexadecimal ? 2 : 1;
    if (begin == entity.size())
        return false;

    std::uint32_t codepoint = 0;
    const std::uint32_t base = hexadecimal ? 16U : 10U;
    for (std::size_t index = begin; index < entity.size(); ++index) {
        const char character = entity[index];
        std::uint32_t digit = std::numeric_limits<std::uint32_t>::max();
        if (character >= '0' && character <= '9')
            digit = static_cast<std::uint32_t>(character - '0');
        else if (hexadecimal && character >= 'a' && character <= 'f')
            digit = static_cast<std::uint32_t>(character - 'a' + 10);
        else if (hexadecimal && character >= 'A' && character <= 'F')
            digit = static_cast<std::uint32_t>(character - 'A' + 10);
        if (digit >= base || codepoint > (0x10FFFFU - digit) / base)
            return false;
        codepoint = codepoint * base + digit;
    }
    if (codepoint > 0x10FFFFU || (codepoint >= 0xD800U && codepoint <= 0xDFFFU))
        return false;

    if (codepoint <= 0x7FU)
        result.push_back(static_cast<char>(codepoint));
    else if (codepoint <= 0x7FFU) {
        result.push_back(static_cast<char>(0xC0U | (codepoint >> 6)));
        result.push_back(static_cast<char>(0x80U | (codepoint & 0x3FU)));
    } else if (codepoint <= 0xFFFFU) {
        result.push_back(static_cast<char>(0xE0U | (codepoint >> 12)));
        result.push_back(static_cast<char>(0x80U | ((codepoint >> 6) & 0x3FU)));
        result.push_back(static_cast<char>(0x80U | (codepoint & 0x3FU)));
    } else {
        result.push_back(static_cast<char>(0xF0U | (codepoint >> 18)));
        result.push_back(static_cast<char>(0x80U | ((codepoint >> 12) & 0x3FU)));
        result.push_back(static_cast<char>(0x80U | ((codepoint >> 6) & 0x3FU)));
        result.push_back(static_cast<char>(0x80U | (codepoint & 0x3FU)));
    }
    return true;
}
} // namespace

std::string decodeHTMLReferences(std::string_view value) {
    std::string result;
    result.reserve(value.size());
    std::size_t run = 0;
    for (std::size_t position = 0; position < value.size();) {
        if (value[position] != '&') {
            ++position;
            continue;
        }
        const std::size_t end = value.find(';', position + 1);
        if (end == std::string_view::npos)
            break;
        const std::string_view entity = value.substr(position + 1, end - position - 1);
        std::string replacement;
        if (entity == "lt")
            replacement = "<";
        else if (entity == "gt")
            replacement = ">";
        else if (entity == "amp")
            replacement = "&";
        else if (entity == "quot")
            replacement = "\"";
        else if (entity == "apos")
            replacement = "'";
        else if (entity == "nbsp")
            replacement = "\xC2\xA0";
        else if (!appendNumericReference(replacement, entity)) {
            position = end + 1;
            continue;
        }
        result.append(value, run, position - run);
        result += replacement;
        position = end + 1;
        run = position;
    }
    result.append(value, run, value.size() - run);
    return result;
}

} // namespace Core
