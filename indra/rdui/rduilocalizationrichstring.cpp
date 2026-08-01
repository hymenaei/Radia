/**
 * @file rduilocalizationrichstring.cpp
 * @brief
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
#include <algorithm>
#include <array>
#include <cctype>
#include <optional>
#include <regex>
#include <string_view>
#include "rduilocalizationinternal.h"

namespace rdui::localization_detail {
namespace {
bool validArgumentName(const std::string& id) {
    static const std::regex pattern(R"(^[a-z][A-Za-z0-9]*$)");
    return std::regex_match(id, pattern);
}

bool validBindingId(const std::string& value) {
    static const std::regex pattern(R"(^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$)");
    return std::regex_match(value, pattern);
}

enum class RichTokenKind : std::uint8_t { Text, Argument, ContainerOpen, ContainerClose, Kbd, Br };

struct ContainerTag {
    std::string_view open;
    std::string_view close;
    TemplateKind kind;
};

constexpr std::array CONTAINER_TAGS{
    ContainerTag{"<b>", "</b>", TemplateKind::B},
    ContainerTag{"<i>", "</i>", TemplateKind::I},
    ContainerTag{"<s>", "</s>", TemplateKind::S},
};

struct RichToken {
    RichTokenKind kind;
    std::string value;
    std::size_t offset = 0;
    const ContainerTag* container = nullptr;
};

struct ScannedToken {
    RichToken token;
    std::size_t length = 0;
};

std::optional<ScannedToken> scanRecognizedTag(const std::string& source, std::size_t offset) {
    for (const ContainerTag& tag : CONTAINER_TAGS) {
        if (source.compare(offset, tag.open.size(), tag.open) == 0)
            return ScannedToken{{RichTokenKind::ContainerOpen, std::string(tag.open), offset, &tag}, tag.open.size()};
        if (source.compare(offset, tag.close.size(), tag.close) == 0)
            return ScannedToken{{RichTokenKind::ContainerClose, std::string(tag.close), offset, &tag}, tag.close.size()};
    }

    constexpr std::string_view BR = "<br/>";
    if (source.compare(offset, BR.size(), BR) == 0) return ScannedToken{{RichTokenKind::Br, {}, offset, nullptr}, BR.size()};

    constexpr std::string_view KBD_PREFIX = "<kbd binding=\"";
    if (source.compare(offset, KBD_PREFIX.size(), KBD_PREFIX) != 0) return std::nullopt;

    const std::size_t binding_begin = offset + KBD_PREFIX.size();
    const std::size_t end = source.find("\"/>", binding_begin);
    if (end == std::string::npos) return std::nullopt;

    std::string binding = source.substr(binding_begin, end - binding_begin);
    if (!validBindingId(binding)) return std::nullopt;
    return ScannedToken{{RichTokenKind::Kbd, std::move(binding), offset, nullptr}, end + 3 - offset};
}

class RichStringTokenizer {
public:
    RichStringTokenizer(const std::string& source, LocalizationLoadResult& result, const std::string& source_name, std::size_t line)
        : mSource(source), mResult(result), mSourceName(source_name), mFirstLine(line) {}

    bool tokenize(std::vector<RichToken>& tokens) {
        std::string text;
        std::size_t text_offset = 0;
        const auto flush = [&]() {
            if (!text.empty()) {
                tokens.push_back({RichTokenKind::Text, std::move(text), text_offset, nullptr});
                text.clear();
            }
        };
        const auto append = [&](std::string value, std::size_t offset) {
            if (text.empty()) text_offset = offset;
            text += value;
        };

        std::size_t offset = 0;
        while (offset < mSource.size()) {
            const std::size_t token_offset = offset;
            if (mSource[offset] == '\\') {
                std::optional<ScannedToken> escaped;
                if (offset + 1 < mSource.size() && mSource[offset + 1] == '<') escaped = scanRecognizedTag(mSource, offset + 1);

                if (offset + 1 < mSource.size() && mSource[offset + 1] == '\\') {
                    append("\\", token_offset);
                    offset += 2;
                    continue;
                }
                if (escaped) {
                    append(mSource.substr(offset + 1, escaped->length), token_offset);
                    offset += escaped->length + 1;
                    continue;
                }
                const std::size_t placeholder_end =
                    offset + 1 < mSource.size() && mSource[offset + 1] == '{' ? mSource.find('}', offset + 2) : std::string::npos;
                if (placeholder_end != std::string::npos && validArgumentName(mSource.substr(offset + 2, placeholder_end - offset - 2))) {
                    append(mSource.substr(offset + 1, placeholder_end - offset), token_offset);
                    offset = placeholder_end + 1;
                    continue;
                }

                mResult.error("localization.string.escape_invalid",
                              offset + 1 >= mSource.size() ? "A trailing backslash is not a valid Rich String escape."
                                                           : "Backslash may escape only a backslash, recognized tag, or valid placeholder.",
                              mSourceName, tokenLine(token_offset));
                return false;
            }

            if (mSource[offset] == '\n') {
                flush();
                tokens.push_back({RichTokenKind::Br, {}, offset, nullptr});
                ++offset;
                continue;
            }

            if (mSource[offset] == '{') {
                const std::size_t end = mSource.find('}', offset + 1);
                if (end != std::string::npos) {
                    std::string name = mSource.substr(offset + 1, end - offset - 1);
                    if (validArgumentName(name)) {
                        flush();
                        tokens.push_back({RichTokenKind::Argument, std::move(name), offset, nullptr});
                        offset = end + 1;
                        continue;
                    }
                }
            }

            if (mSource[offset] == '<') {
                if (const auto tag = scanRecognizedTag(mSource, offset)) {
                    flush();
                    tokens.push_back(tag->token);
                    offset += tag->length;
                    continue;
                }

                const std::size_t end = mSource.find('>', offset + 1);
                const bool tag_shaped = offset + 1 < mSource.size()
                    && (std::isalpha(static_cast<unsigned char>(mSource[offset + 1]))
                        || (mSource[offset + 1] == '/'
                            && offset + 2 < mSource.size()
                            && std::isalpha(static_cast<unsigned char>(mSource[offset + 2]))));
                if (tag_shaped && end != std::string::npos) {
                    mResult.error("localization.string.tag_invalid",
                                  "Unknown, malformed, or misplaced Rich String tag: " + mSource.substr(offset, end - offset + 1) + ".", mSourceName,
                                  tokenLine(token_offset));
                    return false;
                }
            }

            append(std::string(1, mSource[offset]), token_offset);
            ++offset;
        }
        flush();
        return true;
    }

    std::size_t tokenLine(std::size_t offset) const {
        return mFirstLine + static_cast<std::size_t>(std::count(mSource.begin(), mSource.begin() + static_cast<std::ptrdiff_t>(offset), '\n'));
    }

private:
    const std::string& mSource;
    LocalizationLoadResult& mResult;
    const std::string& mSourceName;
    std::size_t mFirstLine = 0;
};

class RichStringParser {
public:
    RichStringParser(const std::vector<RichToken>& tokens, const std::string& source, StringTemplate& parsed, LocalizationLoadResult& result,
                     const std::string& source_name, std::size_t first_line, std::size_t end_line)
        : mTokens(tokens), mSource(source), mParsed(parsed), mResult(result), mSourceName(source_name), mFirstLine(first_line), mEndLine(end_line) {}

    bool parse() { return parseNodes(mParsed.nodes, nullptr); }

private:
    bool parseNodes(std::vector<TemplateNode>& nodes, const ContainerTag* expected_close) {
        while (mIndex < mTokens.size()) {
            const RichToken& token = mTokens[mIndex];
            if (token.kind == RichTokenKind::ContainerClose) {
                if (token.container == expected_close) {
                    ++mIndex;
                    return true;
                }
                mResult.error("localization.string.tag_invalid", "Unknown, malformed, or misplaced Rich String tag: " + token.value + ".",
                              mSourceName, tokenLine(token));
                return false;
            }

            ++mIndex;
            switch (token.kind) {
                case RichTokenKind::Text: appendText(nodes, token.value); break;
                case RichTokenKind::Argument:
                    mParsed.arguments.insert(token.value);
                    nodes.push_back({TemplateKind::Argument, token.value, {}});
                    break;
                case RichTokenKind::ContainerOpen: {
                    TemplateNode node{token.container->kind, {}, {}};
                    if (!parseNodes(node.children, token.container)) return false;
                    nodes.push_back(std::move(node));
                    break;
                }
                case RichTokenKind::Kbd:
                    mParsed.bindings.insert(token.value);
                    nodes.push_back({TemplateKind::Kbd, token.value, {}});
                    break;
                case RichTokenKind::Br: nodes.push_back({TemplateKind::Br, {}, {}}); break;
                case RichTokenKind::ContainerClose: break;
            }
        }

        if (!expected_close) return true;
        mResult.error("localization.string.tag_unclosed", "Rich String is missing closing tag " + std::string(expected_close->close) + ".",
                      mSourceName, mEndLine);
        return false;
    }

    static void appendText(std::vector<TemplateNode>& nodes, const std::string& text) {
        if (text.empty()) return;
        if (!nodes.empty() && nodes.back().kind == TemplateKind::Text) nodes.back().value += text;
        else nodes.push_back({TemplateKind::Text, text, {}});
    }

    std::size_t tokenLine(const RichToken& token) const {
        return mFirstLine + static_cast<std::size_t>(std::count(mSource.begin(), mSource.begin() + static_cast<std::ptrdiff_t>(token.offset), '\n'));
    }

    const std::vector<RichToken>& mTokens;
    const std::string& mSource;
    StringTemplate& mParsed;
    LocalizationLoadResult& mResult;
    const std::string& mSourceName;
    std::size_t mFirstLine = 0;
    std::size_t mEndLine = 0;
    std::size_t mIndex = 0;
};
} // namespace

bool parseRichString(const std::string& source, StringTemplate& parsed, LocalizationLoadResult& result, const std::string& source_name,
                     std::size_t line) {
    parsed = {};
    std::vector<RichToken> tokens;
    RichStringTokenizer tokenizer(source, result, source_name, line);
    if (!tokenizer.tokenize(tokens)) return false;
    const std::size_t end_line = tokenizer.tokenLine(source.size());
    return RichStringParser(tokens, source, parsed, result, source_name, line, end_line).parse();
}
} // namespace rdui::localization_detail
