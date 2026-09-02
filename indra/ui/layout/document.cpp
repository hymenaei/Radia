/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "layout/document.h"
#include <algorithm>
#include <string_view>
#include "html/elementnames.h"
#include "llstring.h"

namespace radia::ui {
namespace {
struct MutableSourceNode;

struct MutableSourceContent {
    SourceRange source;
    std::string text;
    std::unique_ptr<MutableSourceNode> node;

    bool isText() const { return !node; }
};

struct MutableSourceNode {
    HTMLTag tag = HTMLTag::Unknown;
    std::string authoredName;
    SourceRange source;
    SourceAttributeMap attributes;
    std::vector<MutableSourceContent> content;
};

struct MutableSourceDocument {
    std::string sourceName;
    std::unique_ptr<MutableSourceNode> root;
};

std::unique_ptr<const SourceNode> freezeNode(std::unique_ptr<MutableSourceNode> node) {
    if (!node) return nullptr;
    auto result = std::make_unique<SourceNode>();
    result->tag = node->tag;
    result->authoredName = std::move(node->authoredName);
    result->source = node->source;
    result->attributes = std::move(node->attributes);
    result->content.reserve(node->content.size());
    for (MutableSourceContent& content : node->content) {
        SourceContent frozen;
        frozen.source = content.source;
        frozen.text = std::move(content.text);
        frozen.node = freezeNode(std::move(content.node));
        result->content.push_back(std::move(frozen));
    }
    return result;
}

std::unique_ptr<SourceDocument> freezeDocument(std::unique_ptr<MutableSourceDocument> document) {
    if (!document) return nullptr;
    auto result = std::make_unique<SourceDocument>();
    result->sourceName = std::move(document->sourceName);
    result->root = freezeNode(std::move(document->root));
    return result;
}

class HTMLParser final {
public:
    HTMLParser(std::string_view html, std::string sourceName, DiagnosticResult& result)
        : mHTML(html), mResult(result), mDocument(std::make_unique<MutableSourceDocument>()) {
        mDocument->sourceName = std::move(sourceName);
    }

    std::unique_ptr<SourceDocument> parse() {
        while (mOffset < mHTML.size())
            if (!(mHTML[mOffset] == '<' ? parseTag() : parseText())) return nullptr;
        if (!mNodeStack.empty()) {
            fail("Unclosed HTML element: <" + mNodeStack.back()->authoredName + ">.", mOffset);
            return nullptr;
        }
        if (!mDocument->root) {
            fail("Radia UI HTML must contain one root element.", mOffset);
            return nullptr;
        }
        return freezeDocument(std::move(mDocument));
    }

private:
    bool parseTag() {
        if (mHTML.compare(mOffset, 4, "<!--") == 0) return parseComment();
        if (mHTML.compare(mOffset, 2, "</") == 0) return parseEndTag();
        return parseStartTag();
    }

    bool parseComment() {
        const std::size_t end = mHTML.find("-->", mOffset + 4);
        if (end == std::string_view::npos) return fail("Unterminated HTML comment.", mOffset);
        consumeTo(end + 3);
        return true;
    }

    bool parseText() {
        const std::size_t begin = mOffset;
        const std::size_t end = mHTML.find('<', begin);
        const std::size_t textEnd = end == std::string_view::npos ? mHTML.size() : end;
        if (mNodeStack.empty()) {
            std::size_t firstContent = begin;
            while (firstContent < textEnd && isHTMLWhitespace(mHTML[firstContent])) ++firstContent;
            if (firstContent < textEnd) return fail("Text is not allowed outside the root element.", firstContent);
            consumeTo(textEnd);
            return true;
        }

        if (textEnd > begin) {
            MutableSourceContent content;
            content.source.begin = mLocation;
            content.source.end = locationAt(textEnd);
            content.text = LLStringFn::xml_decode(std::string(mHTML.substr(begin, textEnd - begin)), true);
            MutableSourceContent* previous = mNodeStack.back()->content.empty() ? nullptr : &mNodeStack.back()->content.back();
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
        while (cursor < mHTML.size() && isHTMLNameCharacter(mHTML[cursor])) ++cursor;
        if (cursor == nameBegin) return fail("Expected an HTML element name.", tagStart);

        const std::string authoredName(mHTML.substr(nameBegin, cursor - nameBegin));
        if (mNodeStack.empty() && (mDocument->root || mRootClosed)) return fail("Radia UI HTML must contain exactly one root element.", tagStart);

        auto node = std::make_unique<MutableSourceNode>();
        node->tag = lookupHTMLTag(authoredName);
        node->authoredName = authoredName;
        node->source.begin = tagLocation;
        if (node->tag == HTMLTag::Unknown)
            mResult.error("layout.element.unknown", "Unsupported HTML element: " + node->authoredName + ".", mDocument->sourceName, tagLocation.line,
                          tagLocation.column);

        bool selfClosing = false;
        bool closed = false;
        while (cursor < mHTML.size()) {
            if (mHTML[cursor] == '>') {
                ++cursor;
                closed = true;
                break;
            }
            if (mHTML[cursor] == '/' && cursor + 1 < mHTML.size() && mHTML[cursor + 1] == '>') {
                cursor += 2;
                selfClosing = true;
                closed = true;
                break;
            }
            if (isHTMLWhitespace(mHTML[cursor])) {
                ++cursor;
                continue;
            }

            const std::size_t attributeBegin = cursor;
            while (cursor < mHTML.size() && isHTMLNameCharacter(mHTML[cursor])) ++cursor;
            if (cursor == attributeBegin) return fail("Expected an HTML attribute or element close.", cursor);
            const std::string authoredAttribute(mHTML.substr(attributeBegin, cursor - attributeBegin));

            while (cursor < mHTML.size() && isHTMLWhitespace(mHTML[cursor])) ++cursor;
            std::string value;
            bool hasValue = false;
            if (cursor < mHTML.size() && mHTML[cursor] == '=') {
                ++cursor;
                hasValue = true;
                while (cursor < mHTML.size() && isHTMLWhitespace(mHTML[cursor])) ++cursor;
                if (cursor >= mHTML.size()) return fail("Unterminated HTML attribute value.", attributeBegin);

                if (mHTML[cursor] == '\'' || mHTML[cursor] == '"') {
                    const char quote = mHTML[cursor++];
                    const std::size_t valueBegin = cursor;
                    while (cursor < mHTML.size() && mHTML[cursor] != quote) ++cursor;
                    if (cursor >= mHTML.size()) return fail("Unterminated HTML attribute value.", valueBegin);
                    value = std::string(mHTML.substr(valueBegin, cursor - valueBegin));
                    ++cursor;
                } else {
                    const std::size_t valueBegin = cursor;
                    while (cursor < mHTML.size() && !isHTMLWhitespace(mHTML[cursor]) && mHTML[cursor] != '>') ++cursor;
                    value = std::string(mHTML.substr(valueBegin, cursor - valueBegin));
                }
                value = LLStringFn::xml_decode(value, true);
            }

            SourceAttribute attribute;
            attribute.authoredName = authoredAttribute;
            attribute.value = std::move(value);
            attribute.hasValue = hasValue;
            attribute.source.begin = tagLocation;
            attribute.source.end = tagLocation;
            const std::string key = canonicalizeHTMLName(attribute.authoredName);
            if (!node->attributes.emplace(key, std::move(attribute)).second)
                mResult.error("layout.attribute.duplicate", "Attributes differing only by ASCII case are duplicate declarations.",
                              mDocument->sourceName, tagLocation.line, tagLocation.column);
        }

        if (!closed) return fail("Unterminated HTML element.", tagStart);

        if (selfClosing && !isVoidHTMLTag(node->tag)) return fail("Self-closing syntax is only valid for void HTML elements.", tagStart);

        const SourceLocation tagEnd = locationAt(cursor);
        node->source.end = tagEnd;
        MutableSourceNode* next = node.get();
        if (mNodeStack.empty()) mDocument->root = std::move(node);
        else {
            MutableSourceContent content;
            content.source.begin = tagLocation;
            content.source.end = tagEnd;
            content.node = std::move(node);
            mNodeStack.back()->content.push_back(std::move(content));
        }

        if (!selfClosing && !isVoidHTMLTag(lookupHTMLTag(authoredName))) mNodeStack.push_back(next);
        else if (mNodeStack.empty()) mRootClosed = true;
        consumeTo(cursor);
        return true;
    }

    bool parseEndTag() {
        const std::size_t tagStart = mOffset;
        std::size_t cursor = tagStart + 2;
        const std::size_t nameBegin = cursor;
        while (cursor < mHTML.size() && isHTMLNameCharacter(mHTML[cursor])) ++cursor;
        if (cursor == nameBegin) return fail("Expected an HTML closing element name.", tagStart);
        const std::string closingName(mHTML.substr(nameBegin, cursor - nameBegin));
        while (cursor < mHTML.size() && isHTMLWhitespace(mHTML[cursor])) ++cursor;
        if (cursor >= mHTML.size() || mHTML[cursor] != '>') return fail("Unterminated HTML closing element.", tagStart);
        ++cursor;

        if (mNodeStack.empty()) return fail("Unexpected HTML closing element: </" + closingName + ">.", tagStart);
        MutableSourceNode* node = mNodeStack.back();
        if (canonicalizeHTMLName(node->authoredName) != canonicalizeHTMLName(closingName))
            return fail("HTML closing element does not match <" + node->authoredName + ">.", tagStart);

        node->source.end = locationAt(cursor);
        mNodeStack.pop_back();
        if (!mNodeStack.empty() && !mNodeStack.back()->content.empty()) mNodeStack.back()->content.back().source.end = node->source.end;
        if (mNodeStack.empty()) mRootClosed = true;
        consumeTo(cursor);
        return true;
    }

    SourceLocation locationAt(std::size_t offset) const {
        offset = std::min(offset, mHTML.size());
        SourceLocation result = mLocation;
        for (std::size_t index = mOffset; index < offset; ++index) {
            ++result.offset;
            if (mHTML[index] == '\n') {
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
        mResult.error("layout.html.invalid", std::move(message), mDocument->sourceName, location.line, location.column);
        return false;
    }

    std::string_view mHTML;
    DiagnosticResult& mResult;
    std::unique_ptr<MutableSourceDocument> mDocument;
    std::vector<MutableSourceNode*> mNodeStack;
    std::size_t mOffset = 0;
    SourceLocation mLocation{1, 1, 0};
    bool mRootClosed = false;
};
} // namespace

SourceDocumentParseResult SourceDocumentParser::parse(const std::string& html, const std::string& sourceName) const {
    SourceDocumentParseResult result;
    std::unique_ptr<SourceDocument> document = HTMLParser(html, sourceName, result).parse();
    result.document = std::shared_ptr<const SourceDocument>(std::move(document));
    return result;
}
} // namespace radia::ui
