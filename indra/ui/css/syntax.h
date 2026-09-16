/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <limits>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace radia::ui::detail {
enum class CSSTokenKind : std::uint8_t {
    Whitespace,
    Ident,
    Function,
    AtKeyword,
    Hash,
    String,
    BadString,
    Url,
    BadUrl,
    Number,
    Dimension,
    Percentage,
    Colon,
    Semicolon,
    Comma,
    CDO,
    CDC,
    Delim,
    OpenParen,
    CloseParen,
    OpenBracket,
    CloseBracket,
    OpenBrace,
    CloseBrace
};

inline constexpr std::size_t kNoMatchingCSSToken = std::numeric_limits<std::size_t>::max();

struct CSSToken {
    CSSTokenKind kind;
    std::size_t begin = 0;
    std::size_t end = 0;
    std::size_t matching = kNoMatchingCSSToken;
    bool precededByComment = false;
};

struct CSSTokenRange {
    std::size_t begin = 0;
    std::size_t end = 0;
};

struct CSSFunctionRange {
    std::string name;
    CSSTokenRange body;
};

struct CSSDimension {
    std::string number;
    std::string unit;
};

class CSSTokenStream {
public:
    explicit CSSTokenStream(std::string_view source);

    std::string_view source() const { return mSource; }
    const std::vector<CSSToken>& tokens() const { return mTokens; }
    std::string_view text(std::size_t index) const;
    bool hasBalancedBlocks() const;
    std::optional<std::size_t> unclosedCommentOffset() const { return mUnclosedCommentOffset; }

private:
    std::string mSource;
    std::vector<CSSToken> mTokens;
    std::optional<std::size_t> mUnclosedCommentOffset;
};

struct CSSValueRange {
    const CSSTokenStream& stream;
    CSSTokenRange range;
};

bool isCSSWhitespace(char value);
bool isCSSTrivia(CSSTokenKind kind);
std::string trim(const std::string& value);
std::string lower(std::string value);
std::string normalizeCSSKeyword(std::string_view value);
std::string normalizeCSSKeyword(const CSSTokenStream& stream, CSSTokenRange range);
std::optional<CSSFunctionRange> parseCSSFunction(const CSSTokenStream& stream, CSSTokenRange range);
std::optional<CSSDimension> parseCSSDimension(const CSSTokenStream& stream, CSSTokenRange range);
bool startsWith(const std::string& value, const std::string& prefix);
bool endsWith(const std::string& value, const std::string& suffix);
std::pair<std::size_t, std::size_t> cssSourcePosition(std::string_view source, std::size_t offset);
std::string decodeCSSIdentifier(std::string_view value);
std::size_t skipCSSComponent(const CSSTokenStream& stream, std::size_t index, std::size_t end);
CSSTokenRange trimCSSRange(const CSSTokenStream& stream, CSSTokenRange range);
std::string serializeCSSRange(const CSSTokenStream& stream, CSSTokenRange range);
std::optional<std::string> decodeCSSString(std::string_view value);
std::vector<CSSTokenRange> splitCSSComponents(const CSSTokenStream& stream, CSSTokenRange range, bool splitSlash = false);
std::vector<CSSTokenRange> splitCSSOnDelimiter(const CSSTokenStream& stream, CSSTokenRange range, char delimiter);
std::vector<std::string> tokenizeTopLevel(const CSSTokenStream& stream, CSSTokenRange range, bool splitSlash = false);
std::vector<std::string> tokenizeTopLevel(const std::string& value, bool splitSlash = false);
std::vector<std::string> splitTopLevel(const std::string& value, char delimiter);
std::optional<std::size_t> matchingBlock(const std::string& value, std::size_t open);
} // namespace radia::ui::detail
