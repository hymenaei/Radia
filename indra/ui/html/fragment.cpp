/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "dom/fragment.h"
#include <algorithm>
#include <string>
#include <string_view>
#include <utility>
#include <vector>
#include "dom/element.h"
#include "dom/elementinternal.h"
#include "dom/text.h"
#include "html/element.h"
#include "html/elementfactory.h"
#include "html/elementnames.h"
#include "html/floater.h"
#include "html/fragmentinternal.h"
#include "html/icon.h"
#include "html/input.h"
#include "html/label.h"
#include "llstring.h"

namespace radia::ui::html_detail {
using detail::appendText;
using detail::HTMLElementFactory;
using detail::NodeAccess;

namespace {
struct Attribute {
    std::string name;
    std::string value;
    bool hasValue = false;
};

Node* appendFragmentText(Fragment& parent, std::string value) {
    if (value.empty()) return nullptr;
    if (Node* last = parent.lastChild()) {
        if (Text* previous = last->asText(); previous && !NodeAccess::flowBreakBefore(*previous)) {
            previous->setData(previous->data() + value);
            return previous;
        }
    }
    return parent.append(std::make_unique<Text>(std::move(value)));
}

bool hasLayoutText(std::string_view value) {
    for (const char character : value)
        if (!isHTMLWhitespace(character)) return true;
    return false;
}

bool isLineBreakElement(const Element& element) {
    return element.elementName() == kBrTag.localName;
}

bool isVoidElement(const Element& element) {
    return isVoidHTMLTag(lookupHTMLTag(element.elementName()));
}
} // namespace

bool isValidHTMLAttribute(HTMLTag tag, std::string_view name, bool hasValue, std::string_view value) {
    if (name == "id") return hasValue && !value.empty() && !containsHTMLWhitespace(value);
    if (name == "class") return true;
    if (name == "disabled" || name == "hidden") return true;
    if (name == "visibility") return hasValue && (value == "visible" || value == "hidden" || value == "collapse");
    if (name == "shortcut") return tag == HTMLTag::Kbd && hasValue && !value.empty();
    if (tag == HTMLTag::Input) {
        if (name == "type" || name == "name") return hasValue;
        if (name == "switch" || name == "checked") return true;
    }
    if (tag == HTMLTag::Icon && name == "src") return hasValue;
    if (tag == HTMLTag::Label && name == "for") return hasValue && !value.empty() && !containsHTMLWhitespace(value);
    if (tag == HTMLTag::Floater && name == "resizeable") return true;
    return false;
}

class FragmentSerializer final {
public:
    static void serializeNode(const Node& node, std::string& result) {
        if (const Text* text = node.asText()) {
            result += LLStringFn::xml_encode(text->data());
            return;
        }
        if (const Fragment* fragment = node.asFragment()) {
            for (const Node* child : fragment->childNodes()) serializeNode(*child, result);
            return;
        }
        const Element* element = node.asElement();
        if (!element) return;

        result += '<';
        result += element->elementName();
        for (const Element::Attribute& attribute : element->attributes()) {
            result += ' ';
            result += attribute.name;
            if (attribute.value) result += "=\"" + LLStringFn::xml_encode(*attribute.value, true) + "\"";
        }

        result += '>';
        if (isVoidElement(*element)) return;

        for (const Node* child : element->childNodes()) serializeNode(*child, result);
        result += "</";
        result += element->elementName();
        result += '>';
    }
};

class FragmentParser final {
public:
    explicit FragmentParser(std::string_view html) : mHTML(html) {}

    FragmentPtr parse() {
        auto result = std::make_unique<Fragment>();
        bool pendingFlowBreak = false;
        while (mOffset < mHTML.size()) {
            if (mHTML.compare(mOffset, 4, "<!--") == 0) {
                if (!skipComment()) return nullptr;
                continue;
            }
            if (mHTML[mOffset] == '<') {
                if (mHTML.compare(mOffset, 2, "</") == 0) return nullptr;
                NodePtr child = parseElement();
                if (!child) return nullptr;
                Node* added = child.get();
                result->append(std::move(child));
                if (added->asElement() && isLineBreakElement(*added->asElement())) pendingFlowBreak = true;
                else if (pendingFlowBreak) {
                    NodeAccess::setFlowBreakBefore(*added, true);
                    pendingFlowBreak = false;
                }
                continue;
            }
            const std::string text = parseText();
            Node* added = appendFragmentText(*result, text);
            if (added && hasLayoutText(text) && pendingFlowBreak) {
                NodeAccess::setFlowBreakBefore(*added, true);
                pendingFlowBreak = false;
            }
        }
        return result;
    }

private:
    static bool applyAttribute(Element& element, const Attribute& attribute) {
        const std::optional<std::string> value = attribute.hasValue ? std::optional<std::string>(attribute.value) : std::nullopt;
        if (!isValidHTMLAttribute(lookupHTMLTag(element.elementName()), attribute.name, attribute.hasValue, attribute.value)) return false;

        if (attribute.name == "id" || attribute.name == "class" || attribute.name == "disabled" || attribute.name == "hidden") {
            element.setAttribute(attribute.name, value);
            return true;
        }
        if (attribute.name == "visibility") {
            element.setAttribute(attribute.name, value);
            return true;
        }
        if (attribute.name == "shortcut") {
            auto* htmlElement = dynamic_cast<HTMLElement*>(&element);
            if (!htmlElement || element.elementName() != kKbdTag.localName) return false;
            htmlElement->setKeybinding(attribute.value);
            return true;
        }
        if (auto* input = dynamic_cast<HTMLInputElement*>(&element)) {
            if (attribute.name == "type") {
                input->type(attribute.value);
                return true;
            }
            if (attribute.name == "name") {
                input->name(attribute.value);
                return true;
            }
            if (attribute.name == "switch") {
                input->switchMode(true);
                return true;
            }
            if (attribute.name == "checked") {
                input->checked(true);
                return true;
            }
        }
        if (auto* icon = dynamic_cast<HTMLIconElement*>(&element); icon && attribute.name == "src") {
            icon->setName(attribute.value);
            return true;
        }
        if (auto* label = dynamic_cast<HTMLLabelElement*>(&element); label && attribute.name == "for") {
            label->setTargetId(attribute.value);
            return true;
        }
        if (auto* floater = dynamic_cast<HTMLFloaterElement*>(&element); floater && attribute.name == "resizeable") {
            floater->setResizeable(true);
            return true;
        }
        return false;
    }

    std::string parseText() {
        const std::size_t end = mHTML.find('<', mOffset);
        const std::size_t textEnd = end == std::string_view::npos ? mHTML.size() : end;
        std::string result = LLStringFn::xml_decode(std::string(mHTML.substr(mOffset, textEnd - mOffset)));
        mOffset = textEnd;
        return result;
    }

    bool skipComment() {
        const std::size_t end = mHTML.find("-->", mOffset + 4);
        if (end == std::string_view::npos) return false;
        mOffset = end + 3;
        return true;
    }

    bool readName(std::string& name) {
        const std::size_t begin = mOffset;
        while (mOffset < mHTML.size() && isHTMLNameCharacter(mHTML[mOffset])) ++mOffset;
        if (mOffset == begin) return false;
        name = canonicalizeHTMLName(mHTML.substr(begin, mOffset - begin));
        return true;
    }

    void skipWhitespace() {
        while (mOffset < mHTML.size() && isHTMLWhitespace(mHTML[mOffset])) ++mOffset;
    }

    bool readAttribute(Attribute& attribute) {
        std::string name;
        if (!readName(name)) return false;
        attribute.name = std::move(name);
        attribute.value.clear();
        attribute.hasValue = false;
        skipWhitespace();
        if (mOffset >= mHTML.size() || mHTML[mOffset] != '=') return true;
        ++mOffset;
        attribute.hasValue = true;
        skipWhitespace();
        if (mOffset >= mHTML.size()) return false;

        if (mHTML[mOffset] == '\'' || mHTML[mOffset] == '"') {
            const char quote = mHTML[mOffset++];
            const std::size_t begin = mOffset;
            while (mOffset < mHTML.size() && mHTML[mOffset] != quote) ++mOffset;
            if (mOffset == mHTML.size()) return false;
            attribute.value = std::string(mHTML.substr(begin, mOffset - begin));
            ++mOffset;
        } else {
            const std::size_t begin = mOffset;
            while (mOffset < mHTML.size() && !isHTMLWhitespace(mHTML[mOffset]) && mHTML[mOffset] != '>') ++mOffset;
            attribute.value = std::string(mHTML.substr(begin, mOffset - begin));
        }
        attribute.value = LLStringFn::xml_decode(attribute.value, true);
        return true;
    }

    NodePtr parseElement() {
        ++mOffset;
        std::string name;
        if (!readName(name)) return nullptr;
        const HTMLTag tag = lookupHTMLTag(name);
        ElementPtr element = HTMLElementFactory::Create(name);
        if (!element) return nullptr;

        std::vector<Attribute> attributes;
        bool selfClosing = false;
        bool closed = false;
        while (mOffset < mHTML.size()) {
            if (mHTML[mOffset] == '>') {
                ++mOffset;
                closed = true;
                break;
            }
            if (mHTML[mOffset] == '/' && mOffset + 1 < mHTML.size() && mHTML[mOffset + 1] == '>') {
                mOffset += 2;
                selfClosing = true;
                closed = true;
                break;
            }
            if (isHTMLWhitespace(mHTML[mOffset])) {
                ++mOffset;
                continue;
            }
            Attribute attribute;
            if (!readAttribute(attribute)) return nullptr;
            const bool duplicate = std::any_of(attributes.begin(), attributes.end(),
                                               [&attribute](const Attribute& existing) { return existing.name == attribute.name; });
            if (duplicate) return nullptr;
            attributes.push_back(std::move(attribute));
        }
        if (!closed) return nullptr;
        if (selfClosing && !isVoidHTMLTag(tag)) return nullptr;
        for (const Attribute& attribute : attributes)
            if (attribute.name == "type" && !applyAttribute(*element, attribute)) return nullptr;
        for (const Attribute& attribute : attributes)
            if (attribute.name != "type" && !applyAttribute(*element, attribute)) return nullptr;
        if (isVoidHTMLTag(tag)) return NodePtr(std::move(element));

        bool pendingFlowBreak = false;
        while (mOffset < mHTML.size()) {
            if (mHTML.compare(mOffset, 4, "<!--") == 0) {
                if (!skipComment()) return nullptr;
                continue;
            }
            if (mHTML.compare(mOffset, 2, "</") == 0) {
                mOffset += 2;
                std::string closingName;
                if (!readName(closingName)) return nullptr;
                skipWhitespace();
                if (mOffset >= mHTML.size() || mHTML[mOffset++] != '>' || closingName != name) return nullptr;
                return NodePtr(std::move(element));
            }
            if (mHTML[mOffset] == '<') {
                NodePtr child = parseElement();
                if (!child) return nullptr;
                Node* added = child.get();
                element->append(std::move(child));
                if (added->asElement() && isLineBreakElement(*added->asElement())) pendingFlowBreak = true;
                else if (pendingFlowBreak) {
                    NodeAccess::setFlowBreakBefore(*added, true);
                    pendingFlowBreak = false;
                }
            } else {
                const std::string text = parseText();
                Node& added = appendText(*element, text);
                if (hasLayoutText(text) && pendingFlowBreak) {
                    NodeAccess::setFlowBreakBefore(added, true);
                    pendingFlowBreak = false;
                }
            }
        }
        return nullptr;
    }

    std::string_view mHTML;
    std::size_t mOffset = 0;
};

FragmentPtr parseFragment(std::string_view html) {
    return FragmentParser(html).parse();
}

std::string serializeChildren(const Node& parent) {
    std::string result;
    for (const Node* child : parent.childNodes()) FragmentSerializer::serializeNode(*child, result);
    return result;
}
} // namespace radia::ui::html_detail
