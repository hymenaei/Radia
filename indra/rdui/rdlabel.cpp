#include "linden_common.h"
#include "rdlabel.h"
#include "rduilocalization.h"
#include "rduipaintcontext.h"
#include "rduistyle.h"
#include "rduisystem.h"
#include "rduiviewcontract.h"

namespace rdui
{
    Label::Label(std::string text) : Label(ELEMENT, std::move(text)) {}

    Label::Label(const char* element, std::string text) : Widget(element), mText(InlineContent::text(std::move(text))) {}

    Label& Label::setText(std::string text)
    {
        return setContent(InlineContent::text(std::move(text)));
    }

    Label& Label::setContent(TextSource content)
    {
        mText.setContent(std::move(content));
        if (const System* system = attachedSystem())
            onLocaleChanged(*system);
        invalidateMeasure();
        return *this;
    }

    Label& Label::setContent(InlineContent content)
    {
        return setContent(TextSource::literal(std::move(content)));
    }

    Label& Label::setTargetId(std::string id)
    {
        mTargetId = std::move(id);
        mTarget.set(nullptr);
        return *this;
    }

    void Label::onActivate()
    {
        if (Widget* target = mTarget.get()) target->activateFromLabel();
    }

    void Label::onLocaleChanged(const System& system)
    {
        mText.resolveLocalized([&system](const LocalizationRequest& request)
        {
            return system.resolveContent(request);
        });
        mText.resolveKeybindings([&system](const std::string& key) { return system.resolveKeybinding(key); });
        invalidateMeasure();
    }

    bool Label::onKeybindingsChanged(const System& system)
    {
        const bool changed = mText.resolveKeybindings([&system](const std::string& key) { return system.resolveKeybinding(key); });
        if (changed) invalidateMeasure();
        return changed;
    }

    Vec2 Label::intrinsicSize(const StyleSheet& theme, const Style& style, const TextMetrics& text_metrics) const
    {
        return mText.measure(text_metrics, style, theme, *this);
    }

    void Label::paint(PaintContext& context, const Style& style, float) const
    {
        context.paintBox(rect(), style);
        mText.paint(context, rect(), style, attachedStyleSheet(), *this);
    }

    WidgetContract detail::labelContract()
    {
        return defineWidget<Label>(Label::ELEMENT)
            .attributes({allowedAttribute("for")})
            .validate([](const LayoutElement& element, Label& label, ViewBuildResult& result, const std::string& source, const ViewBuildContext*)
            {
                std::string target_id;
                if (!readViewAttribute(element, "for", target_id)) return;
                const LayoutAttribute* attribute = element.attribute("for");
                if (!isLocalIdentifier(target_id))
                {
                    result.error("view.label.for_invalid",
                                 "Label for must be a lowercase kebab-case widget id.", source,
                                 attribute->source.begin.line, attribute->source.begin.column);
                    return;
                }
                label.setTargetId(std::move(target_id));
            })
            .composition([](const LayoutElement& element, Label& label, const ViewScopeContext& scope, ViewBuildResult& result, const std::string& source)
            {
                const LayoutAttribute* attribute = element.attribute("for");
                const SourceRange& source_range = attribute ? attribute->source : element.source();
                if (!attribute)
                {
                    result.error("view.label.for_required", "Label requires a for widget id.", source, source_range.begin.line, source_range.begin.column);
                    return;
                }

                const std::string& target_id = detail::WidgetCompilerAccess::labelTargetId(label);
                if (target_id.empty() || scope.ambiguous(target_id)) return;
                Widget* target = scope.find(target_id);
                if (!target)
                {
                    result.error("view.label.target_missing",
                                 "Label target is missing from its Layout Resource scope: " + target_id + ".",
                                 source, source_range.begin.line, source_range.begin.column);
                    return;
                }
                if (!scope.labelable(*target))
                {
                    result.error("view.label.target_not_labelable",
                                 "Label target is not labelable: " + target_id + ".",
                                 source, source_range.begin.line, source_range.begin.column);
                    return;
                }
                detail::WidgetCompilerAccess::setLabelTarget(label, target);
            })
            .inlineContent({InlineContentKind::B, InlineContentKind::I, InlineContentKind::S, InlineContentKind::Kbd, InlineContentKind::Br},
                           [](TextSource content, Label& label) { label.setContent(std::move(content)); })
            .build();
    }
}
