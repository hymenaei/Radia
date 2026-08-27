/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "elements/label.h"
#include "elements/elementdefinition.h"
#include "localization.h"
#include "style/style.h"

namespace radia::ui {
namespace { constexpr char kElementName[] = "label"; }

LabelElement::LabelElement(std::string text) : Element(kElementName) {
    if (!text.empty()) textContent(std::move(text));
}

LabelElement& LabelElement::setTargetId(std::string id) {
    mTargetId = std::move(id);
    mTarget = nullptr;
    return *this;
}

void LabelElement::onActivate() {
    if (Element* target = mTarget) target->activateFromLabel();
}

ElementDefinition detail::ElementDefinitionFactory::label() {
    return defineElement<LabelElement>(kElementName)
        .attributes({allowedAttribute("for")})
        .validate([](const ElementBuildInput& input, LabelElement& label, LayoutBuildResult& result, const LayoutBuildContext*) {
            std::string targetId;
            if (!readElementAttribute(input, "for", targetId)) return;
            const ElementAttribute* attribute = input.find("for");
            if (!isElementIdentifier(targetId)) {
                result.error("layout.label.for_invalid", "LabelElement for must be a valid element id.", input.sourceName,
                             attribute->source.begin.line, attribute->source.begin.column);
                return;
            }
            label.setTargetId(std::move(targetId));
        })
        .composition([](const ElementBuildInput& input, LabelElement& label, const ElementScopeContext& scope, LayoutBuildResult& result) {
            const ElementAttribute* attribute = input.find("for");
            const SourceRange& sourceRange = attribute ? attribute->source : input.source;
            if (!attribute) {
                result.error("layout.label.for_required", "LabelElement requires a for element id.", input.sourceName, sourceRange.begin.line,
                             sourceRange.begin.column);
                return;
            }

            const std::string& targetId = detail::ElementCompilerAccess::labelTargetId(label);
            if (targetId.empty() || scope.ambiguous(targetId)) return;
            Element* target = scope.find(targetId);
            if (!target) {
                result.error("layout.label.target_missing", "LabelElement target is missing from its Layout Resource scope: " + targetId + ".",
                             input.sourceName, sourceRange.begin.line, sourceRange.begin.column);
                return;
            }
            if (!scope.labelable(*target)) {
                result.error("layout.label.target_not_labelable", "LabelElement target is not labelable: " + targetId + ".", input.sourceName,
                             sourceRange.begin.line, sourceRange.begin.column);
                return;
            }
            detail::ElementCompilerAccess::setLabelTarget(label, target);
        })
        .build();
}
} // namespace radia::ui
