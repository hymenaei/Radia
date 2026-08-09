/**
 * @file resourcecompiler.cpp
 * @brief Builds validated Widget trees from Layout Resource documents.
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
#include "layout/resourcecompiler.h"
#include <algorithm>
#include <cctype>
#include <functional>
#include <unordered_set>
#include "text/inlinecontentcompiler.h"
#include "widgets/button.h"
#include "widgets/floater.h"
#include "widgets/icon.h"
#include "widgets/label.h"
#include "widgets/switch.h"
#include "widgets/widgetcatalog.h"

namespace rdui {
struct LayoutResourceCompiler::BuildState {
    struct AuthoredWidget {
        Widget* widget = nullptr;
        const LayoutNode* node = nullptr;
        const LayoutNode* defaults = nullptr;
        const WidgetContract* contract = nullptr;
        std::string source;
    };

    ViewBuildResult result;
    std::vector<std::string> resources;
    std::unordered_map<std::string, const LayoutNode*> widget_defaults;
    std::unordered_map<const Widget*, AuthoredWidget> authored_widgets;
    const ViewBuildContext* context = nullptr;
};

struct LayoutResourceCompiler::ChildBuildContext {
    Widget& target;
    const LayoutElement& element;
    const WidgetContract& contract;
    const std::string& source;
    BuildState& state;
    std::string widget_text;
    bool pending_flow_break = false;
    bool has_layout_child = false;

    void markLayoutChild(Widget& child) {
        detail::WidgetCompilerAccess::setFlowBreakBefore(child, pending_flow_break);
        pending_flow_break = false;
        has_layout_child = true;
    }
};

namespace {
std::string textKey(const std::string& text) {
    const auto first = std::find_if_not(text.begin(), text.end(), [](unsigned char character) { return std::isspace(character); });
    if (first == text.end()) return {};
    const auto last = std::find_if_not(text.rbegin(), text.rend(), [](unsigned char character) { return std::isspace(character); }).base();
    return std::string(first, last);
}

std::string directoryOf(const std::string& filename) {
    const std::size_t slash = filename.find_last_of('/');
    return slash == std::string::npos ? std::string() : filename.substr(0, slash + 1);
}

std::string resourceChain(const std::vector<std::string>& resources, const std::string& filename) {
    std::string result;
    for (const std::string& resource : resources) {
        if (!result.empty()) result += " -> ";
        result += resource;
    }
    if (!result.empty()) result += " -> ";
    return result + filename;
}

bool hasAuthoredContent(const LayoutNode& node) {
    for (const LayoutContent& content : node.content)
        if (content.element || !textKey(content.text).empty()) return true;
    return false;
}

void rejectLogicalAttributes(const LayoutElement& element, ViewBuildResult& result, const std::string& source) {
    for (const auto& [name, attribute] : element.attributes())
        result.error("view.attribute.unknown", "Unknown attribute on <" + element.name() + ">: " + attribute.authored_name + ".", source,
                     attribute.source.begin.line, attribute.source.begin.column);
}
} // namespace

LayoutResourceCompiler::LayoutResourceCompiler(const LayoutDocumentMap* documents)
    : mDocuments(documents), mWidgetContracts(builtInWidgetContracts()) {}

std::string LayoutResourceCompiler::normalizeResource(std::string filename) {
    std::replace(filename.begin(), filename.end(), '\\', '/');
    while (filename.rfind("./", 0) == 0) filename.erase(0, 2);
    if (filename.rfind("xui/", 0) == 0) filename.erase(0, 4);
    if (filename.empty()) return {};

    std::vector<std::string> segments;
    std::size_t start = 0;
    while (start <= filename.size()) {
        const std::size_t slash = filename.find('/', start);
        const std::string segment = filename.substr(start, slash == std::string::npos ? std::string::npos : slash - start);
        if (segment == "..") {
            if (segments.empty()) return {};
            segments.pop_back();
        } else if (!segment.empty() && segment != ".") segments.push_back(segment);
        if (slash == std::string::npos) break;
        start = slash + 1;
    }

    std::string result;
    for (const std::string& segment : segments) {
        if (!result.empty()) result += '/';
        result += segment;
    }
    return result;
}

ViewBuildResult LayoutResourceCompiler::createFromResource(const std::string& filename, const ViewBuildContext* context) const {
    BuildState state;
    state.context = context;
    const std::string resource = normalizeResource(filename);
    std::unique_ptr<Widget> root = createResourceWidget(resource, state);
    if (root && !state.result.hasErrors()) {
        detail::WidgetCompilerAccess::setIdScopeRoot(*root);
        validateViewScope(*root, state, resource);
    }
    if (!state.result.hasErrors()) state.result.root = std::move(root);
    return std::move(state.result);
}

ViewBuildResult LayoutResourceCompiler::createFromString(const std::string& xml, const std::string& source_name,
                                                         const ViewBuildContext* context) const {
    ViewBuildResult result;
    const std::string source = normalizeResource(source_name);
    LayoutDocumentParseResult parsed = LayoutDocumentParser().parse(xml, source);
    result.warnings = std::move(parsed.warnings);
    result.errors = std::move(parsed.errors);
    std::unique_ptr<LayoutDocument> document = std::move(parsed.document);
    if (!document) return result;

    BuildState state;
    state.context = context;
    state.result = std::move(result);
    std::unique_ptr<Widget> root = buildDocument(*document, nullptr, state);
    if (root && !state.result.hasErrors()) {
        detail::WidgetCompilerAccess::setIdScopeRoot(*root);
        validateViewScope(*root, state, source);
    }
    if (!state.result.hasErrors()) state.result.root = std::move(root);
    return std::move(state.result);
}

void LayoutResourceCompiler::validateViewScope(Widget& scope, BuildState& state, const std::string& source, bool count_root) const {
    std::unordered_map<std::string, Widget*> ids;
    std::unordered_set<std::string> duplicates;
    std::vector<const BuildState::AuthoredWidget*> authored_widgets;
    std::function<void(Widget&, bool)> visit = [&](Widget& widget, bool root) {
        if (!root && widget.idScopeRoot()) {
            if (!widget.id().empty() && !ids.emplace(widget.id(), &widget).second) {
                duplicates.insert(widget.id());
                state.result.error("view.id.duplicate", "Duplicate widget id: " + widget.id() + ".", source);
            }
            validateViewScope(widget, state, source, false);
            return;
        }
        if ((!root || count_root) && !widget.id().empty() && !ids.emplace(widget.id(), &widget).second) {
            duplicates.insert(widget.id());
            state.result.error("view.id.duplicate", "Duplicate widget id: " + widget.id() + ".", source);
        }
        const auto authored = state.authored_widgets.find(&widget);
        if (authored != state.authored_widgets.end()) authored_widgets.push_back(&authored->second);
        for (const auto& child : widget.children()) visit(*child, false);
    };
    visit(scope, true);

    const ViewScopeContext context(ids, duplicates, [this](const Widget& widget) {
        const auto contract = mWidgetContracts.find(schemaNameKey(widget.element()));
        return contract != mWidgetContracts.end() && contract->second.labelable;
    });
    for (const BuildState::AuthoredWidget* authored : authored_widgets) {
        if (!authored->widget || !authored->node || !authored->contract || !authored->contract->composition_behavior.validate) continue;
        const LayoutElement element(*authored->node, authored->defaults);
        authored->contract->composition_behavior.validate(element, *authored->widget, context, state.result, authored->source);
    }
}

DiagnosticResult LayoutResourceCompiler::validateWidgetDefaults(const std::string& element, const ViewBuildContext* context) const {
    BuildState state;
    state.context = context;
    loadWidgetDefaults(element, state);
    DiagnosticResult result;
    result.warnings = std::move(state.result.warnings);
    result.errors = std::move(state.result.errors);
    return result;
}

void LayoutResourceCompiler::loadWidgetDefaults(const std::string& element, BuildState& state) const {
    const std::string lookup = schemaNameKey(element);
    if (state.widget_defaults.find(lookup) != state.widget_defaults.end()) return;
    const auto contract = mWidgetContracts.find(lookup);
    const std::string canonical = contract == mWidgetContracts.end() ? element : contract->second.element;
    const std::string default_resource = "widgets/" + canonical + ".xml";
    const LayoutNode* default_root = nullptr;
    if (contract == mWidgetContracts.end()) {
        state.result.error("view.defaults.element_unknown", "Widget Defaults target an unsupported element: " + canonical + ".", default_resource);
        state.widget_defaults.emplace(lookup, nullptr);
        return;
    }

    if (mDocuments) {
        const auto document = mDocuments->find(default_resource);
        if (document == mDocuments->end() || !document->second || !document->second->root) {
            state.widget_defaults.emplace(lookup, nullptr);
            return;
        }
        default_root = document->second->root.get();
        const std::size_t errors_before = state.result.errors.size();
        const LayoutElement defaults(*default_root);
        if (schemaNameKey(default_root->name) != lookup) {
            state.result.error("view.defaults.root_invalid", "Widget Defaults root must be <" + canonical + ">.", default_resource,
                               default_root->source.begin.line, default_root->source.begin.column);
            default_root = nullptr;
        } else if (hasAuthoredContent(*default_root)) {
            state.result.error("view.defaults.children_unsupported", "Widget Defaults may contain attributes only.", default_resource,
                               default_root->source.begin.line, default_root->source.begin.column);
            default_root = nullptr;
        } else {
            validateViewAttributes(defaults, contract->second.attributes, state.result, default_resource);
            std::string ignored;
            if (readViewAttribute(defaults, "id", ignored)
                || readViewAttribute(defaults, "filename", ignored)
                || readViewAttribute(defaults, "for", ignored)
                || readViewAttribute(defaults, "longClickDelay", ignored)) {
                state.result.error("view.defaults.controller_attribute",
                                   "Widget Defaults cannot declare IDs, relationships, includes, or controller behavior.", default_resource);
                default_root = nullptr;
            }
            for (ActionEventKind kind :
                 {ActionEventKind::Click, ActionEventKind::DoubleClick, ActionEventKind::Change, ActionEventKind::MouseDown, ActionEventKind::MouseUp,
                  ActionEventKind::MouseMove, ActionEventKind::LongClick, ActionEventKind::ContextMenu}) {
                if (default_root && readViewAttribute(defaults, actionAttribute(kind), ignored)) {
                    state.result.error("view.defaults.controller_attribute", "Widget Defaults cannot declare action handlers.", default_resource);
                    default_root = nullptr;
                    break;
                }
            }

            if (default_root) {
                std::unique_ptr<Widget> probe = contract->second.create();
                applyCommonViewAttributes(defaults, *probe, state.result, default_resource, contract->second.supported_actions);
                if (contract->second.attribute_behavior.apply)
                    contract->second.attribute_behavior.apply(defaults, *probe, state.result, default_resource, state.context);
            }
        }
        if (state.result.errors.size() != errors_before) default_root = nullptr;
    }
    state.widget_defaults.emplace(lookup, default_root);
}

std::unique_ptr<Widget> LayoutResourceCompiler::createResourceWidget(const std::string& filename, BuildState& state) const {
    const std::string resource = normalizeResource(filename);
    if (resource.empty()) {
        state.result.error("view.resource.path_invalid", "Invalid or empty resource path: " + filename + ".", filename);
        return nullptr;
    }
    if (resource.rfind("widgets/", 0) == 0) {
        state.result.error("view.resource.reserved", "Widget default resources cannot be instantiated as Views: " + resource + ".", resource);
        return nullptr;
    }
    if (std::find(state.resources.begin(), state.resources.end(), resource) != state.resources.end()) {
        state.result.error("view.resource.cycle", "Recursive panel resource reference: " + resourceChain(state.resources, resource) + ".", resource);
        return nullptr;
    }
    if (!mDocuments) {
        state.result.error("view.resource.provider_missing", "No Layout Resource collection is configured.", resource);
        return nullptr;
    }
    const auto document = mDocuments->find(resource);
    if (document == mDocuments->end() || !document->second) {
        state.result.error("view.resource.missing", "Could not load resource chain: " + resourceChain(state.resources, resource) + ".", resource);
        return nullptr;
    }

    state.resources.push_back(resource);
    std::unique_ptr<Widget> root = buildDocument(*document->second, nullptr, state);
    state.resources.pop_back();
    if (root) detail::WidgetCompilerAccess::setIdScopeRoot(*root);
    return root;
}

std::unique_ptr<Widget> LayoutResourceCompiler::buildDocument(const LayoutDocument& document, std::unique_ptr<Widget> root, BuildState& state) const {
    if (!document.root) {
        state.result.error("view.xml.invalid", "Radia UI XML must contain one root element.", document.source);
        return nullptr;
    }

    return buildNode(*document.root, document.source, std::move(root), state);
}

const WidgetContract* LayoutResourceCompiler::lookupWidgetContract(const LayoutNode& layout_node, const std::string& source,
                                                                   BuildState& state) const {
    const auto contract = mWidgetContracts.find(schemaNameKey(layout_node.name));
    if (contract == mWidgetContracts.end()) {
        state.result.error("view.element.unknown", "Unsupported XML element: " + layout_node.name + ".", source, layout_node.source.begin.line,
                           layout_node.source.begin.column);
        return nullptr;
    }
    if (contract->second.scoped_only) {
        state.result.error("view.element.scoped", "<" + contract->second.element + "> is valid only in its owning composite.", source,
                           layout_node.source.begin.line, layout_node.source.begin.column);
        return nullptr;
    }
    return &contract->second;
}

bool LayoutResourceCompiler::resolveWidgetResource(const LayoutElement& element, const WidgetContract& contract, const std::string& source,
                                                   std::unique_ptr<Widget>& node, BuildState& state) const {
    std::string filename;
    if (!readViewAttribute(element, "filename", filename)) {
        if (!node) node = contract.create();
        return true;
    }

    if (!contract.resource_root) {
        state.result.error("view.filename.unsupported", "The filename attribute is not supported on <" + contract.element + ">.", source,
                           element.source().begin.line, element.source().begin.column);
        return false;
    }

    const bool rooted = filename.rfind("xui/", 0) == 0 || (!filename.empty() && (filename.front() == '/' || filename.front() == '\\'));
    const std::string resource = normalizeResource(rooted ? filename : directoryOf(source) + filename);
    if (resource.empty()) {
        state.result.error("view.resource.path_invalid", "Invalid resource filename: " + filename + ".", source, element.source().begin.line,
                           element.source().begin.column);
        return false;
    }
    node = createResourceWidget(resource, state);
    if (!node) return false;
    if (schemaNameKey(node->element()) != schemaNameKey(contract.resource_root->expected_element)) {
        state.result.error("view.resource.root_invalid",
                           "Referenced resource must have a <" + contract.resource_root->expected_element + "> root: " + resource + ".", source,
                           element.source().begin.line, element.source().begin.column);
        return false;
    }
    return true;
}

std::unique_ptr<Widget> LayoutResourceCompiler::buildNode(const LayoutNode& layout_node, const std::string& source, std::unique_ptr<Widget> node,
                                                          BuildState& state) const {
    const WidgetContract* contract = lookupWidgetContract(layout_node, source, state);
    if (!contract) return nullptr;

    loadWidgetDefaults(contract->element, state);
    const LayoutNode* defaults = state.widget_defaults.find(schemaNameKey(layout_node.name))->second;
    const LayoutElement element(layout_node, defaults);
    if (!resolveWidgetResource(element, *contract, source, node, state)) return nullptr;

    Widget* target = node.get();
    if (!target || target->element() != contract->element) {
        state.result.error("view.builder.type_mismatch", "Registered builder does not create <" + contract->element + ">.", source,
                           element.source().begin.line, element.source().begin.column);
        return nullptr;
    }

    validateViewAttributes(element, contract->attributes, state.result, source);
    applyCommonViewAttributes(element, *target, state.result, source, contract->supported_actions);
    if (contract->attribute_behavior.apply) contract->attribute_behavior.apply(element, *target, state.result, source, state.context);
    if (contract->composition_behavior.validate)
        state.authored_widgets.emplace(target, BuildState::AuthoredWidget{target, &layout_node, defaults, contract, source});

    if (contract->content_behavior.mode == ViewTextContent::Inline) {
        if (contract->content_behavior.apply_inline_content)
            contract->content_behavior.apply_inline_content(compileInlineContent(element.content(), contract->element,
                                                                                 contract->content_behavior.accepted_inline_content, state.result,
                                                                                 source, state.context),
                                                            *target);
        return node;
    }

    buildChildren(*target, element, *contract, source, state);
    return node;
}

void LayoutResourceCompiler::buildChildren(Widget& target, const LayoutElement& element, const WidgetContract& contract, const std::string& source,
                                           BuildState& state) const {
    ChildBuildContext context{target, element, contract, source, state};
    for (const LayoutContent& content : element.content()) {
        if (content.isText()) {
            appendTextContent(content, context);
            continue;
        }

        const LayoutNode& child_node = *content.element;
        if (consumeFlowBreak(child_node, context) == ChildHandling::Handled) continue;
        if (consumeScopedInline(child_node, context) == ChildHandling::Handled) continue;
        const LayoutElement child(child_node);
        const auto part_contract = contract.children_behavior.part_attributes.find(schemaNameKey(child.name()));
        if (part_contract != contract.children_behavior.part_attributes.end())
            validateViewAttributes(child, part_contract->second, state.result, source);
        if (consumeChildContainer(child_node, context) == ChildHandling::Handled) continue;
        (void)buildRegularChild(child_node, context);
    }
    if (context.pending_flow_break)
        state.result.error("view.flow_break.trailing", "Flow Break requires a following layout child.", source, element.source().end.line,
                           element.source().end.column);
    if (contract.content_behavior.mode == ViewTextContent::Widget && !context.widget_text.empty() && contract.content_behavior.apply_text)
        contract.content_behavior.apply_text(std::move(context.widget_text), target, state.result, source, state.context,
                                             element.source().begin.line);
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::appendTextContent(const LayoutContent& content, ChildBuildContext& context) const {
    std::string value = textKey(content.text);
    if (value.empty()) return ChildHandling::Handled;
    if (context.contract.content_behavior.mode == ViewTextContent::Unsupported) {
        context.state.result.error("view.text.unsupported", "Text content is not supported in <" + context.contract.element + ">.", context.source,
                                   content.source.begin.line, content.source.begin.column);
    } else if (context.contract.content_behavior.mode == ViewTextContent::Widget) {
        if (!context.widget_text.empty()) context.widget_text += value;
        else context.widget_text = std::move(value);
    } else {
        TextSource text = localizedViewText(std::move(value), context.state.result, context.source, context.state.context, content.source.begin.line);
        if (context.contract.content_behavior.create_text_child) {
            if (auto child = context.contract.content_behavior.create_text_child(std::move(text))) {
                context.markLayoutChild(*child);
                context.target.addChild(std::move(child));
            }
        } else {
            auto label = std::make_unique<Label>();
            label->setContent(std::move(text));
            context.markLayoutChild(*label);
            context.target.addChild(std::move(label));
        }
    }
    return ChildHandling::Handled;
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::consumeFlowBreak(const LayoutNode& child_node, ChildBuildContext& context) const {
    const LayoutElement child(child_node);
    if (schemaNameKey(child.name()) != schemaNameKey("br")) return ChildHandling::Unhandled;
    rejectLogicalAttributes(child, context.state.result, context.source);
    if (hasAuthoredContent(child_node))
        context.state.result.error("view.flow_break.children_unsupported", "Flow Break <br> cannot contain content.", context.source,
                                   child_node.source.begin.line, child_node.source.begin.column);
    if (!context.has_layout_child)
        context.state.result.error("view.flow_break.leading", "Flow Break requires a preceding layout child.", context.source,
                                   child_node.source.begin.line, child_node.source.begin.column);
    else if (context.pending_flow_break)
        context.state.result.error("view.flow_break.consecutive", "Consecutive Flow Break directives are not supported.", context.source,
                                   child_node.source.begin.line, child_node.source.begin.column);
    else context.pending_flow_break = true;
    return ChildHandling::Handled;
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::consumeScopedInline(const LayoutNode& child_node, ChildBuildContext& context) const {
    const LayoutElement child(child_node);
    const auto scoped_inline = context.contract.content_behavior.scoped_inline_content.find(schemaNameKey(child.name()));
    if (scoped_inline == context.contract.content_behavior.scoped_inline_content.end()) return ChildHandling::Unhandled;
    rejectLogicalAttributes(child, context.state.result, context.source);
    Widget* scoped_part =
        scoped_inline->second.apply(compileInlineContent(child.content(), scoped_inline->second.element, scoped_inline->second.accepted,
                                                         context.state.result, context.source, context.state.context),
                                    context.target, context.state.result, context.source, child.source().begin.line, child.source().begin.column);
    if (scoped_part) context.markLayoutChild(*scoped_part);
    return ChildHandling::Handled;
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::consumeChildContainer(const LayoutNode& child_node, ChildBuildContext& context) const {
    if (!context.contract.children_behavior.claim) return ChildHandling::Unhandled;
    const LayoutElement child(child_node);
    const ChildClaim claim = context.contract.children_behavior.claim(child, context.target, context.state.result, context.source);
    if (claim.kind() == ChildClaim::Kind::NotHandled) return ChildHandling::Unhandled;
    if (Widget* container = claim.container()) {
        for (const LayoutContent& nested_content : child.content()) {
            if (nested_content.isText()) {
                if (!textKey(nested_content.text).empty())
                    context.state.result.error("view.text.unsupported", "Text content is not supported in <" + child.name() + ">.", context.source,
                                               nested_content.source.begin.line, nested_content.source.begin.column);
                continue;
            }
            if (auto child_widget = buildNode(*nested_content.element, context.source, nullptr, context.state))
                container->addChild(std::move(child_widget));
        }
    }
    return ChildHandling::Handled;
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::buildRegularChild(const LayoutNode& child_node, ChildBuildContext& context) const {
    if (auto child_widget = buildNode(child_node, context.source, nullptr, context.state)) {
        context.markLayoutChild(*child_widget);
        context.target.addChild(std::move(child_widget));
    }
    return ChildHandling::Handled;
}
} // namespace rdui
