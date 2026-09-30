/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "HTMLLabelElement.h"
#include <type_traits>
#include "ComputedStyle.h"
#include "ElementInternal.h"
#include "HTMLName.h"
#include "LocalizationCatalog.h"
#include "ResourceElementDefinition.h"

namespace Core {
namespace {
Element* scopeRootForLabel(HTMLLabelElement& label) {
    Element* root = &label;
    while (root->parentElement() && !root->idScopeRoot())
        root = root->parentElement();
    return root;
}

const Element* scopeRootForLabel(const HTMLLabelElement& label) {
    const Element* root = &label;
    while (root->parentElement() && !root->idScopeRoot())
        root = root->parentElement();
    return root;
}

bool isLabelable(const Element& element) {
    const ResourceElementDefinition* definition = findElementDefinition(findHTMLTag(element.elementName()));
    return definition && definition->labelable;
}

template<typename LabelT> auto findImplicitLabelTarget(LabelT& label) {
    using ElementPointer = std::conditional_t<std::is_const_v<LabelT>, const Element*, Element*>;
    ElementPointer result = nullptr;
    std::size_t count = 0;
    const auto visit = [&](auto&& self, auto& current) -> void {
        for (auto* child : current.children()) {
            if (isLabelable(*child)) {
                ++count;
                if (count == 1)
                    result = child;
            }
            if (!child->idScopeRoot())
                self(self, *child);
        }
    };
    visit(visit, label);
    return count == 1 ? result : nullptr;
}

std::size_t implicitLabelTargetCount(const Element& label) {
    std::size_t count = 0;
    const auto visit = [&](auto&& self, const Element& current) -> void {
        for (const Element* child : current.children()) {
            if (isLabelable(*child))
                ++count;
            if (!child->idScopeRoot())
                self(self, *child);
        }
    };
    visit(visit, label);
    return count;
}

template<typename LabelT, typename IndexT> auto findLabelTarget(LabelT& label) {
    const Element::Attribute* targetAttribute = label.attribute("for");
    if (!targetAttribute)
        return findImplicitLabelTarget(label);
    if (!targetAttribute->value || targetAttribute->value->empty())
        return static_cast<typename IndexT::ElementPointer>(nullptr);

    IndexT index;
    detail::indexElementsInScope(*scopeRootForLabel(label), index);
    const auto found = index.first.find(*targetAttribute->value);
    if (found == index.first.end() || index.ambiguous.contains(*targetAttribute->value))
        return static_cast<typename IndexT::ElementPointer>(nullptr);
    return isLabelable(*found->second) ? found->second : static_cast<typename IndexT::ElementPointer>(nullptr);
}
} // namespace

HTMLLabelElement::HTMLLabelElement(std::string text)
    : HTMLElement(HTMLTagName(HTMLTag::Label)) {
    if (!text.empty())
        textContent(std::move(text));
}

HTMLLabelElement& HTMLLabelElement::setTargetId(std::string id) {
    mTargetId = std::move(id);
    if (mTargetId.empty())
        removeAttribute("for");
    else
        setAttribute("for", mTargetId);
    return *this;
}

Element* HTMLLabelElement::target() { return findLabelTarget<HTMLLabelElement, detail::ElementIdIndex>(*this); }

const Element* HTMLLabelElement::target() const { return findLabelTarget<const HTMLLabelElement, detail::ConstElementIdIndex>(*this); }

AccessibleSemantics HTMLLabelElement::accessibleSemantics() const {
    AccessibleSemantics result = HTMLElement::accessibleSemantics();
    result.role = AccessibleRole::Label;
    result.labelTarget = target();
    return result;
}

void HTMLLabelElement::onActivate() {
    if (Element* targetElement = target())
        targetElement->activateFromLabel();
}

ResourceElementDefinition detail::ElementDefinitions::label() {
    return defineElement<HTMLLabelElement>(HTMLTagName(HTMLTag::Label))
        .attributes({allowedAttribute("for")})
        .validate([](const ElementBuildInput& input, HTMLLabelElement& label, ElementBuildContext& context) {
            std::string targetId;
            if (!readElementAttribute(input, "for", targetId))
                return;
            const ElementAttribute* attribute = input.find("for");
            if (targetId.empty() || containsHTMLWhitespace(targetId)) {
                context.error("layout.label.for_invalid", "HTMLLabelElement for must be non-empty and contain no ASCII whitespace.",
                    input.sourceName, attribute->source.begin.line, attribute->source.begin.column);
                return;
            }
            label.setTargetId(std::move(targetId));
        })
        .composition(
            [](const ElementBuildInput& input, HTMLLabelElement& label, const ElementScopeContext& scope, ElementBuildContext& context) {
                const ElementAttribute* attribute = input.find("for");
                const SourceRange& sourceRange = attribute ? attribute->source : input.source;
                if (!attribute) {
                    if (findImplicitLabelTarget(label))
                        return;
                    const bool ambiguous = implicitLabelTargetCount(label) > 1;
                    context.error(ambiguous ? "layout.label.target_ambiguous" : "layout.label.for_required",
                        ambiguous ? "HTMLLabelElement has more than one labelable descendant."
                                  : "HTMLLabelElement requires a for element id or one labelable descendant.",
                        input.sourceName, sourceRange.begin.line, sourceRange.begin.column);
                    return;
                }

                const std::string& targetId = label.targetId();
                if (targetId.empty())
                    return;
                if (scope.ambiguous(targetId)) {
                    context.error("layout.label.target_ambiguous",
                        "HTMLLabelElement target is ambiguous in its Layout Resource scope: " + targetId + ".", input.sourceName,
                        sourceRange.begin.line, sourceRange.begin.column);
                    return;
                }
                Element* target = scope.find(targetId);
                if (!target) {
                    context.error("layout.label.target_missing",
                        "HTMLLabelElement target is missing from its Layout Resource scope: " + targetId + ".", input.sourceName,
                        sourceRange.begin.line, sourceRange.begin.column);
                    return;
                }
                if (!scope.labelable(*target)) {
                    context.error("layout.label.target_not_labelable", "HTMLLabelElement target is not labelable: " + targetId + ".",
                        input.sourceName, sourceRange.begin.line, sourceRange.begin.column);
                    return;
                }
            })
        .build();
}
} // namespace Core
