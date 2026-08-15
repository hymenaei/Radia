/**
 * @file widgetcontract.cpp
 * @brief Implements Widget Contract validation and composite topology.
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
#include "widgets/widgetcontract.h"
#include <algorithm>
#include <cctype>
#include <chrono>
#include <cmath>
#include <cstdlib>
#include <limits>
#include <sstream>
#include <unordered_map>
#include <unordered_set>
#include "widgets/label.h"
#include "widgets/widgetcatalog.h"

namespace radia::ui {
void detail::WidgetCompilerAccess::setStyleIdentity(Widget& widget, std::string elementName, std::string part) {
    widget.setStyleElement(std::move(elementName));
    widget.setPart(std::move(part));
}

void detail::WidgetCompilerAccess::setIdScopeRoot(Widget& widget) {
    widget.setIdScopeRoot(true);
}

void detail::WidgetCompilerAccess::setState(Widget& widget, WidgetState state, bool enabled) {
    widget.setState(state, enabled);
}

const std::string& detail::WidgetCompilerAccess::labelTargetId(const Label& label) {
    return label.mTargetId;
}

Widget* detail::WidgetCompilerAccess::labelTarget(const Label& label) {
    return label.mTarget.get();
}

void detail::WidgetCompilerAccess::setLabelTarget(Label& label, Widget* target) {
    label.mTarget.set(target);
}

void detail::WidgetCompilerAccess::setFlowBreakBefore(Widget& widget, bool enabled) {
    widget.mFlowBreakBefore = enabled;
}

const char* eventAttribute(WidgetEventKind kind) {
    switch (kind) {
#define WIDGET_EVENT_ENTRY(name, attribute)                                                                                                          \
    case WidgetEventKind::name: return attribute;
#include "widgetevents.def"
#undef WIDGET_EVENT_ENTRY
    }
    return "";
}

namespace {
std::optional<std::chrono::milliseconds> durationValue(const std::string& value) {
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

CompositeTopology makeCompositeTopology(const WidgetContract& contract) {
    CompositeTopology topology;
    topology.indices.reserve(contract.compositeParts.size());
    for (std::size_t index = 0; index < contract.compositeParts.size(); ++index) {
        const CompositePartContract& part = contract.compositeParts[index];
        if (part.path.empty() || !part.create || !topology.indices.emplace(part.path, index).second) topology.valid = false;
    }
    if (!topology.valid) return topology;

    std::vector<uint8_t> state(contract.compositeParts.size(), 0);
    std::function<void(std::size_t)> visit = [&](std::size_t index) {
        if (!topology.valid || state[index] == 2) return;
        if (state[index] == 1) {
            topology.valid = false;
            return;
        }
        state[index] = 1;
        const CompositePartContract& part = contract.compositeParts[index];
        if (!part.parentPath.empty()) {
            const auto parent = topology.indices.find(part.parentPath);
            if (parent == topology.indices.end()) {
                topology.valid = false;
                return;
            }
            visit(parent->second);
        }
        state[index] = 2;
        topology.order.push_back(index);
    };
    for (std::size_t index = 0; index < contract.compositeParts.size() && topology.valid; ++index) visit(index);
    if (!topology.valid) topology.order.clear();
    return topology;
}

const CompositeTopology* topologyFor(const WidgetContract& contract, CompositeTopology& fallback) {
    if (contract.compositeTopology) return contract.compositeTopology.get();
    fallback = makeCompositeTopology(contract);
    return &fallback;
}

void collectCompositeInstances(Widget& owner, const WidgetContract& contract, const CompositeTopology& topology, CompositeInstances& instances) {
    for (const std::size_t index : topology.order) {
        const CompositePartContract& part = contract.compositeParts[index];
        Widget* parent = &owner;
        if (!part.parentPath.empty()) {
            const auto found = instances.find(part.parentPath);
            if (found == instances.end()) continue;
            parent = found->second;
        }
        const auto existing =
            std::find_if(parent->children().begin(), parent->children().end(), [&owner, &part](const std::unique_ptr<Widget>& child) {
                return child->styleElement() == owner.styleElement() && child->part() == part.path;
            });
        if (existing != parent->children().end()) instances.emplace(part.path, existing->get());
    }
}

Widget* instantiateCompositePartImpl(Widget& owner, const WidgetContract& contract, const CompositeTopology& topology, std::size_t partIndex,
                                     CompositeInstances& instances, std::unordered_set<std::string>& constructing) {
    if (!topology.valid || partIndex >= contract.compositeParts.size()) return nullptr;
    const CompositePartContract& part = contract.compositeParts[partIndex];
    const auto existing = instances.find(part.path);
    if (existing != instances.end()) return existing->second;
    if (!constructing.insert(part.path).second || part.path.empty() || !part.create) return nullptr;

    Widget* parent = &owner;
    if (!part.parentPath.empty()) {
        const auto parentIndex = topology.indices.find(part.parentPath);
        if (parentIndex == topology.indices.end()) {
            constructing.erase(part.path);
            return nullptr;
        }
        parent = instantiateCompositePartImpl(owner, contract, topology, parentIndex->second, instances, constructing);
        if (!parent) {
            constructing.erase(part.path);
            return nullptr;
        }
    }

    std::unique_ptr<Widget> child = part.create();
    if (!child) {
        constructing.erase(part.path);
        return nullptr;
    }
    detail::WidgetCompilerAccess::setStyleIdentity(*child, owner.styleElement(), part.path);
    Widget* instance = child.get();
    parent->Widget::addChild(std::move(child));
    instances.emplace(part.path, instance);
    constructing.erase(part.path);
    if (part.bind) part.bind(owner, *instance);
    return instance;
}
} // namespace

void detail::prepareCompositeTopology(WidgetContract& contract) {
    contract.compositeTopology = std::make_shared<const CompositeTopology>(makeCompositeTopology(contract));
}

const WidgetContract* findWidgetContract(const std::string& elementName) {
    const auto& contracts = builtInWidgetContracts();
    const auto widget = contracts.find(schemaNameKey(elementName));
    return widget == contracts.end() ? nullptr : &widget->second;
}

const CompositePartContract* findCompositePartContract(const WidgetContract& widget, const std::vector<std::string>& parts) {
    if (parts.empty()) return nullptr;
    std::string path;
    for (const std::string& part : parts) {
        if (!path.empty()) path += "::";
        path += part;
    }
    if (widget.compositeTopology) {
        const auto index = widget.compositeTopology->indices.find(path);
        if (index != widget.compositeTopology->indices.end() && index->second < widget.compositeParts.size())
            return &widget.compositeParts[index->second];
    }
    const auto composite = std::find_if(widget.compositeParts.begin(), widget.compositeParts.end(),
                                        [&path](const CompositePartContract& part) { return part.path == path; });
    return composite == widget.compositeParts.end() ? nullptr : &*composite;
}

bool producesState(const WidgetContract& widget, WidgetState state) {
    return std::find(widget.producedStates.begin(), widget.producedStates.end(), state) != widget.producedStates.end();
}

bool producesState(const CompositePartContract& part, WidgetState state) {
    return std::find(part.producedStates.begin(), part.producedStates.end(), state) != part.producedStates.end();
}

void detail::instantiateCompositeParts(Widget& owner, const WidgetContract& contract) {
    CompositeTopology fallback;
    const CompositeTopology& topology = *topologyFor(contract, fallback);
    if (!topology.valid) return;
    CompositeInstances instances;
    instances.reserve(contract.compositeParts.size());
    collectCompositeInstances(owner, contract, topology, instances);
    std::unordered_set<std::string> constructing;
    for (const std::size_t index : topology.order)
        if (contract.compositeParts[index].eager) instantiateCompositePartImpl(owner, contract, topology, index, instances, constructing);
}

Widget* detail::instantiateCompositePart(Widget& owner, const WidgetContract& contract, const std::string& path) {
    CompositeTopology fallback;
    const CompositeTopology& topology = *topologyFor(contract, fallback);
    if (!topology.valid) return nullptr;
    const auto part = topology.indices.find(path);
    if (part == topology.indices.end()) return nullptr;
    CompositeInstances instances;
    instances.reserve(contract.compositeParts.size());
    collectCompositeInstances(owner, contract, topology, instances);
    std::unordered_set<std::string> constructing;
    return instantiateCompositePartImpl(owner, contract, topology, part->second, instances, constructing);
}

bool readLayoutAttribute(const LayoutElement& element, const char* name, std::string& value) {
    const LayoutAttribute* attribute = element.attribute(name);
    if (!attribute) return false;
    value = attribute->value;
    return true;
}

bool readLayoutBoolean(const LayoutElement& element, const char* name, bool& value, LayoutBuildResult& result, const std::string& source) {
    std::string text;
    if (!readLayoutAttribute(element, name, text)) return false;
    if (text == "true" || text == "1") value = true;
    else if (text == "false" || text == "0") value = false;
    else {
        result.error("layout.attribute.boolean_invalid", "Invalid boolean value for " + std::string(name) + ": " + text + ".", source,
                     element.source().begin.line, element.source().begin.column);
        return false;
    }
    return true;
}

namespace {
bool readLayoutVisibility(const LayoutElement& element, Visibility& value, LayoutBuildResult& result, const std::string& source) {
    std::string text;
    if (!readLayoutAttribute(element, "visibility", text)) return false;
    if (text == "visible") value = Visibility::Visible;
    else if (text == "hidden") value = Visibility::Hidden;
    else if (text == "collapsed") value = Visibility::Collapsed;
    else {
        result.error("layout.attribute.visibility_invalid", "Invalid visibility value: " + text + ". Expected visible, hidden, or collapsed.", source,
                     element.source().begin.line, element.source().begin.column);
        return false;
    }
    return true;
}
} // namespace

TextSource localizedLayoutText(std::string value, LayoutBuildResult& result, const std::string& source, const LayoutBuildContext* context,
                               std::size_t line) {
    if (!context) return TextSource::text(std::move(value));
    if (!context->hasLocalizationKey(value)) result.error("layout.localization.missing", "Unknown localization key: " + value + ".", source, line);
    return context->localizeContent(std::move(value));
}

void validateWidgetAttributes(const LayoutElement& element, const std::vector<std::string>& widgetAttributes, LayoutBuildResult& result,
                              const std::string& source) {
    static const std::unordered_set<std::string> sCommonAttributes = {
        "id",       "class",       "visibility", "disabled",    "longClickDelay", "onClick",       "onDoubleClick",
        "onChange", "onMouseDown", "onMouseUp",  "onMouseMove", "onLongClick",    "onContextMenu", "x",
        "y",        "width",       "height",     "interactive", "blocksPointer",
    };
    std::unordered_set<std::string> allowed;
    for (const std::string& name : widgetAttributes) allowed.insert(schemaNameKey(name));
    for (const std::string& name : sCommonAttributes) allowed.insert(schemaNameKey(name));
    const WidgetContract* widgetContract = findWidgetContract(element.name());
    const std::string elementName = widgetContract ? widgetContract->elementName : schemaNameKey(element.name());
    for (const auto& [attributeName, attribute] : element.attributes())
        if (!allowed.count(attributeName))
            result.error("layout.attribute.unknown", "Unknown attribute on <" + elementName + ">: " + attribute.authoredName + ".", source,
                         attribute.source.begin.line, attribute.source.begin.column);
}

void applyCommonWidgetAttributes(const LayoutElement& element, Widget& widget, LayoutBuildResult& result, const std::string& source,
                                 const std::vector<WidgetEventKind>& supportedEvents) {
    static const char* sUnsupportedAttributes[] = {
        "x", "y", "width", "height", "interactive", "blocksPointer",
    };
    for (const char* name : sUnsupportedAttributes) {
        std::string ignored;
        if (readLayoutAttribute(element, name, ignored))
            result.error("layout.attribute.unsupported", std::string("Unsupported XML attribute: ") + name + ".", source, element.source().begin.line,
                         element.source().begin.column);
    }

    std::string value;
    if (readLayoutAttribute(element, "id", value)) {
        const LayoutAttribute* attribute = element.attribute("id");
        if (!isWidgetIdentifier(value))
            result.error("layout.id.invalid", "Widget id must be a valid identifier.", source, attribute->source.begin.line,
                         attribute->source.begin.column);
        else widget.setId(value);
    }
    if (readLayoutAttribute(element, "class", value)) {
        std::stringstream classes(value);
        while (classes >> value)
            if (!isKebabCaseIdentifier(value)) {
                const LayoutAttribute* attribute = element.attribute("class");
                result.error("layout.class.invalid", "Widget class must be a lowercase kebab-case identifier.", source, attribute->source.begin.line,
                             attribute->source.begin.column);
            } else widget.addClass(value);
    }
    Visibility visibility = Visibility::Visible;
    if (readLayoutVisibility(element, visibility, result, source)) widget.setVisibility(visibility);
    bool boolean = false;
    if (readLayoutBoolean(element, "disabled", boolean, result, source)) widget.setDisabled(boolean);

    for (WidgetEventKind kind : kAllWidgetEventKinds) {
        const char* name = eventAttribute(kind);
        if (!readLayoutAttribute(element, name, value)) continue;
        const LayoutAttribute* attribute = element.attribute(name);
        if (std::find(supportedEvents.begin(), supportedEvents.end(), kind) == supportedEvents.end()) {
            result.warning("layout.event.unsupported",
                           "Event attribute " + std::string(name) + " is not supported on <" + widget.elementName() + ">.", source,
                           attribute->source.begin.line, attribute->source.begin.column);
            continue;
        }

        EventCallParseResult parsed = parseEventCall(value);
        if (!parsed.ok()) {
            result.warning(eventCallParseErrorCode(parsed.error), eventCallParseErrorMessage(parsed.error), source, attribute->source.begin.line,
                           attribute->source.begin.column + parsed.errorOffset);
            continue;
        }
        const std::string& handlerName = parsed.call->name();
        if (handlerName == "postBuild" || handlerName == "onOpen" || handlerName == "onClose") {
            result.warning("layout.event.handler_reserved", "Controller lifecycle name cannot be used as an Event Handler: " + handlerName + ".",
                           source, attribute->source.begin.line, attribute->source.begin.column);
            continue;
        }
        widget.setEventCall(kind, std::move(*parsed.call));
    }

    if (readLayoutAttribute(element, "longClickDelay", value)) {
        const bool supportsLongClick = std::find(supportedEvents.begin(), supportedEvents.end(), WidgetEventKind::LongClick) != supportedEvents.end();
        if (supportsLongClick && !widget.eventCall(WidgetEventKind::LongClick))
            result.warning("layout.long_click.event_missing", "longClickDelay requires a valid onLongClick Event Handler Call.", source);
        else if (!supportsLongClick)
            result.warning("layout.event.unsupported", "longClickDelay is not supported on <" + widget.elementName() + ">.", source);
        else {
            const auto duration = durationValue(value);
            if (duration) widget.setLongClickDelay(*duration);
            else result.error("layout.attribute.duration_invalid", "Invalid duration for longClickDelay: " + value + ".", source);
        }
    }
}
} // namespace radia::ui
