#include "linden_common.h"
#include "rduiviewcontract.h"
#include "rdlabel.h"
#include "rduiwidgetcatalog.h"
#include <algorithm>
#include <cctype>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <limits>
#include <sstream>
#include <unordered_map>
#include <unordered_set>

namespace rdui
{
    void detail::WidgetCompilerAccess::setStyleIdentity(Widget& widget, std::string element, std::string part)
    {
        widget.setStyleElement(std::move(element));
        widget.setPart(std::move(part));
    }

    void detail::WidgetCompilerAccess::setIdScopeRoot(Widget& widget)
    {
        widget.setIdScopeRoot(true);
    }

    void detail::WidgetCompilerAccess::setState(Widget& widget, WidgetState state, bool enabled)
    {
        widget.setState(state, enabled);
    }

    const std::string& detail::WidgetCompilerAccess::labelTargetId(const Label& label)
    {
        return label.mTargetId;
    }

    Widget* detail::WidgetCompilerAccess::labelTarget(const Label& label)
    {
        return label.mTarget.get();
    }

    void detail::WidgetCompilerAccess::setLabelTarget(Label& label, Widget* target)
    {
        label.mTarget.set(target);
    }

    void detail::WidgetCompilerAccess::setFlowBreakBefore(Widget& widget, bool enabled)
    {
        widget.mFlowBreakBefore = enabled;
    }

    const char* actionAttribute(ActionEventKind kind)
    {
        switch (kind)
        {
            case ActionEventKind::Click: return "onClick";
            case ActionEventKind::DoubleClick: return "onDoubleClick";
            case ActionEventKind::Change: return "onChange";
            case ActionEventKind::MouseDown: return "onMouseDown";
            case ActionEventKind::MouseUp: return "onMouseUp";
            case ActionEventKind::MouseMove: return "onMouseMove";
            case ActionEventKind::LongClick: return "onLongClick";
            case ActionEventKind::ContextMenu: return "onContextMenu";
        }
        return "";
    }

    namespace
    {
        std::optional<std::chrono::milliseconds> durationValue(const std::string& value)
        {
            char* suffix = nullptr;
            const double amount = std::strtod(value.c_str(), &suffix);
            if (suffix == value.c_str() || !std::isfinite(amount) || amount <= 0.0) return std::nullopt;
            double milliseconds = 0.0;
            if (std::string(suffix) == "ms") milliseconds = amount;
            else if (std::string(suffix) == "s") milliseconds = amount * 1000.0;
            else return std::nullopt;
            if (milliseconds > static_cast<double>(std::numeric_limits<long long>::max())) return std::nullopt;
            const auto rounded = static_cast<long long>(std::llround(milliseconds));
            return rounded > 0 ? std::optional<std::chrono::milliseconds>(std::chrono::milliseconds(rounded)) : std::nullopt;
        }

        using CompositeInstances = std::unordered_map<std::string, Widget*>;

        void validateCompositeContract(const WidgetContract& contract)
        {
            std::unordered_set<std::string> paths;
            paths.reserve(contract.composite_parts.size());
            for (const CompositePartContract& part : contract.composite_parts)
            {
                llassert_always(!part.path.empty());
                llassert_always(part.create);
                llassert_always(paths.insert(part.path).second);
            }
            for (const CompositePartContract& part : contract.composite_parts) llassert_always(part.parent_path.empty() || paths.count(part.parent_path) == 1);
        }

        void collectCompositeInstances(Widget& owner, const WidgetContract& contract, CompositeInstances& instances)
        {
            for (const CompositePartContract& part : contract.composite_parts)
            {
                Widget* parent = &owner;
                if (!part.parent_path.empty())
                {
                    const auto found = instances.find(part.parent_path);
                    if (found == instances.end()) continue;
                    parent = found->second;
                }
                const auto existing = std::find_if(parent->children().begin(), parent->children().end(),
                    [&owner, &part](const std::unique_ptr<Widget>& child)
                    {
                        return child->styleElement() == owner.styleElement() && child->part() == part.path;
                    });
                if (existing != parent->children().end()) instances.emplace(part.path, existing->get());
            }
        }

        Widget* instantiateCompositePartImpl(Widget& owner, const WidgetContract& contract,
                                             const CompositePartContract& part,
                                             CompositeInstances& instances,
                                             std::unordered_set<std::string>& constructing)
        {
            const auto existing = instances.find(part.path);
            if (existing != instances.end()) return existing->second;
            llassert_always(constructing.insert(part.path).second);
            llassert_always(!part.path.empty());
            llassert_always(part.create);

            Widget* parent = &owner;
            if (!part.parent_path.empty())
            {
                const auto parent_contract = std::find_if(contract.composite_parts.begin(), contract.composite_parts.end(),
                    [&part](const CompositePartContract& candidate)
                    {
                        return candidate.path == part.parent_path;
                    });
                llassert_always(parent_contract != contract.composite_parts.end());
                parent = instantiateCompositePartImpl(owner, contract, *parent_contract, instances, constructing);
            }

            std::unique_ptr<Widget> child = part.create();
            llassert_always(child != nullptr);
            detail::WidgetCompilerAccess::setStyleIdentity(*child, owner.styleElement(), part.path);
            Widget* instance = child.get();
            parent->Widget::addChild(std::move(child));
            instances.emplace(part.path, instance);
            constructing.erase(part.path);
            if (part.bind) part.bind(owner, *instance);
            return instance;
        }
    }

    const WidgetContract* findWidgetContract(const std::string& element)
    {
        const auto& contracts = builtInWidgetContracts();
        const auto widget = contracts.find(schemaNameKey(element));
        return widget == contracts.end() ? nullptr : &widget->second;
    }

    const CompositePartContract* findCompositePartContract(const WidgetContract& widget,
                                                           const std::vector<std::string>& parts)
    {
        if (parts.empty()) return nullptr;
        std::string path;
        for (const std::string& part : parts)
        {
            if (!path.empty()) path += "::";
            path += part;
        }
        const auto composite = std::find_if(widget.composite_parts.begin(), widget.composite_parts.end(),
                                            [&path](const CompositePartContract& part) { return part.path == path; });
        return composite == widget.composite_parts.end() ? nullptr : &*composite;
    }

    bool producesState(const WidgetContract& widget, WidgetState state)
    {
        return std::find(widget.produced_states.begin(), widget.produced_states.end(), state)
               != widget.produced_states.end();
    }

    bool producesState(const CompositePartContract& part, WidgetState state)
    {
        return std::find(part.produced_states.begin(), part.produced_states.end(), state)
               != part.produced_states.end();
    }

    void detail::instantiateCompositeParts(Widget& owner, const WidgetContract& contract)
    {
        validateCompositeContract(contract);
        CompositeInstances instances;
        instances.reserve(contract.composite_parts.size());
        collectCompositeInstances(owner, contract, instances);
        std::unordered_set<std::string> constructing;
        for (const CompositePartContract& part : contract.composite_parts)
        {
            if (part.eager) instantiateCompositePartImpl(owner, contract, part, instances, constructing);
        }
    }

    Widget* detail::instantiateCompositePart(Widget& owner, const WidgetContract& contract, const std::string& path)
    {
        validateCompositeContract(contract);
        const auto part = std::find_if(contract.composite_parts.begin(), contract.composite_parts.end(),
                                       [&path](const CompositePartContract& candidate) { return candidate.path == path; });
        llassert_always(part != contract.composite_parts.end());
        CompositeInstances instances;
        instances.reserve(contract.composite_parts.size());
        collectCompositeInstances(owner, contract, instances);
        std::unordered_set<std::string> constructing;
        return instantiateCompositePartImpl(owner, contract, *part, instances, constructing);
    }

    bool readViewAttribute(const LayoutElement& element, const char* name, std::string& value)
    {
        const LayoutAttribute* attribute = element.attribute(name);
        if (!attribute) return false;
        value = attribute->value;
        return true;
    }

    bool readViewBoolean(const LayoutElement& element, const char* name, bool& value, ViewBuildResult& result, const std::string& source)
    {
        std::string text;
        if (!readViewAttribute(element, name, text)) return false;
        if (text == "true" || text == "1") value = true;
        else if (text == "false" || text == "0") value = false;
        else
        {
            result.error("view.attribute.boolean_invalid", "Invalid boolean value for " + std::string(name) + ": " + text + ".", source,
                         element.source().begin.line, element.source().begin.column);
            return false;
        }
        return true;
    }

    namespace
    {
        bool readViewVisibility(const LayoutElement& element, Visibility& value,
                                ViewBuildResult& result, const std::string& source)
        {
            std::string text;
            if (!readViewAttribute(element, "visibility", text)) return false;
            if (text == "visible") value = Visibility::Visible;
            else if (text == "hidden") value = Visibility::Hidden;
            else if (text == "collapsed") value = Visibility::Collapsed;
            else
            {
                result.error("view.attribute.visibility_invalid",
                             "Invalid visibility value: " + text + ". Expected visible, hidden, or collapsed.",
                             source, element.source().begin.line, element.source().begin.column);
                return false;
            }
            return true;
        }
    }

    TextValue localizedViewText(std::string value, ViewBuildResult& result, const std::string& source,
                                const ViewBuildContext* context, std::size_t line)
    {
        if (!context) return TextValue::literal(std::move(value));
        if (!context->hasLocalizationKey(value))
            result.error("view.localization.missing", "Unknown localization key: " + value + ".", source, line);
        return context->localized(std::move(value));
    }

    void validateViewAttributes(const LayoutElement& element, const std::vector<std::string>& widget_attributes, ViewBuildResult& result, const std::string& source)
    {
        static const std::unordered_set<std::string> common = {
            "id", "class", "visibility", "disabled", "longClickDelay",
            "onClick", "onDoubleClick", "onChange", "onMouseDown", "onMouseUp", "onMouseMove",
            "onLongClick", "onContextMenu",
            "x", "y", "width", "height", "interactive", "blocksPointer", "action",
        };
        std::unordered_set<std::string> allowed;
        for (const std::string& name : widget_attributes) allowed.insert(schemaNameKey(name));
        for (const std::string& name : common) allowed.insert(schemaNameKey(name));
        const WidgetContract* widget_contract = findWidgetContract(element.name());
        const std::string element_name = widget_contract ? widget_contract->element : schemaNameKey(element.name());
        for (const auto& [attribute_name, attribute] : element.attributes())
        {
            if (!allowed.count(attribute_name))
                result.error("view.attribute.unknown", "Unknown attribute on <" + element_name + ">: " + attribute.authored_name + ".", source,
                             attribute.source.begin.line, attribute.source.begin.column);
        }
    }

    void applyCommonViewAttributes(const LayoutElement& element, Widget& widget, ViewBuildResult& result,
                                   const std::string& source,
                                   const std::vector<ActionEventKind>& supported_actions)
    {
        static const char* unsupported[] = {
            "x", "y", "width", "height", "interactive", "blocksPointer", "action",
        };
        for (const char* name : unsupported)
        {
            std::string ignored;
            if (readViewAttribute(element, name, ignored))
                result.error("view.attribute.unsupported", std::string("Unsupported XML attribute: ") + name + ".", source,
                             element.source().begin.line, element.source().begin.column);
        }

        std::string value;
        if (readViewAttribute(element, "id", value))
        {
            const LayoutAttribute* attribute = element.attribute("id");
            if (!isLocalIdentifier(value))
                result.error("view.id.invalid", "Widget id must be a lowercase kebab-case identifier.", source,
                             attribute->source.begin.line, attribute->source.begin.column);
            else widget.setId(value);
        }
        if (readViewAttribute(element, "class", value))
        {
            std::stringstream classes(value);
            while (classes >> value)
            {
                if (!isLocalIdentifier(value))
                {
                    const LayoutAttribute* attribute = element.attribute("class");
                    result.error("view.class.invalid", "Widget class must be a lowercase kebab-case identifier.", source,
                                 attribute->source.begin.line, attribute->source.begin.column);
                }
                else widget.addClass(value);
            }
        }
        Visibility visibility = Visibility::Visible;
        if (readViewVisibility(element, visibility, result, source)) widget.setVisibility(visibility);
        bool boolean = false;
        if (readViewBoolean(element, "disabled", boolean, result, source)) widget.setDisabled(boolean);

        for (ActionEventKind kind : {ActionEventKind::Click, ActionEventKind::DoubleClick, ActionEventKind::Change,
                                     ActionEventKind::MouseDown, ActionEventKind::MouseUp, ActionEventKind::MouseMove,
                                     ActionEventKind::LongClick, ActionEventKind::ContextMenu})
        {
            const char* name = actionAttribute(kind);
            if (!readViewAttribute(element, name, value)) continue;
            if (std::find(supported_actions.begin(), supported_actions.end(), kind) == supported_actions.end())
                result.error("view.action.unsupported", "Action attribute " + std::string(name) + " is not supported on <" + widget.element() + ">.", source);
            else if (!isLocalIdentifier(value))
            {
                const LayoutAttribute* attribute = element.attribute(name);
                result.error("view.action.name_invalid", "Invalid action name for " + std::string(name) + ".", source,
                             attribute->source.begin.line, attribute->source.begin.column);
            }
            else
                widget.setAction(kind, value);
        }

        if (readViewAttribute(element, "longClickDelay", value))
        {
            const bool supports_long_click = std::find(supported_actions.begin(), supported_actions.end(), ActionEventKind::LongClick)
                                            != supported_actions.end();
            if (supports_long_click && widget.action(ActionEventKind::LongClick).empty())
                result.error("view.long_click.action_missing", "longClickDelay requires onLongClick.", source);
            else if (!supports_long_click)
                result.error("view.action.unsupported", "longClickDelay is not supported on <" + widget.element() + ">.", source);
            else
            {
                const auto duration = durationValue(value);
                if (duration) widget.setLongClickDelay(*duration);
                else result.error("view.attribute.duration_invalid", "Invalid duration for longClickDelay: " + value + ".", source);
            }
        }
    }
}
