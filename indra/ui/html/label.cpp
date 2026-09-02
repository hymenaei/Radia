/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/label.h"
#include "dom/elementinternal.h"
#include "html/elementnames.h"
#include "localization.h"
#include "resource/elementdefinition.h"
#include "style/style.h"

namespace radia::ui {
using detail::ElementCompilerAccess;
using detail::findElementInScope;

namespace {
const Element* scopeRootForLabel(const HTMLLabelElement& label) {
    const Element* root = &label;
    while (root->parentElement() && !root->idScopeRoot()) root = root->parentElement();
    return root;
}

bool isLabelable(const Element& element) {
    const ResourceElementDefinition* definition = findElementDefinition(lookupHTMLTag(element.elementName()));
    return definition && definition->labelable;
}

} // namespace

HTMLLabelElement::HTMLLabelElement(std::string text) : HTMLElement(kLabelTag.localName) {
    if (!text.empty()) textContent(std::move(text));
}

HTMLLabelElement& HTMLLabelElement::setTargetId(std::string id) {
    mTargetId = std::move(id);
    if (mTargetId.empty()) removeAttribute("for");
    else setAttribute("for", mTargetId);
    return *this;
}

Element* HTMLLabelElement::target() const {
    const Attribute* targetAttribute = attribute("for");
    if (!targetAttribute || !targetAttribute->value || targetAttribute->value->empty()) return nullptr;

    const Element* candidate = findElementInScope(*scopeRootForLabel(*this), *targetAttribute->value);
    return candidate && isLabelable(*candidate) ? const_cast<Element*>(candidate) : nullptr;
}

void HTMLLabelElement::onActivate() {
    if (Element* targetElement = target()) targetElement->activateFromLabel();
}

ResourceElementDefinition detail::ElementDefinitions::label() {
    return defineElement<HTMLLabelElement>(kLabelTag.localName)
        .attributes({allowedAttribute("for")})
        .validate([](const ElementBuildInput& input, HTMLLabelElement& label, ElementBuildContext& context) {
            std::string targetId;
            if (!readElementAttribute(input, "for", targetId)) return;
            const ElementAttribute* attribute = input.find("for");
            if (targetId.empty() || containsHTMLWhitespace(targetId)) {
                context.error("layout.label.for_invalid", "HTMLLabelElement for must be non-empty and contain no ASCII whitespace.", input.sourceName,
                              attribute->source.begin.line, attribute->source.begin.column);
                return;
            }
            label.setTargetId(std::move(targetId));
        })
        .composition([](const ElementBuildInput& input, HTMLLabelElement& label, const ElementScopeContext& scope, ElementBuildContext& context) {
            const ElementAttribute* attribute = input.find("for");
            const SourceRange& sourceRange = attribute ? attribute->source : input.source;
            if (!attribute) {
                context.error("layout.label.for_required", "HTMLLabelElement requires a for element id.", input.sourceName, sourceRange.begin.line,
                              sourceRange.begin.column);
                return;
            }

            const std::string& targetId = ElementCompilerAccess::labelTargetId(label);
            if (targetId.empty()) return;
            if (scope.ambiguous(targetId)) {
                context.error("layout.label.target_ambiguous", "HTMLLabelElement target is ambiguous in its Layout Resource scope: " + targetId + ".",
                              input.sourceName, sourceRange.begin.line, sourceRange.begin.column);
                return;
            }
            Element* target = scope.find(targetId);
            if (!target) {
                context.error("layout.label.target_missing", "HTMLLabelElement target is missing from its Layout Resource scope: " + targetId + ".",
                              input.sourceName, sourceRange.begin.line, sourceRange.begin.column);
                return;
            }
            if (!scope.labelable(*target)) {
                context.error("layout.label.target_not_labelable", "HTMLLabelElement target is not labelable: " + targetId + ".", input.sourceName,
                              sourceRange.begin.line, sourceRange.begin.column);
                return;
            }
        })
        .build();
}
} // namespace radia::ui
