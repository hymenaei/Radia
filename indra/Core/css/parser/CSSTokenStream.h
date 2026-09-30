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

namespace Core::CSS::detail {
enum class TokenKind : std::uint8_t {
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

inline constexpr std::size_t kNoMatchingToken = std::numeric_limits<std::size_t>::max();

struct Token {
    TokenKind kind;
    std::size_t begin = 0;
    std::size_t end = 0;
    std::size_t matching = kNoMatchingToken;
    bool precededByComment = false;
    std::optional<float> numericValue;
};

struct TokenRange {
    std::size_t begin = 0;
    std::size_t end = 0;
};

struct FunctionRange {
    std::string name;
    TokenRange body;
};

struct Dimension {
    std::string number;
    std::string unit;
};

class TokenStream {
public:
    explicit TokenStream(std::string_view source);

    std::string_view source() const { return mSource; }
    const std::vector<Token>& tokens() const { return mTokens; }
    std::string_view text(std::size_t index) const;
    bool hasBalancedBlocks() const;
    std::optional<std::size_t> unclosedCommentOffset() const { return mUnclosedCommentOffset; }

private:
    std::string mSource;
    std::vector<Token> mTokens;
    std::optional<std::size_t> mUnclosedCommentOffset;
};

struct ValueRange {
    const TokenStream& stream;
    TokenRange range;
};

bool isWhitespace(char value);
bool isTrivia(TokenKind kind);
std::string trim(const std::string& value);
std::string lower(std::string value);
std::string normalizeKeyword(std::string_view value);
std::string normalizeKeyword(const TokenStream& stream, TokenRange range);
std::optional<FunctionRange> parseFunction(const TokenStream& stream, TokenRange range);
std::optional<std::string> parseUrl(const TokenStream& stream, TokenRange range);
std::optional<Dimension> parseDimension(const TokenStream& stream, TokenRange range);
bool startsWith(const std::string& value, const std::string& prefix);
bool endsWith(const std::string& value, const std::string& suffix);
std::pair<std::size_t, std::size_t> sourcePosition(std::string_view source, std::size_t offset);
std::string decodeIdentifier(std::string_view value);
std::size_t skipComponent(const TokenStream& stream, std::size_t index, std::size_t end);
TokenRange trimRange(const TokenStream& stream, TokenRange range);
std::string serializeRange(const TokenStream& stream, TokenRange range);
std::optional<std::string> decodeString(std::string_view value);
std::vector<TokenRange> splitComponents(const TokenStream& stream, TokenRange range, bool splitSlash = false);
std::vector<TokenRange> splitOnDelimiter(const TokenStream& stream, TokenRange range, char delimiter);
std::vector<std::string> tokenizeTopLevel(const TokenStream& stream, TokenRange range, bool splitSlash = false);
std::vector<std::string> tokenizeTopLevel(const std::string& value, bool splitSlash = false);
std::vector<std::string> splitTopLevel(const std::string& value, char delimiter);
std::optional<std::size_t> matchingBlock(const std::string& value, std::size_t open);
} // namespace Core::CSS::detail
