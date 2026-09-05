/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/elementnames.h"

namespace radia::ui {
namespace {
struct TagInfo {
    HTMLTag tag;
    std::string_view localName;
    bool isVoid;
};

constexpr TagInfo kTagInfo[] = {
    {HTMLTag::Abbr, kAbbrTag.localName, false},
    {HTMLTag::B, kBTag.localName, false},
    {HTMLTag::Button, kButtonTag.localName, false},
    {HTMLTag::Br, kBrTag.localName, true},
    {HTMLTag::Cite, kCiteTag.localName, false},
    {HTMLTag::Code, kCodeTag.localName, false},
    {HTMLTag::Dfn, kDfnTag.localName, false},
    {HTMLTag::Del, kDelTag.localName, false},
    {HTMLTag::Div, kDivTag.localName, false},
    {HTMLTag::Em, kEmTag.localName, false},
    {HTMLTag::Fieldset, kFieldsetTag.localName, false},
    {HTMLTag::Floater, kFloaterTag.localName, false},
    {HTMLTag::Head, kHeadTag.localName, false},
    {HTMLTag::Header, kHeaderTag.localName, false},
    {HTMLTag::I, kITag.localName, false},
    {HTMLTag::Icon, kIconTag.localName, false},
    {HTMLTag::Ins, kInsTag.localName, false},
    {HTMLTag::Kbd, kKbdTag.localName, false},
    {HTMLTag::Label, kLabelTag.localName, false},
    {HTMLTag::Legend, kLegendTag.localName, false},
    {HTMLTag::Link, kLinkTag.localName, false},
    {HTMLTag::Mark, kMarkTag.localName, false},
    {HTMLTag::Minimize, kMinimizeTag.localName, false},
    {HTMLTag::Close, kCloseTag.localName, false},
    {HTMLTag::Panel, kPanelTag.localName, false},
    {HTMLTag::Paragraph, kParagraphTag.localName, false},
    {HTMLTag::Q, kQTag.localName, false},
    {HTMLTag::S, kSTag.localName, false},
    {HTMLTag::Small, kSmallTag.localName, false},
    {HTMLTag::Strong, kStrongTag.localName, false},
    {HTMLTag::Title, kTitleTag.localName, false},
    {HTMLTag::U, kUTag.localName, false},
    {HTMLTag::Input, kInputTag.localName, true},
    {HTMLTag::Body, kBodyTag.localName, false},
};

} // namespace

bool isHTMLNameCharacter(char character) {
    return (character >= 'a' && character <= 'z')
        || (character >= 'A' && character <= 'Z')
        || (character >= '0' && character <= '9')
        || character == '-'
        || character == '_'
        || character == ':';
}

bool isHTMLWhitespace(char character) {
    return character == '\t' || character == '\n' || character == '\f' || character == '\r' || character == ' ';
}

bool containsHTMLWhitespace(std::string_view value) {
    for (const char character : value)
        if (isHTMLWhitespace(character)) return true;
    return false;
}

std::string canonicalizeHTMLName(std::string_view name) {
    std::string result;
    result.reserve(name.size());
    for (const char character : name) result.push_back(character >= 'A' && character <= 'Z' ? static_cast<char>(character + ('a' - 'A')) : character);
    return result;
}

std::string_view htmlTagName(HTMLTag tag) {
    for (const TagInfo& info : kTagInfo)
        if (info.tag == tag) return info.localName;
    return {};
}

HTMLTag lookupHTMLTag(std::string_view name) {
    const std::string canonical = canonicalizeHTMLName(name);
    for (const TagInfo& info : kTagInfo)
        if (canonical == info.localName) return info.tag;
    return HTMLTag::Unknown;
}

bool isVoidHTMLTag(HTMLTag tag) {
    for (const TagInfo& info : kTagInfo)
        if (info.tag == tag) return info.isVoid;
    return false;
}
} // namespace radia::ui
