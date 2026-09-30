/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <string>
#include <string_view>
#include <utility>
#include <vector>
#include "Element.h"
#include "ElementInternal.h"
#include "Fragment.h"
#include "FragmentInternal.h"
#include "HTMLElementFactory.h"
#include "HTMLName.h"
#include "ResourceElementDefinition.h"
#include "Text.h"
#include "llstring.h"

namespace Core::detail {
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
    if (value.empty())
        return nullptr;
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
        if (!isHTMLWhitespace(character))
            return true;
    return false;
}

bool isLineBreakElement(const Element& element) { return element.elementName() == HTMLTagName(HTMLTag::Br); }

bool trackFlowBreak(Node& child, bool& hasLayoutChild, bool& pendingFlowBreak) {
    const Element* element = child.asElement();
    if (element && isLineBreakElement(*element)) {
        if (!hasLayoutChild || pendingFlowBreak)
            return false;
        pendingFlowBreak = true;
        return true;
    }

    const Text* text = child.asText();
    const bool contributesLayout = !text || hasLayoutText(text->data());
    if (contributesLayout) {
        if (pendingFlowBreak) {
            NodeAccess::setFlowBreakBefore(child, true);
            pendingFlowBreak = false;
        }
        hasLayoutChild = true;
    }
    return true;
}

bool isVoidElement(const Element& element) { return isVoidHTMLTag(findHTMLTag(element.elementName())); }

bool isFragmentBooleanAttribute(HTMLTag tag, std::string_view name) {
    if (name == "disabled" || name == "hidden")
        return true;
    return (tag == HTMLTag::Input && (name == "switch" || name == "checked")) || (tag == HTMLTag::Floater && name == "resizable");
}

bool applyFragmentAttributes(Element& element, HTMLTag tag, std::string_view elementName, const std::vector<Attribute>& attributes,
    bool scoped) {
    ResourceBuildResult result;
    ElementBuildContext context(result, nullptr);
    ElementBuildInput input;
    input.tag = tag;
    input.authoredName = elementName;
    input.sourceName = "<fragment>";
    input.attributes.reserve(attributes.size());
    for (const Attribute& attribute : attributes) {
        if (!isRegisteredHTMLAttribute(tag, attribute.name) || attribute.name == "filename")
            return false;
        ElementAttribute value {attribute.name, attribute.value, attribute.hasValue, {}};
        if (isFragmentBooleanAttribute(tag, attribute.name)) {
            value.value = "true";
            value.hasValue = true;
        }
        input.attributes.emplace(attribute.name, std::move(value));
    }

    const ResourceElementDefinition* definition = findElementDefinition(tag);
    if (!definition || (definition->scopedOnly && !scoped))
        return false;
    applyCommonElementAttributes(input, element, context);
    applyElementDefinitionAttributes(*definition, input, element, context);
    return result.warnings.empty() && !result.hasErrors();
}

const ScopedElementDefinition* scopedChildDefinition(const Element& parent, HTMLTag tag) {
    const ResourceElementDefinition* definition = findElementDefinition(findHTMLTag(parent.elementName()));
    if (!definition)
        return nullptr;
    const auto child = definition->contentBehavior.scopedElements.find(canonicalizeHTMLName(HTMLTagName(tag)));
    return child == definition->contentBehavior.scopedElements.end() ? nullptr : &child->second;
}

const std::vector<HTMLTag>* scopedContentTags(std::string_view elementName) {
    const std::string name = canonicalizeHTMLName(elementName);
    for (const auto& descriptor : htmlTags) {
        const ResourceElementDefinition* owner = findElementDefinition(descriptor.tag);
        if (!owner)
            continue;
        const auto scoped = owner->contentBehavior.scopedElements.find(name);
        if (scoped != owner->contentBehavior.scopedElements.end())
            return &scoped->second.acceptedTags;
    }
    return nullptr;
}

const std::vector<HTMLTag>* scopedContentTags(const Element& context) {
    for (const Element* current = &context; current; current = current->parentElement()) {
        const ResourceElementDefinition* definition = findElementDefinition(findHTMLTag(current->elementName()));
        if (definition && definition->scopedOnly)
            return scopedContentTags(current->elementName());
    }
    return nullptr;
}

bool acceptsScopedContent(const std::vector<HTMLTag>* acceptedTags, HTMLTag tag) {
    return !acceptedTags || std::find(acceptedTags->begin(), acceptedTags->end(), tag) != acceptedTags->end();
}

void appendNode(Node& parent, NodePtr child) {
    if (Element* element = parent.asElement())
        element->append(std::move(child));
    else
        parent.asFragment()->append(std::move(child));
}
} // namespace

class FragmentSerializer final {
public:
    static void serializeNode(const Node& node, std::string& result) {
        if (const Text* text = node.asText()) {
            result += LLStringFn::xml_encode(text->data());
            return;
        }
        if (const Fragment* fragment = node.asFragment()) {
            for (const Node* child : fragment->childNodes())
                serializeNode(*child, result);
            return;
        }
        const Element* element = node.asElement();
        if (!element)
            return;

        result += '<';
        result += element->elementName();
        for (const Element::Attribute& attribute : element->attributes()) {
            result += ' ';
            result += attribute.name;
            if (attribute.value)
                result += "=\"" + LLStringFn::xml_encode(*attribute.value, true) + "\"";
        }

        result += '>';
        if (isVoidElement(*element))
            return;

        for (const Node* child : element->childNodes())
            serializeNode(*child, result);
        result += "</";
        result += element->elementName();
        result += '>';
    }
};

class FragmentParser final {
public:
    FragmentParser(std::string_view html, const Element* context)
        : mHTML(html)
        , mContext(context) {}

    FragmentPtr parse() {
        auto result = std::make_unique<Fragment>();
        ElementPtr contextElement;
        Node* parent = result.get();
        const std::vector<HTMLTag>* acceptedTags = mContext ? scopedContentTags(*mContext) : nullptr;
        if (mContext) {
            const ResourceElementDefinition* definition = findElementDefinition(findHTMLTag(mContext->elementName()));
            if (definition && !definition->contentBehavior.scopedElements.empty()) {
                contextElement = HTMLElementFactory::create(mContext->elementName());
                if (!contextElement)
                    return nullptr;
                parent = contextElement.get();
            }
        }
        bool pendingFlowBreak = false;
        bool hasLayoutChild = false;
        while (mOffset < mHTML.size()) {
            if (mHTML.compare(mOffset, 4, "<!--") == 0) {
                if (!skipComment())
                    return nullptr;
                continue;
            }
            if (mHTML[mOffset] == '<') {
                if (mHTML.compare(mOffset, 2, "</") == 0)
                    return nullptr;
                Node* child = parseElement(*parent, acceptedTags);
                if (!child)
                    return nullptr;
                if (!trackFlowBreak(*child, hasLayoutChild, pendingFlowBreak))
                    return nullptr;
                continue;
            }
            const std::string text = parseText();
            Node* added = nullptr;
            if (Element* element = parent->asElement())
                added = &appendText(*element, text);
            else
                added = appendFragmentText(*result, text);
            if (added && !trackFlowBreak(*added, hasLayoutChild, pendingFlowBreak))
                return nullptr;
        }
        if (pendingFlowBreak)
            return nullptr;
        if (contextElement)
            while (Node* child = contextElement->firstChild())
                result->append(child->remove());
        return result;
    }

private:
    std::string parseText() {
        const std::size_t end = mHTML.find('<', mOffset);
        const std::size_t textEnd = end == std::string_view::npos ? mHTML.size() : end;
        std::string result = decodeHTMLReferences(mHTML.substr(mOffset, textEnd - mOffset));
        mOffset = textEnd;
        return result;
    }

    bool skipComment() {
        const std::size_t end = mHTML.find("-->", mOffset + 4);
        if (end == std::string_view::npos)
            return false;
        mOffset = end + 3;
        return true;
    }

    bool readName(std::string& name) {
        const std::size_t begin = mOffset;
        while (mOffset < mHTML.size() && isHTMLNameCharacter(mHTML[mOffset]))
            ++mOffset;
        if (mOffset == begin)
            return false;
        name = canonicalizeHTMLName(mHTML.substr(begin, mOffset - begin));
        return true;
    }

    void skipWhitespace() {
        while (mOffset < mHTML.size() && isHTMLWhitespace(mHTML[mOffset]))
            ++mOffset;
    }

    bool readAttribute(Attribute& attribute) {
        std::string name;
        if (!readName(name))
            return false;
        attribute.name = std::move(name);
        attribute.value.clear();
        attribute.hasValue = false;
        skipWhitespace();
        if (mOffset >= mHTML.size() || mHTML[mOffset] != '=')
            return true;
        ++mOffset;
        attribute.hasValue = true;
        skipWhitespace();
        if (mOffset >= mHTML.size())
            return false;

        if (mHTML[mOffset] == '\'' || mHTML[mOffset] == '"') {
            const char quote = mHTML[mOffset++];
            const std::size_t begin = mOffset;
            while (mOffset < mHTML.size() && mHTML[mOffset] != quote)
                ++mOffset;
            if (mOffset == mHTML.size())
                return false;
            attribute.value = std::string(mHTML.substr(begin, mOffset - begin));
            ++mOffset;
            const bool selfClosingTerminator = mOffset + 1 < mHTML.size() && mHTML[mOffset] == '/' && mHTML[mOffset + 1] == '>';
            if (mOffset < mHTML.size() && !isHTMLWhitespace(mHTML[mOffset]) && mHTML[mOffset] != '>' && !selfClosingTerminator)
                return false;
        } else {
            const std::size_t begin = mOffset;
            while (mOffset < mHTML.size() && !isHTMLWhitespace(mHTML[mOffset]) && mHTML[mOffset] != '>')
                ++mOffset;
            attribute.value = std::string(mHTML.substr(begin, mOffset - begin));
        }
        attribute.value = decodeHTMLReferences(attribute.value);
        return true;
    }

    Node* parseElement(Node& parentNode, const std::vector<HTMLTag>* acceptedTags) {
        ++mOffset;
        std::string name;
        if (!readName(name))
            return nullptr;
        const HTMLTag tag = findHTMLTag(name);
        if (!acceptsScopedContent(acceptedTags, tag))
            return nullptr;
        Element* parent = parentNode.asElement();
        const ScopedElementDefinition* scoped = parent ? scopedChildDefinition(*parent, tag) : nullptr;
        const ResourceElementDefinition* definition = findElementDefinition(tag);
        if (!definition || (definition->scopedOnly && !scoped))
            return nullptr;

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
            if (!readAttribute(attribute))
                return nullptr;
            const bool duplicate = std::any_of(attributes.begin(), attributes.end(), [&attribute](const Attribute& existing) {
                return existing.name == attribute.name;
            });
            if (duplicate)
                return nullptr;
            attributes.push_back(std::move(attribute));
        }
        if (!closed)
            return nullptr;
        if (selfClosing && !isVoidHTMLTag(tag))
            return nullptr;

        ElementPtr owner;
        Element* element = nullptr;
        if (scoped) {
            if (!parent || !scoped->create)
                return nullptr;
            ResourceBuildResult result;
            ElementBuildContext context(result, nullptr);
            element = scoped->create(*parent, context, "<fragment>", 0, 0);
            if (!element || result.hasErrors())
                return nullptr;
        } else {
            owner = HTMLElementFactory::create(name);
            element = owner.get();
        }
        if (!element || !applyFragmentAttributes(*element, tag, name, attributes, scoped != nullptr))
            return nullptr;
        if (!scoped)
            appendNode(parentNode, std::move(owner));
        if (isVoidHTMLTag(tag))
            return element;

        bool pendingFlowBreak = false;
        bool hasLayoutChild = false;
        const std::vector<HTMLTag>* childAcceptedTags = scoped ? &scoped->acceptedTags : acceptedTags;
        const bool unsupportedContent = definition->contentBehavior.mode == ElementContentMode::Unsupported;
        while (mOffset < mHTML.size()) {
            if (mHTML.compare(mOffset, 4, "<!--") == 0) {
                if (!skipComment())
                    return nullptr;
                continue;
            }
            if (mHTML.compare(mOffset, 2, "</") == 0) {
                mOffset += 2;
                std::string closingName;
                if (!readName(closingName))
                    return nullptr;
                skipWhitespace();
                if (mOffset >= mHTML.size() || mHTML[mOffset++] != '>' || closingName != name)
                    return nullptr;
                if (pendingFlowBreak)
                    return nullptr;
                return element;
            }
            if (mHTML[mOffset] == '<') {
                if (unsupportedContent)
                    return nullptr;
                Node* child = parseElement(*element, childAcceptedTags);
                if (!child)
                    return nullptr;
                if (!trackFlowBreak(*child, hasLayoutChild, pendingFlowBreak))
                    return nullptr;
            } else {
                const std::string text = parseText();
                if (unsupportedContent && hasLayoutText(text))
                    return nullptr;
                Node& added = appendText(*element, text);
                if (!trackFlowBreak(added, hasLayoutChild, pendingFlowBreak))
                    return nullptr;
            }
        }
        return nullptr;
    }

    std::string_view mHTML;
    const Element* mContext = nullptr;
    std::size_t mOffset = 0;
};

FragmentPtr parseFragment(std::string_view html, const Element* context) { return FragmentParser(html, context).parse(); }

std::string serializeChildren(const Node& parent) {
    std::string result;
    for (const Node* child : parent.childNodes())
        FragmentSerializer::serializeNode(*child, result);
    return result;
}
} // namespace Core::detail
