/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "css/syntax.h"
#include <charconv>
#include <cctype>
#include <cmath>
#include <cstdint>
#include <limits>
#include <string>

namespace radia::ui::detail {
namespace {
constexpr std::uint32_t kReplacementCodePoint = 0xfffd;

struct CSSCodePoint {
    std::uint32_t value = kReplacementCodePoint;
    std::size_t width = 1;
    bool valid = false;
};

bool isUTF8Continuation(unsigned char value) {
    return value >= 0x80 && value <= 0xbf;
}

CSSCodePoint decodeCSSCodePoint(std::string_view source, std::size_t position) {
    if (position >= source.size()) return {};
    const auto first = static_cast<unsigned char>(source[position]);
    if (first <= 0x7f) return {first, 1, true};

    if (first >= 0xc2 && first <= 0xdf) {
        if (position + 1 < source.size() && isUTF8Continuation(static_cast<unsigned char>(source[position + 1])))
            return {static_cast<std::uint32_t>(((first & 0x1f) << 6) | (static_cast<unsigned char>(source[position + 1]) & 0x3f)), 2, true};
        return {};
    }

    if (first >= 0xe0 && first <= 0xef) {
        if (position + 2 >= source.size()) return {};
        const auto second = static_cast<unsigned char>(source[position + 1]);
        const auto third = static_cast<unsigned char>(source[position + 2]);
        const bool secondValid = first == 0xe0 ? second >= 0xa0 && second <= 0xbf
            : first == 0xed                    ? second >= 0x80 && second <= 0x9f
                                               : isUTF8Continuation(second);
        if (isUTF8Continuation(third) && secondValid) {
            const std::uint32_t value = static_cast<std::uint32_t>(((first & 0x0f) << 12) | ((second & 0x3f) << 6) | (third & 0x3f));
            return {value, 3, true};
        }
        if (first == 0xed && second >= 0xa0 && second <= 0xbf && isUTF8Continuation(third)) return {kReplacementCodePoint, 3, false};
        return {};
    }

    if (first >= 0xf0 && first <= 0xf4) {
        if (position + 3 >= source.size()) return {};
        const auto second = static_cast<unsigned char>(source[position + 1]);
        const auto third = static_cast<unsigned char>(source[position + 2]);
        const auto fourth = static_cast<unsigned char>(source[position + 3]);
        const bool secondValid = first == 0xf0 ? second >= 0x90 && second <= 0xbf
            : first == 0xf4                    ? second >= 0x80 && second <= 0x8f
                                               : isUTF8Continuation(second);
        if (secondValid && isUTF8Continuation(third) && isUTF8Continuation(fourth))
            return {static_cast<std::uint32_t>(((first & 0x07) << 18) | ((second & 0x3f) << 12) | ((third & 0x3f) << 6) | (fourth & 0x3f)), 4, true};
        return {};
    }
    return {};
}

bool isCSSWhitespaceCodePoint(std::uint32_t value) {
    return value == '\t' || value == '\n' || value == '\f' || value == '\r' || value == ' ';
}

bool isCSSNameStart(std::uint32_t value) {
    return (value >= 'a' && value <= 'z') || (value >= 'A' && value <= 'Z') || value == '_' || value >= 0x80;
}

bool isCSSNameCodePoint(std::uint32_t value) {
    return isCSSNameStart(value) || (value >= '0' && value <= '9') || value == '-';
}

bool isCSSHexDigit(char value) {
    const auto character = static_cast<unsigned char>(value);
    return (character >= '0' && character <= '9') || (character >= 'a' && character <= 'f') || (character >= 'A' && character <= 'F');
}

std::string preprocessCSS(std::string_view source) {
    std::string result;
    result.reserve(source.size());
    for (std::size_t position = 0; position < source.size();) {
        const CSSCodePoint codePoint = decodeCSSCodePoint(source, position);
        if (codePoint.value == '\r') {
            position += codePoint.width;
            if (position < source.size() && source[position] == '\n') ++position;
            result.push_back('\n');
        } else if (codePoint.value == '\f') {
            ++position;
            result.push_back('\n');
        } else if (codePoint.value == 0 || !codePoint.valid) {
            result.append("\xEF\xBF\xBD");
            position += codePoint.width;
        } else {
            result.append(source.substr(position, codePoint.width));
            position += codePoint.width;
        }
    }
    return result;
}

std::size_t normalizedCSSOffset(std::string_view source, std::size_t originalOffset) {
    return preprocessCSS(source.substr(0, originalOffset)).size();
}

std::size_t originalCSSOffset(std::string_view source, std::size_t normalizedOffset) {
    std::size_t original = 0;
    std::size_t normalized = 0;
    while (original < source.size() && normalized < normalizedOffset) {
        const CSSCodePoint codePoint = decodeCSSCodePoint(source, original);
        if (codePoint.value == '\r' && original + codePoint.width < source.size() && source[original + codePoint.width] == '\n') {
            original += codePoint.width + 1;
            ++normalized;
        } else if (codePoint.value == '\f' || codePoint.value == '\r') {
            original += codePoint.width;
            ++normalized;
        } else if (codePoint.value == 0 || !codePoint.valid) {
            original += codePoint.width;
            normalized += 3;
        } else {
            original += codePoint.width;
            normalized += codePoint.width;
        }
    }
    return original;
}

bool isValidCSSEscape(std::string_view source, std::size_t position) {
    return position + 1 < source.size()
        && source[position] == '\\'
        && source[position + 1] != '\n'
        && source[position + 1] != '\r'
        && source[position + 1] != '\f';
}

bool startsIdentifier(std::string_view source, std::size_t position) {
    if (position >= source.size()) return false;
    const CSSCodePoint first = decodeCSSCodePoint(source, position);
    if (first.value == '-') {
        const std::size_t secondPosition = position + first.width;
        if (secondPosition >= source.size()) return false;
        const CSSCodePoint second = decodeCSSCodePoint(source, secondPosition);
        return second.value == '-' || isCSSNameStart(second.value) || isValidCSSEscape(source, secondPosition);
    }
    return isCSSNameStart(first.value) || isValidCSSEscape(source, position);
}

bool startsNumber(std::string_view source, std::size_t position) {
    if (position >= source.size()) return false;
    if (source[position] == '+' || source[position] == '-') {
        ++position;
        if (position >= source.size()) return false;
    }
    if (std::isdigit(static_cast<unsigned char>(source[position]))) return true;
    return source[position] == '.' && position + 1 < source.size() && std::isdigit(static_cast<unsigned char>(source[position + 1]));
}

std::size_t consumeEscape(std::string_view source, std::size_t position) {
    ++position;
    if (position >= source.size()) return position;
    if (source[position] == '\r') {
        ++position;
        if (position < source.size() && source[position] == '\n') ++position;
        return position;
    }
    if (source[position] == '\n' || source[position] == '\f') return position + 1;
    if (!isCSSHexDigit(source[position])) return position + decodeCSSCodePoint(source, position).width;
    std::size_t digits = 0;
    while (position < source.size() && digits < 6 && isCSSHexDigit(source[position])) {
        ++position;
        ++digits;
    }
    if (position < source.size() && isCSSWhitespace(source[position])) ++position;
    return position;
}

std::size_t consumeName(std::string_view source, std::size_t position) {
    while (position < source.size()) {
        const CSSCodePoint codePoint = decodeCSSCodePoint(source, position);
        if (isCSSNameCodePoint(codePoint.value)) position += codePoint.width;
        else if (isValidCSSEscape(source, position)) position = consumeEscape(source, position);
        else break;
    }
    return position;
}

std::size_t consumeNumber(std::string_view source, std::size_t position) {
    if (position < source.size() && (source[position] == '+' || source[position] == '-')) ++position;
    while (position < source.size() && std::isdigit(static_cast<unsigned char>(source[position]))) ++position;
    if (position + 1 < source.size() && source[position] == '.' && std::isdigit(static_cast<unsigned char>(source[position + 1]))) {
        position += 2;
        while (position < source.size() && std::isdigit(static_cast<unsigned char>(source[position]))) ++position;
    }
    if (position < source.size() && (source[position] == 'e' || source[position] == 'E')) {
        std::size_t exponent = position + 1;
        if (exponent < source.size() && (source[exponent] == '+' || source[exponent] == '-')) ++exponent;
        const std::size_t digits = exponent;
        while (exponent < source.size() && std::isdigit(static_cast<unsigned char>(source[exponent]))) ++exponent;
        if (exponent != digits) position = exponent;
    }
    return position;
}

std::optional<float> parseCSSNumber(std::string_view source) {
    if (!source.empty() && source.front() == '+') source.remove_prefix(1);

    float value = 0.f;
    const auto [end, error] = std::from_chars(source.data(), source.data() + source.size(), value, std::chars_format::general);
    if (error != std::errc{} || end != source.data() + source.size() || !std::isfinite(value)) return std::nullopt;
    return value;
}

bool isNonPrintable(std::uint32_t value) {
    return value == 0 || (value >= 0x01 && value <= 0x08) || value == 0x0b || (value >= 0x0e && value <= 0x1f) || (value >= 0x7f && value <= 0x9f);
}

struct URLTokenEnd {
    std::size_t end = 0;
    bool bad = false;
};

URLTokenEnd consumeUnquotedURL(std::string_view source, std::size_t position) {
    bool bad = false;
    while (position < source.size()) {
        const CSSCodePoint codePoint = decodeCSSCodePoint(source, position);
        if (codePoint.value == ')') return {position + codePoint.width, bad};
        if (isCSSWhitespaceCodePoint(codePoint.value)) {
            while (position < source.size() && isCSSWhitespaceCodePoint(decodeCSSCodePoint(source, position).value))
                position += decodeCSSCodePoint(source, position).width;
            if (position == source.size()) return {position, bad};
            if (source[position] == ')') return {position + 1, bad};
            bad = true;
            continue;
        }
        if (codePoint.value == '\\') {
            if (!isValidCSSEscape(source, position)) {
                bad = true;
                position += codePoint.width;
            } else position = consumeEscape(source, position);
            continue;
        }
        if (codePoint.value == '\'' || codePoint.value == '"' || codePoint.value == '(' || isNonPrintable(codePoint.value)) bad = true;
        position += codePoint.width;
    }
    return {position, bad};
}

bool isOpeningBlock(CSSTokenKind kind) {
    return kind == CSSTokenKind::Function || kind == CSSTokenKind::OpenParen || kind == CSSTokenKind::OpenBracket || kind == CSSTokenKind::OpenBrace;
}

bool isClosingBlock(CSSTokenKind kind) {
    return kind == CSSTokenKind::CloseParen || kind == CSSTokenKind::CloseBracket || kind == CSSTokenKind::CloseBrace;
}

bool isMatchingBlock(CSSTokenKind open, CSSTokenKind close) {
    return (open == CSSTokenKind::Function || open == CSSTokenKind::OpenParen) && close == CSSTokenKind::CloseParen
        || open == CSSTokenKind::OpenBracket && close == CSSTokenKind::CloseBracket
        || open == CSSTokenKind::OpenBrace && close == CSSTokenKind::CloseBrace;
}

std::size_t trimTokenBegin(const CSSTokenStream& stream, std::size_t begin, std::size_t end) {
    while (begin < end && isCSSTrivia(stream.tokens()[begin].kind)) ++begin;
    return begin;
}

std::size_t trimTokenEnd(const CSSTokenStream& stream, std::size_t begin, std::size_t end) {
    while (end > begin && isCSSTrivia(stream.tokens()[end - 1].kind)) --end;
    return end;
}

bool hasBalancedRange(const CSSTokenStream& stream, CSSTokenRange range) {
    if (range.begin > range.end || range.end > stream.tokens().size()) return false;
    for (std::size_t index = range.begin; index < range.end; ++index) {
        const CSSToken& token = stream.tokens()[index];
        if ((isOpeningBlock(token.kind) || isClosingBlock(token.kind))
            && (token.matching == kNoMatchingCSSToken || token.matching < range.begin || token.matching >= range.end))
            return false;
    }
    return true;
}
} // namespace

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
    for (char& character : value)
        if (character >= 'A' && character <= 'Z') character = static_cast<char>(character - 'A' + 'a');
    return value;
}

CSSTokenStream::CSSTokenStream(std::string_view source) : mSource(preprocessCSS(source)) {
    struct OpenBlock {
        std::size_t token = 0;
        CSSTokenKind kind = CSSTokenKind::OpenParen;
    };

    std::vector<OpenBlock> blocks;
    bool commentBeforeNextToken = false;
    const auto addToken = [this, &commentBeforeNextToken](CSSTokenKind kind, std::size_t begin, std::size_t end) {
        std::optional<float> numericValue;
        if (kind == CSSTokenKind::Number || kind == CSSTokenKind::Dimension || kind == CSSTokenKind::Percentage) {
            const std::string_view token = mSource.substr(begin, end - begin);
            numericValue = parseCSSNumber(token.substr(0, consumeNumber(token, 0)));
        }
        mTokens.push_back({kind, begin, end, kNoMatchingCSSToken, commentBeforeNextToken, numericValue});
        commentBeforeNextToken = false;
        return mTokens.size() - 1;
    };
    const auto addOpenBlock = [&](CSSTokenKind kind, std::size_t begin, std::size_t end) {
        const std::size_t token = addToken(kind, begin, end);
        blocks.push_back({token, kind});
    };

    std::size_t position = 0;
    while (position < mSource.size()) {
        const std::size_t start = position;
        const char character = mSource[position];
        if (isCSSWhitespace(character)) {
            while (position < mSource.size() && isCSSWhitespace(mSource[position])) ++position;
            addToken(CSSTokenKind::Whitespace, start, position);
            continue;
        }
        if (character == '/' && position + 1 < mSource.size() && mSource[position + 1] == '*') {
            position += 2;
            while (position + 1 < mSource.size() && !(mSource[position] == '*' && mSource[position + 1] == '/')) ++position;
            if (position + 1 < mSource.size()) position += 2;
            else {
                position = mSource.size();
                if (!mUnclosedCommentOffset) mUnclosedCommentOffset = start;
            }
            commentBeforeNextToken = true;
            continue;
        }
        if (mSource.compare(position, 4, "<!--") == 0) {
            position += 4;
            addToken(CSSTokenKind::CDO, start, position);
            continue;
        }
        if (mSource.compare(position, 3, "-->") == 0) {
            position += 3;
            addToken(CSSTokenKind::CDC, start, position);
            continue;
        }
        if (character == '\'' || character == '"') {
            const char quote = character;
            ++position;
            bool bad = false;
            while (position < mSource.size()) {
                if (mSource[position] == quote) {
                    ++position;
                    break;
                }
                if (mSource[position] == '\n' || mSource[position] == '\r' || mSource[position] == '\f') {
                    bad = true;
                    break;
                }
                if (mSource[position] == '\\') {
                    if (position + 1 >= mSource.size()) {
                        ++position;
                        break;
                    }
                    position = consumeEscape(mSource, position);
                    continue;
                }
                ++position;
            }
            addToken(bad ? CSSTokenKind::BadString : CSSTokenKind::String, start, position);
            continue;
        }
        if (character == '(') {
            ++position;
            addOpenBlock(CSSTokenKind::OpenParen, start, position);
            continue;
        }
        if (character == '[') {
            ++position;
            addOpenBlock(CSSTokenKind::OpenBracket, start, position);
            continue;
        }
        if (character == '{') {
            ++position;
            addOpenBlock(CSSTokenKind::OpenBrace, start, position);
            continue;
        }
        if (character == ')' || character == ']' || character == '}') {
            ++position;
            const CSSTokenKind kind = character == ')' ? CSSTokenKind::CloseParen
                : character == ']'                     ? CSSTokenKind::CloseBracket
                                                       : CSSTokenKind::CloseBrace;
            const std::size_t token = addToken(kind, start, position);
            if (!blocks.empty() && isMatchingBlock(blocks.back().kind, kind)) {
                mTokens[blocks.back().token].matching = token;
                mTokens[token].matching = blocks.back().token;
                blocks.pop_back();
            }
            continue;
        }
        if (character == '@' && startsIdentifier(mSource, position + 1)) {
            position = consumeName(mSource, position + 1);
            addToken(CSSTokenKind::AtKeyword, start, position);
            continue;
        }
        if (character == '#'
            && (position + 1 < mSource.size()
                && (isCSSNameCodePoint(decodeCSSCodePoint(mSource, position + 1).value) || isValidCSSEscape(mSource, position + 1)))) {
            position = consumeName(mSource, position + 1);
            addToken(CSSTokenKind::Hash, start, position);
            continue;
        }
        if (startsNumber(mSource, position)) {
            position = consumeNumber(mSource, position);
            CSSTokenKind kind = CSSTokenKind::Number;
            if (position < mSource.size() && mSource[position] == '%') {
                ++position;
                kind = CSSTokenKind::Percentage;
            } else if (startsIdentifier(mSource, position)) {
                position = consumeName(mSource, position);
                kind = CSSTokenKind::Dimension;
            }
            addToken(kind, start, position);
            continue;
        }
        if (startsIdentifier(mSource, position)) {
            position = consumeName(mSource, position);
            if (position < mSource.size() && mSource[position] == '(') {
                const std::string name = lower(decodeCSSIdentifier(mSource.substr(start, position - start)));
                if (name == "url") {
                    std::size_t urlPosition = position + 1;
                    while (urlPosition < mSource.size() && isCSSWhitespace(mSource[urlPosition])) ++urlPosition;
                    if (urlPosition == mSource.size() || (mSource[urlPosition] != '\'' && mSource[urlPosition] != '"')) {
                        const URLTokenEnd url = consumeUnquotedURL(mSource, urlPosition);
                        position = url.end;
                        addToken(url.bad ? CSSTokenKind::BadUrl : CSSTokenKind::Url, start, position);
                        continue;
                    }
                }
                ++position;
                addOpenBlock(CSSTokenKind::Function, start, position);
                continue;
            }
            addToken(CSSTokenKind::Ident, start, position);
            continue;
        }
        if (character == '\\' && isValidCSSEscape(mSource, position)) {
            position = consumeName(mSource, position);
            addToken(CSSTokenKind::Ident, start, position);
            continue;
        }
        if (character == ':') {
            ++position;
            addToken(CSSTokenKind::Colon, start, position);
            continue;
        }
        if (character == ';') {
            ++position;
            addToken(CSSTokenKind::Semicolon, start, position);
            continue;
        }
        if (character == ',') {
            ++position;
            addToken(CSSTokenKind::Comma, start, position);
            continue;
        }
        ++position;
        addToken(CSSTokenKind::Delim, start, position);
    }
}

std::string_view CSSTokenStream::text(std::size_t index) const {
    if (index >= mTokens.size()) return {};
    const CSSToken& token = mTokens[index];
    return std::string_view(mSource).substr(token.begin, token.end - token.begin);
}

bool CSSTokenStream::hasBalancedBlocks() const {
    for (const CSSToken& token : mTokens)
        if ((isOpeningBlock(token.kind) || isClosingBlock(token.kind)) && token.matching == kNoMatchingCSSToken) return false;
    return true;
}

bool isCSSTrivia(CSSTokenKind kind) {
    return kind == CSSTokenKind::Whitespace;
}

bool startsWith(const std::string& value, const std::string& prefix) {
    return value.rfind(prefix, 0) == 0;
}

bool endsWith(const std::string& value, const std::string& suffix) {
    return value.size() >= suffix.size() && value.compare(value.size() - suffix.size(), suffix.size(), suffix) == 0;
}

std::pair<std::size_t, std::size_t> cssSourcePosition(std::string_view source, std::size_t offset) {
    std::size_t line = 1;
    std::size_t column = 1;
    for (std::size_t index = 0; index < offset && index < source.size();) {
        const CSSCodePoint codePoint = decodeCSSCodePoint(source, index);
        if (codePoint.value == '\r') {
            ++line;
            column = 1;
            index += codePoint.width;
            if (index < offset && index < source.size() && source[index] == '\n') ++index;
        } else if (codePoint.value == '\n' || codePoint.value == '\f') {
            ++line;
            column = 1;
            index += codePoint.width;
        } else {
            ++column;
            index += codePoint.width;
        }
    }
    return {line, column};
}

void appendCSSCodePoint(std::string& result, std::uint32_t codePoint) {
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
}

std::string decodeCSSIdentifier(std::string_view value) {
    const std::string normalized = preprocessCSS(value);
    std::string result;
    result.reserve(normalized.size());
    for (std::size_t position = 0; position < normalized.size();) {
        const CSSCodePoint decoded = decodeCSSCodePoint(normalized, position);
        if (decoded.value != '\\') {
            result.append(normalized, position, decoded.width);
            position += decoded.width;
            continue;
        }
        ++position;
        if (position >= normalized.size()) {
            result.push_back('\\');
            continue;
        }
        if (normalized[position] == '\n' || normalized[position] == '\f') {
            ++position;
            continue;
        }
        if (normalized[position] == '\r') {
            ++position;
            if (position < normalized.size() && normalized[position] == '\n') ++position;
            continue;
        }
        if (!isCSSHexDigit(normalized[position])) {
            result.push_back(normalized[position++]);
            continue;
        }
        std::uint32_t codePoint = 0;
        std::size_t digits = 0;
        while (position < normalized.size() && digits < 6 && isCSSHexDigit(normalized[position])) {
            codePoint *= 16;
            const auto digit = static_cast<unsigned char>(normalized[position++]);
            codePoint += digit <= '9' ? digit - '0' : (digit <= 'F' ? digit - 'A' + 10 : digit - 'a' + 10);
            ++digits;
        }
        if (position < normalized.size() && isCSSWhitespace(normalized[position])) ++position;
        appendCSSCodePoint(result, codePoint);
    }
    return result;
}

std::string normalizeCSSKeyword(std::string_view value) {
    const std::string trimmed = trim(std::string(value));
    const CSSTokenStream stream(trimmed);
    return normalizeCSSKeyword(stream, {0, stream.tokens().size()});
}

std::string normalizeCSSKeyword(const CSSTokenStream& stream, CSSTokenRange range) {
    range = trimCSSRange(stream, range);
    bool identifiersOnly = range.begin != range.end;
    for (std::size_t index = range.begin; index < range.end; ++index) {
        const CSSToken& token = stream.tokens()[index];
        if (token.kind != CSSTokenKind::Ident && token.kind != CSSTokenKind::Whitespace) identifiersOnly = false;
    }
    if (!identifiersOnly) return lower(trim(serializeCSSRange(stream, range)));

    std::string result;
    for (std::size_t index = range.begin; index < range.end; ++index) {
        const CSSToken& token = stream.tokens()[index];
        if (token.kind == CSSTokenKind::Ident) {
            if (index != range.begin && token.precededByComment && !result.empty() && !isCSSWhitespace(result.back())) result.push_back(' ');
            result += lower(decodeCSSIdentifier(stream.text(index)));
        } else {
            result += stream.text(index);
        }
    }
    return result;
}

std::optional<std::string> decodeCSSString(std::string_view value) {
    const std::string normalized = preprocessCSS(value);
    if (normalized.size() < 2 || (normalized.front() != '\'' && normalized.front() != '"') || normalized.back() != normalized.front())
        return std::nullopt;
    std::string result;
    result.reserve(normalized.size() - 2);
    for (std::size_t position = 1; position + 1 < normalized.size();) {
        const CSSCodePoint decoded = decodeCSSCodePoint(normalized, position);
        position += decoded.width;
        if (decoded.value != '\\') {
            result.append(normalized, position - decoded.width, decoded.width);
            continue;
        }
        if (position + 1 >= normalized.size()) return std::nullopt;
        if (normalized[position] == '\n' || normalized[position] == '\f') {
            ++position;
            continue;
        }
        if (normalized[position] == '\r') {
            ++position;
            if (position + 1 < normalized.size() && normalized[position] == '\n') ++position;
            continue;
        }
        if (!isCSSHexDigit(normalized[position])) {
            result.push_back(normalized[position++]);
            continue;
        }
        std::uint32_t codePoint = 0;
        std::size_t digits = 0;
        while (position + 1 < normalized.size() && digits < 6 && isCSSHexDigit(normalized[position])) {
            codePoint *= 16;
            const auto digit = static_cast<unsigned char>(normalized[position++]);
            codePoint += digit <= '9' ? digit - '0' : (digit <= 'F' ? digit - 'A' + 10 : digit - 'a' + 10);
            ++digits;
        }
        if (position + 1 < normalized.size() && isCSSWhitespace(normalized[position])) ++position;
        appendCSSCodePoint(result, codePoint);
    }
    return result;
}

std::size_t skipCSSComponent(const CSSTokenStream& stream, std::size_t index, std::size_t end) {
    if (index >= end || index >= stream.tokens().size()) return end;
    const CSSToken& token = stream.tokens()[index];
    if (!isOpeningBlock(token.kind)) return index + 1;
    if (token.matching == kNoMatchingCSSToken || token.matching >= end) return end;
    return token.matching + 1;
}

CSSTokenRange trimCSSRange(const CSSTokenStream& stream, CSSTokenRange range) {
    while (range.begin < range.end && isCSSTrivia(stream.tokens()[range.begin].kind)) ++range.begin;
    while (range.end > range.begin && isCSSTrivia(stream.tokens()[range.end - 1].kind)) --range.end;
    return range;
}

std::string serializeCSSRange(const CSSTokenStream& stream, CSSTokenRange range) {
    if (range.begin > range.end || range.end > stream.tokens().size()) return {};
    std::string result;
    for (std::size_t index = range.begin; index < range.end; ++index) {
        if (index != range.begin && stream.tokens()[index].precededByComment) {
            const bool triviaBoundary = isCSSTrivia(stream.tokens()[index - 1].kind) || isCSSTrivia(stream.tokens()[index].kind);
            if (!triviaBoundary) {
                const std::string merged = std::string(stream.text(index - 1)) + std::string(stream.text(index));
                const CSSTokenStream mergedStream(merged);
                if (mergedStream.tokens().size() != 2
                    || mergedStream.tokens()[0].kind != stream.tokens()[index - 1].kind
                    || mergedStream.tokens()[1].kind != stream.tokens()[index].kind)
                    result.append("/**/");
            }
        }
        result.append(stream.text(index));
    }
    return result;
}

std::optional<CSSFunctionRange> parseCSSFunction(const CSSTokenStream& stream, CSSTokenRange range) {
    range = trimCSSRange(stream, range);
    if (!hasBalancedRange(stream, range)) return std::nullopt;

    std::vector<std::size_t> significant;
    for (std::size_t index = range.begin; index < range.end; ++index)
        if (!isCSSTrivia(stream.tokens()[index].kind)) significant.push_back(index);
    if (significant.size() < 2) return std::nullopt;

    const std::size_t function = significant.front();
    const std::size_t close = significant.back();
    const CSSToken& functionToken = stream.tokens()[function];
    if (functionToken.kind != CSSTokenKind::Function || functionToken.matching != close || stream.tokens()[close].kind != CSSTokenKind::CloseParen)
        return std::nullopt;

    const std::string_view functionText = stream.text(function);
    if (functionText.empty() || functionText.back() != '(') return std::nullopt;
    return CSSFunctionRange{lower(decodeCSSIdentifier(functionText.substr(0, functionText.size() - 1))), {function + 1, close}};
}

std::optional<std::string> parseCSSUrl(const CSSTokenStream& stream, CSSTokenRange range) {
    range = trimCSSRange(stream, range);
    const auto& tokens = stream.tokens();
    std::vector<std::size_t> significant;
    for (std::size_t index = range.begin; index < range.end; ++index)
        if (!isCSSTrivia(tokens[index].kind)) significant.push_back(index);
    if (significant.size() == 1 && tokens[significant.front()].kind == CSSTokenKind::Url) {
        const std::string_view token = stream.text(significant.front());
        const std::size_t open = token.find('(');
        if (open == std::string_view::npos || token.empty() || token.back() != ')') return std::nullopt;
        const std::string decoded = decodeCSSIdentifier(trim(std::string(token.substr(open + 1, token.size() - open - 2))));
        return decoded.empty() ? std::nullopt : std::optional<std::string>(decoded);
    }
    if (significant.size() != 3 || tokens[significant[0]].kind != CSSTokenKind::Function || tokens[significant[2]].kind != CSSTokenKind::CloseParen)
        return std::nullopt;
    const auto function = parseCSSFunction(stream, range);
    if (!function
        || function->name != "url"
        || tokens[significant[0]].matching != significant[2]
        || tokens[significant[1]].kind != CSSTokenKind::String)
        return std::nullopt;
    const std::optional<std::string> decoded = decodeCSSString(stream.text(significant[1]));
    return decoded && !decoded->empty() ? decoded : std::nullopt;
}

std::optional<CSSDimension> parseCSSDimension(const CSSTokenStream& stream, CSSTokenRange range) {
    range = trimCSSRange(stream, range);
    std::vector<std::size_t> significant;
    for (std::size_t index = range.begin; index < range.end; ++index)
        if (!isCSSTrivia(stream.tokens()[index].kind)) significant.push_back(index);
    if (significant.size() != 1 || stream.tokens()[significant.front()].kind != CSSTokenKind::Dimension) return std::nullopt;

    const std::string_view dimensionText = stream.text(significant.front());
    const std::size_t numberEnd = consumeNumber(dimensionText, 0);
    if (numberEnd == 0 || numberEnd == dimensionText.size()) return std::nullopt;
    return CSSDimension{std::string(dimensionText.substr(0, numberEnd)), lower(decodeCSSIdentifier(dimensionText.substr(numberEnd)))};
}

std::vector<CSSTokenRange> splitCSSComponents(const CSSTokenStream& stream, CSSTokenRange range, bool splitSlash) {
    if (!hasBalancedRange(stream, range)) return {};

    std::vector<CSSTokenRange> result;
    std::size_t component = kNoMatchingCSSToken;
    const auto finish = [&](std::size_t end) {
        if (component == kNoMatchingCSSToken) return;
        const std::size_t begin = trimTokenBegin(stream, component, end);
        const std::size_t trimmedEnd = trimTokenEnd(stream, begin, end);
        if (begin < trimmedEnd) result.push_back({begin, trimmedEnd});
        component = kNoMatchingCSSToken;
    };

    for (std::size_t index = range.begin; index < range.end;) {
        if (isCSSTrivia(stream.tokens()[index].kind)) {
            finish(index);
            while (index < range.end && isCSSTrivia(stream.tokens()[index].kind)) ++index;
            continue;
        }
        if (component != kNoMatchingCSSToken && stream.tokens()[index].precededByComment) finish(index);
        if (splitSlash && stream.tokens()[index].kind == CSSTokenKind::Delim && stream.text(index) == "/") {
            finish(index);
            result.push_back({index, index + 1});
            ++index;
            continue;
        }
        if (component == kNoMatchingCSSToken) component = index;
        index = skipCSSComponent(stream, index, range.end);
    }
    finish(range.end);
    return result;
}

std::vector<CSSTokenRange> splitCSSOnDelimiter(const CSSTokenStream& stream, CSSTokenRange range, char delimiter) {
    if (!hasBalancedRange(stream, range)) return {};

    const auto isDelimiter = [&](std::size_t index) {
        const CSSTokenKind kind = stream.tokens()[index].kind;
        if ((delimiter == ':' && kind == CSSTokenKind::Colon)
            || (delimiter == ';' && kind == CSSTokenKind::Semicolon)
            || (delimiter == ',' && kind == CSSTokenKind::Comma))
            return true;
        return kind == CSSTokenKind::Delim && stream.text(index).size() == 1 && stream.text(index)[0] == delimiter;
    };

    std::vector<CSSTokenRange> result;
    std::size_t start = range.begin;
    for (std::size_t index = range.begin; index < range.end;) {
        if (isOpeningBlock(stream.tokens()[index].kind)) {
            index = skipCSSComponent(stream, index, range.end);
            continue;
        }
        if (isDelimiter(index)) {
            const std::size_t begin = trimTokenBegin(stream, start, index);
            const std::size_t end = trimTokenEnd(stream, begin, index);
            result.push_back({begin, end});
            start = index + 1;
        }
        ++index;
    }
    const std::size_t begin = trimTokenBegin(stream, start, range.end);
    const std::size_t end = trimTokenEnd(stream, begin, range.end);
    result.push_back({begin, end});
    return result;
}

std::vector<std::string> tokenizeTopLevel(const CSSTokenStream& stream, CSSTokenRange range, bool splitSlash) {
    if (!hasBalancedRange(stream, range)) return {};
    std::vector<std::string> result;
    for (const CSSTokenRange component : splitCSSComponents(stream, range, splitSlash)) result.push_back(trim(serializeCSSRange(stream, component)));
    return result;
}

std::vector<std::string> tokenizeTopLevel(const std::string& value, bool splitSlash) {
    const CSSTokenStream stream(value);
    return tokenizeTopLevel(stream, {0, stream.tokens().size()}, splitSlash);
}

std::vector<std::string> splitTopLevel(const std::string& value, char delimiter) {
    const CSSTokenStream stream(value);
    if (!stream.hasBalancedBlocks()) return {};
    std::vector<std::string> result;
    for (const CSSTokenRange range : splitCSSOnDelimiter(stream, {0, stream.tokens().size()}, delimiter))
        result.push_back(trim(serializeCSSRange(stream, range)));
    return result;
}

std::optional<std::size_t> matchingBlock(const std::string& value, std::size_t open) {
    if (open >= value.size() || value[open] != '{') return std::nullopt;
    const CSSTokenStream stream(value);
    const std::size_t normalizedOpen = normalizedCSSOffset(value, open);
    for (std::size_t index = 0; index < stream.tokens().size(); ++index) {
        const CSSToken& token = stream.tokens()[index];
        if (token.kind == CSSTokenKind::OpenBrace && token.begin == normalizedOpen && token.matching != kNoMatchingCSSToken)
            return originalCSSOffset(value, stream.tokens()[token.matching].begin);
    }
    return std::nullopt;
}
} // namespace radia::ui::detail
