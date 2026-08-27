/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "layout/document.h"
#include <algorithm>
#include <cctype>
#include <string_view>
#include "llstring.h"
#include "layout/schema.h"

namespace radia::ui {
namespace {
struct TagInfo {
    Tag tag;
    const char* name;
    std::uint8_t flags;
};

constexpr std::uint8_t kGenericElement = 1 << 0;
constexpr std::uint8_t kLocalizedInline = 1 << 1;

constexpr TagInfo kTagInfo[] = {
    {Tag::Abbr, "abbr", kGenericElement | kLocalizedInline},
    {Tag::B, "b", kGenericElement | kLocalizedInline},
    {Tag::Button, "button", 0},
    {Tag::Br, "br", kGenericElement | kLocalizedInline},
    {Tag::Cite, "cite", kGenericElement | kLocalizedInline},
    {Tag::Code, "code", kGenericElement | kLocalizedInline},
    {Tag::Dfn, "dfn", kGenericElement | kLocalizedInline},
    {Tag::Del, "del", kGenericElement | kLocalizedInline},
    {Tag::Div, "div", kGenericElement},
    {Tag::Em, "em", kGenericElement | kLocalizedInline},
    {Tag::Fieldset, "fieldset", 0},
    {Tag::Floater, "floater", 0},
    {Tag::Head, "head", kGenericElement},
    {Tag::Header, "header", kGenericElement},
    {Tag::I, "i", kGenericElement | kLocalizedInline},
    {Tag::Icon, "icon", 0},
    {Tag::Ins, "ins", kGenericElement | kLocalizedInline},
    {Tag::Kbd, "kbd", kGenericElement | kLocalizedInline},
    {Tag::Label, "label", 0},
    {Tag::Legend, "legend", 0},
    {Tag::Link, "link", kGenericElement},
    {Tag::Mark, "mark", kGenericElement | kLocalizedInline},
    {Tag::Minimize, "minimize", 0},
    {Tag::Close, "close", 0},
    {Tag::Panel, "panel", 0},
    {Tag::Paragraph, "p", kGenericElement},
    {Tag::Q, "q", kGenericElement | kLocalizedInline},
    {Tag::S, "s", kGenericElement | kLocalizedInline},
    {Tag::Small, "small", kGenericElement | kLocalizedInline},
    {Tag::Strong, "strong", kGenericElement | kLocalizedInline},
    {Tag::Title, "title", kGenericElement},
    {Tag::U, "u", kGenericElement | kLocalizedInline},
    {Tag::Input, "input", 0},
    {Tag::Body, "body", kGenericElement},
};

bool hasFlag(Tag tag, std::uint8_t flag) {
    for (const TagInfo& info : kTagInfo)
        if (info.tag == tag) return (info.flags & flag) != 0;
    return false;
}

const std::vector<Tag>& tagsWithFlag(std::uint8_t flag) {
    static const std::vector<Tag> generic = [] {
        std::vector<Tag> result;
        for (const TagInfo& info : kTagInfo)
            if ((info.flags & kGenericElement) != 0) result.push_back(info.tag);
        return result;
    }();
    static const std::vector<Tag> localizedInline = [] {
        std::vector<Tag> result;
        for (const TagInfo& info : kTagInfo)
            if ((info.flags & kLocalizedInline) != 0) result.push_back(info.tag);
        return result;
    }();
    return flag == kGenericElement ? generic : localizedInline;
}

bool isMarkupNameCharacter(char character) {
    return std::isalnum(static_cast<unsigned char>(character)) || character == '-' || character == '_' || character == ':';
}

bool isVoidElementName(std::string_view name) {
    const std::string key = schemaNameKey(name);
    return key == "br" || key == "input";
}

bool isMarkupWhitespace(char character) {
    return std::isspace(static_cast<unsigned char>(character)) != 0;
}

class HtmlParser final {
public:
    HtmlParser(std::string_view markup, std::string source, DiagnosticResult& result)
        : mMarkup(markup), mResult(result), mDocument(std::make_unique<SourceDocument>()) {
        mDocument->source = std::move(source);
    }

    std::unique_ptr<SourceDocument> parse() {
        while (mOffset < mMarkup.size()) {
            if (!(mMarkup[mOffset] == '<' ? parseTag() : parseText())) return nullptr;
        }
        if (!mNodeStack.empty()) {
            fail("Unclosed HTML element: <" + mNodeStack.back()->authoredName + ">.", mOffset);
            return nullptr;
        }
        if (!mDocument->root) {
            fail("Radia UI HTML must contain one root element.", mOffset);
            return nullptr;
        }
        return std::move(mDocument);
    }

private:
    bool parseTag() {
        if (mMarkup.compare(mOffset, 4, "<!--") == 0) return parseComment();
        if (mMarkup.compare(mOffset, 2, "</") == 0) return parseEndTag();
        return parseStartTag();
    }

    bool parseComment() {
        const std::size_t end = mMarkup.find("-->", mOffset + 4);
        if (end == std::string_view::npos) return fail("Unterminated HTML comment.", mOffset);
        consumeTo(end + 3);
        return true;
    }

    bool parseText() {
        const std::size_t begin = mOffset;
        const std::size_t end = mMarkup.find('<', begin);
        const std::size_t textEnd = end == std::string_view::npos ? mMarkup.size() : end;
        if (mNodeStack.empty()) {
            std::size_t firstContent = begin;
            while (firstContent < textEnd && isMarkupWhitespace(mMarkup[firstContent])) ++firstContent;
            if (firstContent < textEnd) return fail("Text is not allowed outside the root element.", firstContent);
            consumeTo(textEnd);
            return true;
        }

        if (textEnd > begin) {
            SourceContent content;
            content.source.begin = mLocation;
            content.source.end = locationAt(textEnd);
            content.text = LLStringFn::xml_decode(std::string(mMarkup.substr(begin, textEnd - begin)), true);
            SourceContent* previous = mNodeStack.back()->content.empty() ? nullptr : &mNodeStack.back()->content.back();
            if (previous && previous->isText()) {
                previous->text += content.text;
                previous->source.end = content.source.end;
            } else {
                mNodeStack.back()->content.push_back(std::move(content));
            }
        }
        consumeTo(textEnd);
        return true;
    }

    bool parseStartTag() {
        const std::size_t tagStart = mOffset;
        const SourceLocation tagLocation = mLocation;
        std::size_t cursor = tagStart + 1;
        const std::size_t nameBegin = cursor;
        while (cursor < mMarkup.size() && isMarkupNameCharacter(mMarkup[cursor])) ++cursor;
        if (cursor == nameBegin) return fail("Expected an HTML element name.", tagStart);

        const std::string authoredName(mMarkup.substr(nameBegin, cursor - nameBegin));
        if (mNodeStack.empty() && (mDocument->root || mRootClosed))
            return fail("Radia UI HTML must contain exactly one root element.", tagStart);

        auto node = std::make_unique<SourceNode>();
        node->tag = sourceTagFromName(authoredName);
        node->authoredName = authoredName;
        node->source.begin = tagLocation;
        if (node->tag == Tag::Unknown)
            mResult.error("layout.element.unknown", "Unsupported HTML element: " + node->authoredName + ".", mDocument->source,
                          tagLocation.line, tagLocation.column);

        bool selfClosing = false;
        bool closed = false;
        while (cursor < mMarkup.size()) {
            if (mMarkup[cursor] == '>') {
                ++cursor;
                closed = true;
                break;
            }
            if (mMarkup[cursor] == '/' && cursor + 1 < mMarkup.size() && mMarkup[cursor + 1] == '>') {
                cursor += 2;
                selfClosing = true;
                closed = true;
                break;
            }
            if (isMarkupWhitespace(mMarkup[cursor])) {
                ++cursor;
                continue;
            }

            const std::size_t attributeBegin = cursor;
            while (cursor < mMarkup.size() && isMarkupNameCharacter(mMarkup[cursor])) ++cursor;
            if (cursor == attributeBegin) return fail("Expected an HTML attribute or element close.", cursor);
            const std::string authoredAttribute(mMarkup.substr(attributeBegin, cursor - attributeBegin));

            while (cursor < mMarkup.size() && isMarkupWhitespace(mMarkup[cursor])) ++cursor;
            std::string value;
            if (cursor < mMarkup.size() && mMarkup[cursor] == '=') {
                ++cursor;
                while (cursor < mMarkup.size() && isMarkupWhitespace(mMarkup[cursor])) ++cursor;
                if (cursor >= mMarkup.size()) return fail("Unterminated HTML attribute value.", attributeBegin);

                if (mMarkup[cursor] == '\'' || mMarkup[cursor] == '"') {
                    const char quote = mMarkup[cursor++];
                    const std::size_t valueBegin = cursor;
                    while (cursor < mMarkup.size() && mMarkup[cursor] != quote) ++cursor;
                    if (cursor >= mMarkup.size()) return fail("Unterminated HTML attribute value.", valueBegin);
                    value = std::string(mMarkup.substr(valueBegin, cursor - valueBegin));
                    ++cursor;
                } else {
                    const std::size_t valueBegin = cursor;
                    while (cursor < mMarkup.size()
                           && !isMarkupWhitespace(mMarkup[cursor])
                           && mMarkup[cursor] != '>'
                           && !(mMarkup[cursor] == '/' && cursor + 1 < mMarkup.size() && mMarkup[cursor + 1] == '>'))
                        ++cursor;
                    value = std::string(mMarkup.substr(valueBegin, cursor - valueBegin));
                }
                value = LLStringFn::xml_decode(value, true);
            }

            SourceAttribute attribute;
            attribute.authoredName = authoredAttribute;
            attribute.value = std::move(value);
            attribute.source.begin = tagLocation;
            attribute.source.end = tagLocation;
            const std::string key = schemaNameKey(attribute.authoredName);
            if (!node->attributes.emplace(key, std::move(attribute)).second)
                mResult.error("layout.attribute.duplicate", "Attributes differing only by ASCII case are duplicate declarations.",
                              mDocument->source, tagLocation.line, tagLocation.column);
        }

        if (!closed) return fail("Unterminated HTML element.", tagStart);

        const SourceLocation tagEnd = locationAt(cursor);
        node->source.end = tagEnd;
        SourceNode* next = node.get();
        if (mNodeStack.empty()) mDocument->root = std::move(node);
        else {
            SourceContent content;
            content.source.begin = tagLocation;
            content.source.end = tagEnd;
            content.node = std::move(node);
            mNodeStack.back()->content.push_back(std::move(content));
        }

        if (!selfClosing && !isVoidElementName(authoredName)) mNodeStack.push_back(next);
        else if (mNodeStack.empty()) mRootClosed = true;
        consumeTo(cursor);
        return true;
    }

    bool parseEndTag() {
        const std::size_t tagStart = mOffset;
        std::size_t cursor = tagStart + 2;
        const std::size_t nameBegin = cursor;
        while (cursor < mMarkup.size() && isMarkupNameCharacter(mMarkup[cursor])) ++cursor;
        if (cursor == nameBegin) return fail("Expected an HTML closing element name.", tagStart);
        const std::string closingName(mMarkup.substr(nameBegin, cursor - nameBegin));
        while (cursor < mMarkup.size() && isMarkupWhitespace(mMarkup[cursor])) ++cursor;
        if (cursor >= mMarkup.size() || mMarkup[cursor] != '>') return fail("Unterminated HTML closing element.", tagStart);
        ++cursor;

        if (mNodeStack.empty()) return fail("Unexpected HTML closing element: </" + closingName + ">.", tagStart);
        SourceNode* node = mNodeStack.back();
        if (schemaNameKey(node->authoredName) != schemaNameKey(closingName))
            return fail("HTML closing element does not match <" + node->authoredName + ">.", tagStart);

        node->source.end = locationAt(cursor);
        mNodeStack.pop_back();
        if (!mNodeStack.empty() && !mNodeStack.back()->content.empty()) mNodeStack.back()->content.back().source.end = node->source.end;
        if (mNodeStack.empty()) mRootClosed = true;
        consumeTo(cursor);
        return true;
    }

    SourceLocation locationAt(std::size_t offset) const {
        offset = std::min(offset, mMarkup.size());
        SourceLocation result = mLocation;
        for (std::size_t index = mOffset; index < offset; ++index) {
            ++result.offset;
            if (mMarkup[index] == '\n') {
                ++result.line;
                result.column = 1;
            } else ++result.column;
        }
        return result;
    }

    void consumeTo(std::size_t offset) {
        mLocation = locationAt(offset);
        mOffset = offset;
    }

    bool fail(std::string message, std::size_t offset) {
        const SourceLocation location = locationAt(offset);
        mResult.error("layout.html.invalid", std::move(message), mDocument->source, location.line, location.column);
        return false;
    }

    std::string_view mMarkup;
    DiagnosticResult& mResult;
    std::unique_ptr<SourceDocument> mDocument;
    std::vector<SourceNode*> mNodeStack;
    std::size_t mOffset = 0;
    SourceLocation mLocation{1, 1, 0};
    bool mRootClosed = false;
};
} // namespace

const char* sourceTagName(Tag tag) {
    for (const TagInfo& info : kTagInfo)
        if (info.tag == tag) return info.name;
    return "";
}

Tag sourceTagFromName(std::string_view name) {
    const std::string key = schemaNameKey(name);
    for (const TagInfo& info : kTagInfo)
        if (key == info.name) return info.tag;
    return Tag::Unknown;
}

bool isGenericElementTag(Tag tag) {
    return hasFlag(tag, kGenericElement);
}

bool isLocalizedInlineTag(Tag tag) {
    return hasFlag(tag, kLocalizedInline);
}

const std::vector<Tag>& genericElementTags() {
    return tagsWithFlag(kGenericElement);
}

const std::vector<Tag>& localizedInlineTags() {
    return tagsWithFlag(kLocalizedInline);
}

SourceDocumentParseResult SourceDocumentParser::parse(const std::string& html, const std::string& source) const {
    SourceDocumentParseResult result;
    result.document = HtmlParser(html, source, result).parse();
    return result;
}
} // namespace radia::ui
