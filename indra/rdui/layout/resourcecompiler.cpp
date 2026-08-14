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

    LayoutBuildResult result;
    std::vector<std::string> resourceStack;
    std::unordered_map<std::string, const LayoutNode*> widgetDefaults;
    std::unordered_map<const Widget*, AuthoredWidget> authoredWidgetRecords;
    const LayoutBuildContext* context = nullptr;
};

struct LayoutResourceCompiler::ChildBuildContext {
    Widget& target;
    const LayoutElement& element;
    const WidgetContract& contract;
    const std::string& source;
    BuildState& state;
    std::string widgetText;
    bool pendingFlowBreak = false;
    bool hasLayoutChild = false;

    void markLayoutChild(Widget& child) {
        detail::WidgetCompilerAccess::setFlowBreakBefore(child, pendingFlowBreak);
        pendingFlowBreak = false;
        hasLayoutChild = true;
    }
};

namespace {
std::string trimmedText(const std::string& text) {
    const auto first = std::find_if_not(text.begin(), text.end(), [](unsigned char character) { return std::isspace(character); });
    if (first == text.end()) return {};
    const auto last = std::find_if_not(text.rbegin(), text.rend(), [](unsigned char character) { return std::isspace(character); }).base();
    return std::string(first, last);
}

std::string directoryOf(const std::string& sourcePath) {
    const std::size_t slash = sourcePath.find_last_of('/');
    return slash == std::string::npos ? std::string() : sourcePath.substr(0, slash + 1);
}

std::string resourceChain(const std::vector<std::string>& resourceStack, const std::string& resourceId) {
    std::string result;
    for (const std::string& resource : resourceStack) {
        if (!result.empty()) result += " -> ";
        result += resource;
    }
    if (!result.empty()) result += " -> ";
    return result + resourceId;
}

bool hasAuthoredContent(const LayoutNode& node) {
    for (const LayoutContent& content : node.content)
        if (content.node || !trimmedText(content.text).empty()) return true;
    return false;
}

void rejectAuthoredAttributes(const LayoutElement& element, LayoutBuildResult& result, const std::string& source) {
    for (const auto& [name, attribute] : element.attributes())
        result.error("layout.attribute.unknown", "Unknown attribute on <" + element.name() + ">: " + attribute.authoredName + ".", source,
                     attribute.source.begin.line, attribute.source.begin.column);
}
} // namespace

LayoutResourceCompiler::LayoutResourceCompiler(const LayoutDocumentMap* documents)
    : mDocuments(documents), mWidgetContracts(builtInWidgetContracts()) {}

std::string LayoutResourceCompiler::normalizeResource(std::string resourceId) {
    std::replace(resourceId.begin(), resourceId.end(), '\\', '/');
    while (resourceId.rfind("./", 0) == 0) resourceId.erase(0, 2);
    if (resourceId.rfind("xui/", 0) == 0) resourceId.erase(0, 4);
    if (resourceId.empty()) return {};

    std::vector<std::string> segments;
    std::size_t start = 0;
    while (start <= resourceId.size()) {
        const std::size_t slash = resourceId.find('/', start);
        const std::string segment = resourceId.substr(start, slash == std::string::npos ? std::string::npos : slash - start);
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

LayoutBuildResult LayoutResourceCompiler::buildWidgetTreeFromResource(const std::string& resourceId, const LayoutBuildContext* context) const {
    BuildState state;
    state.context = context;
    const std::string resource = normalizeResource(resourceId);
    std::unique_ptr<Widget> root = createResourceWidget(resource, state);
    if (root && !state.result.hasErrors()) {
        detail::WidgetCompilerAccess::setIdScopeRoot(*root);
        validateWidgetScope(*root, state, resource);
    }
    if (!state.result.hasErrors()) state.result.root = std::move(root);
    return std::move(state.result);
}

LayoutBuildResult LayoutResourceCompiler::buildWidgetTreeFromString(const std::string& xml, const std::string& sourceName,
                                                                    const LayoutBuildContext* context) const {
    LayoutBuildResult result;
    const std::string source = normalizeResource(sourceName);
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
        validateWidgetScope(*root, state, source);
    }
    if (!state.result.hasErrors()) state.result.root = std::move(root);
    return std::move(state.result);
}

void LayoutResourceCompiler::validateWidgetScope(Widget& scope, BuildState& state, const std::string& source, bool countRoot) const {
    std::unordered_map<std::string, Widget*> ids;
    std::unordered_set<std::string> duplicates;
    std::vector<const BuildState::AuthoredWidget*> authoredWidgetRecords;
    std::function<void(Widget&, bool)> visit = [&](Widget& widget, bool root) {
        if (!root && widget.idScopeRoot()) {
            if (!widget.id().empty() && !ids.emplace(widget.id(), &widget).second) {
                duplicates.insert(widget.id());
                state.result.error("layout.id.duplicate", "Duplicate widget id: " + widget.id() + ".", source);
            }
            validateWidgetScope(widget, state, source, false);
            return;
        }
        if ((!root || countRoot) && !widget.id().empty() && !ids.emplace(widget.id(), &widget).second) {
            duplicates.insert(widget.id());
            state.result.error("layout.id.duplicate", "Duplicate widget id: " + widget.id() + ".", source);
        }
        const auto authored = state.authoredWidgetRecords.find(&widget);
        if (authored != state.authoredWidgetRecords.end()) authoredWidgetRecords.push_back(&authored->second);
        for (const auto& child : widget.children()) visit(*child, false);
    };
    visit(scope, true);

    const WidgetScopeContext context(ids, duplicates, [this](const Widget& widget) {
        const auto contract = mWidgetContracts.find(schemaNameKey(widget.elementName()));
        return contract != mWidgetContracts.end() && contract->second.labelable;
    });
    for (const BuildState::AuthoredWidget* authored : authoredWidgetRecords) {
        if (!authored->widget || !authored->node || !authored->contract || !authored->contract->compositionBehavior.validate) continue;
        const LayoutElement element(*authored->node, authored->defaults);
        authored->contract->compositionBehavior.validate(element, *authored->widget, context, state.result, authored->source);
    }
}

DiagnosticResult LayoutResourceCompiler::validateWidgetDefaults(const std::string& element, const LayoutBuildContext* context) const {
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
    if (state.widgetDefaults.find(lookup) != state.widgetDefaults.end()) return;
    const auto contract = mWidgetContracts.find(lookup);
    const std::string canonical = contract == mWidgetContracts.end() ? element : contract->second.elementName;
    const std::string defaultResource = "widgets/" + canonical + ".xml";
    const LayoutNode* defaultRoot = nullptr;
    if (contract == mWidgetContracts.end()) {
        state.result.error("layout.defaults.element_unknown", "Widget Defaults target an unsupported element: " + canonical + ".", defaultResource);
        state.widgetDefaults.emplace(lookup, nullptr);
        return;
    }

    if (mDocuments) {
        const auto document = mDocuments->find(defaultResource);
        if (document == mDocuments->end() || !document->second || !document->second->root) {
            state.widgetDefaults.emplace(lookup, nullptr);
            return;
        }
        defaultRoot = document->second->root.get();
        const std::size_t errorsBefore = state.result.errors.size();
        const LayoutElement defaults(*defaultRoot);
        if (schemaNameKey(defaultRoot->name) != lookup) {
            state.result.error("layout.defaults.root_invalid", "Widget Defaults root must be <" + canonical + ">.", defaultResource,
                               defaultRoot->source.begin.line, defaultRoot->source.begin.column);
            defaultRoot = nullptr;
        } else if (hasAuthoredContent(*defaultRoot)) {
            state.result.error("layout.defaults.children_unsupported", "Widget Defaults may contain attributes only.", defaultResource,
                               defaultRoot->source.begin.line, defaultRoot->source.begin.column);
            defaultRoot = nullptr;
        } else {
            validateWidgetAttributes(defaults, contract->second.attributes, state.result, defaultResource);
            std::string ignored;
            if (readLayoutAttribute(defaults, "id", ignored)
                || readLayoutAttribute(defaults, "filename", ignored)
                || readLayoutAttribute(defaults, "for", ignored)
                || readLayoutAttribute(defaults, "longClickDelay", ignored)) {
                state.result.error("layout.defaults.controller_attribute",
                                   "Widget Defaults cannot declare IDs, relationships, includes, or controller behavior.", defaultResource);
                defaultRoot = nullptr;
            }
            for (WidgetEventKind kind : kAllWidgetEventKinds) {
                if (defaultRoot && readLayoutAttribute(defaults, eventAttribute(kind), ignored)) {
                    state.result.error("layout.defaults.controller_attribute", "Widget Defaults cannot declare Event Handler Calls.",
                                       defaultResource);
                    defaultRoot = nullptr;
                    break;
                }
            }

            if (defaultRoot) {
                std::unique_ptr<Widget> probe = contract->second.create();
                applyCommonWidgetAttributes(defaults, *probe, state.result, defaultResource, contract->second.supportedEvents);
                if (contract->second.attributeBehavior.apply)
                    contract->second.attributeBehavior.apply(defaults, *probe, state.result, defaultResource, state.context);
            }
        }
        if (state.result.errors.size() != errorsBefore) defaultRoot = nullptr;
    }
    state.widgetDefaults.emplace(lookup, defaultRoot);
}

std::unique_ptr<Widget> LayoutResourceCompiler::createResourceWidget(const std::string& resourceId, BuildState& state) const {
    const std::string resource = normalizeResource(resourceId);
    if (resource.empty()) {
        state.result.error("layout.resource.path_invalid", "Invalid or empty resource path: " + resourceId + ".", resourceId);
        return nullptr;
    }
    if (resource.rfind("widgets/", 0) == 0) {
        state.result.error("layout.resource.reserved", "Widget default resources cannot be instantiated as Layout Resources: " + resource + ".",
                           resource);
        return nullptr;
    }
    if (std::find(state.resourceStack.begin(), state.resourceStack.end(), resource) != state.resourceStack.end()) {
        state.result.error("layout.resource.cycle", "Recursive layout resource reference: " + resourceChain(state.resourceStack, resource) + ".",
                           resource);
        return nullptr;
    }
    if (!mDocuments) {
        state.result.error("layout.resource.provider_missing", "No Layout Resource collection is configured.", resource);
        return nullptr;
    }
    const auto document = mDocuments->find(resource);
    if (document == mDocuments->end() || !document->second) {
        state.result.error("layout.resource.missing", "Could not load resource chain: " + resourceChain(state.resourceStack, resource) + ".",
                           resource);
        return nullptr;
    }

    state.resourceStack.push_back(resource);
    std::unique_ptr<Widget> root = buildDocument(*document->second, nullptr, state);
    state.resourceStack.pop_back();
    if (root) detail::WidgetCompilerAccess::setIdScopeRoot(*root);
    return root;
}

std::unique_ptr<Widget> LayoutResourceCompiler::buildDocument(const LayoutDocument& document, std::unique_ptr<Widget> root, BuildState& state) const {
    if (!document.root) {
        state.result.error("layout.xml.invalid", "Radia UI XML must contain one root element.", document.source);
        return nullptr;
    }

    return buildNode(*document.root, document.source, std::move(root), state);
}

const WidgetContract* LayoutResourceCompiler::lookupWidgetContract(const LayoutNode& layoutNode, const std::string& source, BuildState& state) const {
    const auto contract = mWidgetContracts.find(schemaNameKey(layoutNode.name));
    if (contract == mWidgetContracts.end()) {
        state.result.error("layout.element.unknown", "Unsupported XML element: " + layoutNode.name + ".", source, layoutNode.source.begin.line,
                           layoutNode.source.begin.column);
        return nullptr;
    }
    if (contract->second.scopedOnly) {
        state.result.error("layout.element.scoped", "<" + contract->second.elementName + "> is valid only in its owning composite.", source,
                           layoutNode.source.begin.line, layoutNode.source.begin.column);
        return nullptr;
    }
    return &contract->second;
}

bool LayoutResourceCompiler::resolveWidgetResource(const LayoutElement& element, const WidgetContract& contract, const std::string& source,
                                                   std::unique_ptr<Widget>& widget, BuildState& state) const {
    std::string filename;
    if (!readLayoutAttribute(element, "filename", filename)) {
        if (!widget) widget = contract.create();
        return true;
    }

    if (!contract.resourceRoot) {
        state.result.error("layout.filename.unsupported", "The filename attribute is not supported on <" + contract.elementName + ">.", source,
                           element.source().begin.line, element.source().begin.column);
        return false;
    }

    const bool rooted = filename.rfind("xui/", 0) == 0 || (!filename.empty() && (filename.front() == '/' || filename.front() == '\\'));
    const std::string resource = normalizeResource(rooted ? filename : directoryOf(source) + filename);
    if (resource.empty()) {
        state.result.error("layout.resource.path_invalid", "Invalid resource filename: " + filename + ".", source, element.source().begin.line,
                           element.source().begin.column);
        return false;
    }
    widget = createResourceWidget(resource, state);
    if (!widget) return false;
    if (schemaNameKey(widget->elementName()) != schemaNameKey(contract.resourceRoot->expectedElementName)) {
        state.result.error("layout.resource.root_invalid",
                           "Referenced resource must have a <" + contract.resourceRoot->expectedElementName + "> root: " + resource + ".", source,
                           element.source().begin.line, element.source().begin.column);
        return false;
    }
    return true;
}

std::unique_ptr<Widget> LayoutResourceCompiler::buildNode(const LayoutNode& layoutNode, const std::string& source, std::unique_ptr<Widget> widget,
                                                          BuildState& state) const {
    const WidgetContract* contract = lookupWidgetContract(layoutNode, source, state);
    if (!contract) return nullptr;

    loadWidgetDefaults(contract->elementName, state);
    const LayoutNode* defaults = state.widgetDefaults.find(schemaNameKey(layoutNode.name))->second;
    const LayoutElement element(layoutNode, defaults);
    if (!resolveWidgetResource(element, *contract, source, widget, state)) return nullptr;

    Widget* target = widget.get();
    if (!target || target->elementName() != contract->elementName) {
        state.result.error("layout.builder.type_mismatch", "Registered builder does not create <" + contract->elementName + ">.", source,
                           element.source().begin.line, element.source().begin.column);
        return nullptr;
    }

    validateWidgetAttributes(element, contract->attributes, state.result, source);
    applyCommonWidgetAttributes(element, *target, state.result, source, contract->supportedEvents);
    if (contract->attributeBehavior.apply) contract->attributeBehavior.apply(element, *target, state.result, source, state.context);
    if (contract->compositionBehavior.validate)
        state.authoredWidgetRecords.emplace(target, BuildState::AuthoredWidget{target, &layoutNode, defaults, contract, source});

    if (contract->contentBehavior.mode == WidgetTextContentMode::InlineContent) {
        if (contract->contentBehavior.applyInlineContent)
            contract->contentBehavior.applyInlineContent(compileInlineContent(element.content(), contract->elementName,
                                                                              contract->contentBehavior.acceptedInlineContent, state.result, source,
                                                                              state.context),
                                                         *target);
        return widget;
    }

    buildChildren(*target, element, *contract, source, state);
    return widget;
}

void LayoutResourceCompiler::buildChildren(Widget& target, const LayoutElement& element, const WidgetContract& contract, const std::string& source,
                                           BuildState& state) const {
    ChildBuildContext context{target, element, contract, source, state};
    for (const LayoutContent& content : element.content()) {
        if (content.isText()) {
            appendTextContent(content, context);
            continue;
        }

        const LayoutNode& childNode = *content.node;
        if (consumeFlowBreak(childNode, context) == ChildHandling::Handled) continue;
        if (consumeScopedInline(childNode, context) == ChildHandling::Handled) continue;
        const LayoutElement child(childNode);
        const auto part_contract = contract.childrenBehavior.partAttributes.find(schemaNameKey(child.name()));
        if (part_contract != contract.childrenBehavior.partAttributes.end())
            validateWidgetAttributes(child, part_contract->second, state.result, source);
        if (consumeChildContainer(childNode, context) == ChildHandling::Handled) continue;
        (void)buildRegularChild(childNode, context);
    }
    if (context.pendingFlowBreak)
        state.result.error("layout.flow_break.trailing", "Flow Break requires a following layout child.", source, element.source().end.line,
                           element.source().end.column);
    if (contract.contentBehavior.mode == WidgetTextContentMode::WidgetText && !context.widgetText.empty() && contract.contentBehavior.applyText)
        contract.contentBehavior.applyText(std::move(context.widgetText), target, state.result, source, state.context, element.source().begin.line);
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::appendTextContent(const LayoutContent& content, ChildBuildContext& context) const {
    std::string value = trimmedText(content.text);
    if (value.empty()) return ChildHandling::Handled;
    if (context.contract.contentBehavior.mode == WidgetTextContentMode::Unsupported) {
        context.state.result.error("layout.text.unsupported", "Text content is not supported in <" + context.contract.elementName + ">.",
                                   context.source, content.source.begin.line, content.source.begin.column);
    } else if (context.contract.contentBehavior.mode == WidgetTextContentMode::WidgetText) {
        if (!context.widgetText.empty()) context.widgetText += value;
        else context.widgetText = std::move(value);
    } else {
        TextSource text =
            localizedLayoutText(std::move(value), context.state.result, context.source, context.state.context, content.source.begin.line);
        if (context.contract.contentBehavior.createTextChild) {
            if (auto child = context.contract.contentBehavior.createTextChild(std::move(text))) {
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

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::consumeFlowBreak(const LayoutNode& childNode, ChildBuildContext& context) const {
    const LayoutElement child(childNode);
    if (schemaNameKey(child.name()) != schemaNameKey("br")) return ChildHandling::Unhandled;
    rejectAuthoredAttributes(child, context.state.result, context.source);
    if (hasAuthoredContent(childNode))
        context.state.result.error("layout.flow_break.children_unsupported", "Flow Break <br> cannot contain content.", context.source,
                                   childNode.source.begin.line, childNode.source.begin.column);
    if (!context.hasLayoutChild)
        context.state.result.error("layout.flow_break.leading", "Flow Break requires a preceding layout child.", context.source,
                                   childNode.source.begin.line, childNode.source.begin.column);
    else if (context.pendingFlowBreak)
        context.state.result.error("layout.flow_break.consecutive", "Consecutive Flow Break directives are not supported.", context.source,
                                   childNode.source.begin.line, childNode.source.begin.column);
    else context.pendingFlowBreak = true;
    return ChildHandling::Handled;
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::consumeScopedInline(const LayoutNode& childNode, ChildBuildContext& context) const {
    const LayoutElement child(childNode);
    const auto scoped_inline = context.contract.contentBehavior.scopedInlineContent.find(schemaNameKey(child.name()));
    if (scoped_inline == context.contract.contentBehavior.scopedInlineContent.end()) return ChildHandling::Unhandled;
    rejectAuthoredAttributes(child, context.state.result, context.source);
    Widget* scoped_part =
        scoped_inline->second.apply(compileInlineContent(child.content(), scoped_inline->second.elementName, scoped_inline->second.accepted,
                                                         context.state.result, context.source, context.state.context),
                                    context.target, context.state.result, context.source, child.source().begin.line, child.source().begin.column);
    if (scoped_part) context.markLayoutChild(*scoped_part);
    return ChildHandling::Handled;
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::consumeChildContainer(const LayoutNode& childNode, ChildBuildContext& context) const {
    if (!context.contract.childrenBehavior.claim) return ChildHandling::Unhandled;
    const LayoutElement child(childNode);
    const ChildClaim claim = context.contract.childrenBehavior.claim(child, context.target, context.state.result, context.source);
    if (claim.kind() == ChildClaim::Kind::NotHandled) return ChildHandling::Unhandled;
    if (Widget* container = claim.container()) {
        for (const LayoutContent& nested_content : child.content()) {
            if (nested_content.isText()) {
                if (!trimmedText(nested_content.text).empty())
                    context.state.result.error("layout.text.unsupported", "Text content is not supported in <" + child.name() + ">.", context.source,
                                               nested_content.source.begin.line, nested_content.source.begin.column);
                continue;
            }
            if (auto child_widget = buildNode(*nested_content.node, context.source, nullptr, context.state))
                container->addChild(std::move(child_widget));
        }
    }
    return ChildHandling::Handled;
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::buildRegularChild(const LayoutNode& childNode, ChildBuildContext& context) const {
    if (auto child_widget = buildNode(childNode, context.source, nullptr, context.state)) {
        context.markLayoutChild(*child_widget);
        context.target.addChild(std::move(child_widget));
    }
    return ChildHandling::Handled;
}
} // namespace rdui
