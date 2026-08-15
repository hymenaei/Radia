/**
 * @file label.cpp
 * @brief Defines Label targeting and activation for named controls.
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
#include "widgets/label.h"
#include "localization/localization.h"
#include "render/paintcontext.h"
#include "style/style.h"
#include "system.h"
#include "widgets/widgetcontractbuilder.h"

namespace radia::ui {
Label::Label(std::string text) : Label(sElement, std::move(text)) {}

Label::Label(const char* elementName, std::string text) : Widget(elementName), mText(InlineContent::text(std::move(text))) {}

Label& Label::setText(std::string text) {
    return setContent(InlineContent::text(std::move(text)));
}

Label& Label::setContent(TextSource content) {
    mText.setContent(std::move(content));
    if (const System* system = attachedSystem()) onLocaleChanged(*system);
    else invalidateText();
    return *this;
}

Label& Label::setContent(InlineContent content) {
    return setContent(TextSource::literal(std::move(content)));
}

bool Label::setTextContent(TextSource content) {
    setContent(std::move(content));
    return true;
}

Label& Label::setTargetId(std::string id) {
    mTargetId = std::move(id);
    mTarget.set(nullptr);
    return *this;
}

void Label::onActivate() {
    if (Widget* target = mTarget.get()) target->activateFromLabel();
}

void Label::onLocaleChanged(const System& system) {
    mText.resolveLocalized([&system](const LocalizationRequest& request) { return system.resolveContent(request); });
    mText.resolveKeybindings([&system](const std::string& key) { return system.resolveKeybinding(key); });
    invalidateText();
}

bool Label::onKeybindingsChanged(const System& system) {
    const bool changed = mText.resolveKeybindings([&system](const std::string& key) { return system.resolveKeybinding(key); });
    if (changed) invalidateText();
    return changed;
}

Vec2 Label::intrinsicSize(const StyleSheet& styleSheet, const Style& style, const TextMetrics& textMetrics,
                          const IntrinsicSizeConstraints& constraints) const {
    return mText.measure(textMetrics, style, styleSheet, *this, constraints.width);
}

void Label::paint(PaintContext& context, const Style& style, float) const {
    context.paintBox(rect(), style);
    mText.paint(context, insetRect(rect(), style.padding), style, attachedStyleSheet(), *this);
}

WidgetContract detail::labelContract() {
    return defineWidget<Label>(Label::sElement)
        .attributes({allowedAttribute("for")})
        .validate([](const LayoutElement& element, Label& label, LayoutBuildResult& result, const std::string& source, const LayoutBuildContext*) {
            std::string targetId;
            if (!readLayoutAttribute(element, "for", targetId)) return;
            const LayoutAttribute* attribute = element.attribute("for");
            if (!isWidgetIdentifier(targetId)) {
                result.error("layout.label.for_invalid", "Label for must be a valid widget id.", source, attribute->source.begin.line,
                             attribute->source.begin.column);
                return;
            }
            label.setTargetId(std::move(targetId));
        })
        .composition(
            [](const LayoutElement& element, Label& label, const WidgetScopeContext& scope, LayoutBuildResult& result, const std::string& source) {
                const LayoutAttribute* attribute = element.attribute("for");
                const SourceRange& sourceRange = attribute ? attribute->source : element.source();
                if (!attribute) {
                    result.error("layout.label.for_required", "Label requires a for widget id.", source, sourceRange.begin.line,
                                 sourceRange.begin.column);
                    return;
                }

                const std::string& targetId = detail::WidgetCompilerAccess::labelTargetId(label);
                if (targetId.empty() || scope.ambiguous(targetId)) return;
                Widget* target = scope.find(targetId);
                if (!target) {
                    result.error("layout.label.target_missing", "Label target is missing from its Layout Resource scope: " + targetId + ".", source,
                                 sourceRange.begin.line, sourceRange.begin.column);
                    return;
                }
                if (!scope.labelable(*target)) {
                    result.error("layout.label.target_not_labelable", "Label target is not labelable: " + targetId + ".", source,
                                 sourceRange.begin.line, sourceRange.begin.column);
                    return;
                }
                detail::WidgetCompilerAccess::setLabelTarget(label, target);
            })
        .inlineContent({InlineContentKind::B, InlineContentKind::I, InlineContentKind::S, InlineContentKind::Kbd, InlineContentKind::Br},
                       [](TextSource content, Label& label) { label.setContent(std::move(content)); })
        .build();
}
} // namespace radia::ui
