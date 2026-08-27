/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "layout/resourcecompiler.h"
#include <algorithm>
#include <cctype>
#include <functional>
#include <memory>
#include <unordered_set>
#include "elements/button.h"
#include "elements/elementdefinition.h"
#include "elements/elementevent.h"
#include "elements/elementinternal.h"
#include "elements/floater.h"
#include "elements/icon.h"
#include "elements/label.h"
#include "layout/document.h"
#include "resourceprovider.h"
#include "text/inlineelements.h"

namespace radia::ui {
struct LayoutResourceCompiler::BuildState {
    struct AuthoredElement {
        Element* element = nullptr;
        ElementBuildInput input;
        const ElementDefinition* definition = nullptr;
    };

    LayoutBuildResult result;
    std::vector<std::string> resourceStack;
    std::unordered_map<std::string, std::shared_ptr<const SourceDocument>> documents;
    std::unordered_set<std::string> invalidDocuments;
    std::unordered_map<std::string, const SourceNode*> elementDefaults;
    std::unordered_map<const Element*, AuthoredElement> authoredElementRecords;
    const LayoutBuildContext* context = nullptr;
};

struct LayoutResourceCompiler::ChildBuildContext {
    Element& target;
    const ElementDefinition& definition;
    const std::string& source;
    BuildState& state;
    std::string elementText;
    bool pendingFlowBreak = false;
    bool hasLayoutChild = false;

    void markLayoutChild(Node& child) {
        detail::NodeAccess::setFlowBreakBefore(child, pendingFlowBreak);
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

bool hasAuthoredContent(const SourceNode& node) {
    for (const SourceContent& content : node.content)
        if (content.node || !trimmedText(content.text).empty()) return true;
    return false;
}

ElementBuildInput makeElementInput(const SourceNode& node, const SourceNode* defaults, const std::string& source) {
    ElementBuildInput input;
    input.tag = node.tag;
    input.authoredName = node.authoredName;
    input.source = node.source;
    input.sourceName = source;
    input.attributes.reserve(node.attributes.size() + (defaults ? defaults->attributes.size() : 0));
    for (const auto& [name, attribute] : node.attributes)
        input.attributes.emplace(name, ElementAttribute{attribute.authoredName, attribute.value, attribute.source});
    if (defaults)
        for (const auto& [name, attribute] : defaults->attributes)
            input.attributes.try_emplace(name, ElementAttribute{attribute.authoredName, attribute.value, attribute.source});
    return input;
}

void rejectAuthoredAttributes(const SourceNode& node, LayoutBuildResult& result, const std::string& source) {
    for (const auto& [name, attribute] : node.attributes)
        result.error("layout.attribute.unknown",
                     "Unknown attribute on <" + std::string(sourceTagName(node.tag)) + ">: " + attribute.authoredName + ".", source,
                     attribute.source.begin.line, attribute.source.begin.column);
}

bool validateDefaultNode(const SourceNode& node, LayoutBuildResult& result, const std::string& source, const LayoutBuildContext* context) {
    std::string inputType;
    if (node.tag == Tag::Input) {
        const auto type = node.attributes.find("type");
        if (type != node.attributes.end()) inputType = type->second.value;
    }
    const ElementDefinition* definition = findElementDefinition(node.tag, inputType);
    if (!definition) {
        result.error("layout.defaults.element_unknown", "Element Defaults contain an unsupported element: " + node.authoredName + ".", source,
                     node.source.begin.line, node.source.begin.column);
        return false;
    }

    const std::size_t errorsBefore = result.errors.size();
    const ElementBuildInput input = makeElementInput(node, nullptr, source);
    validateElementAttributes(input, definition->attributes, result);

    std::string ignored;
    if (readElementAttribute(input, "id", ignored)
        || readElementAttribute(input, "filename", ignored)
        || readElementAttribute(input, "for", ignored)) {
        result.error("layout.defaults.controller_attribute",
                     "Element Defaults cannot declare IDs, relationships, includes, or controller behavior.", source,
                     node.source.begin.line, node.source.begin.column);
    }
    for (const AuthoredEventDescriptor& descriptor : kAuthoredEventDescriptors) {
        if (readElementAttribute(input, descriptor.attribute, ignored)) {
            result.error("layout.defaults.controller_attribute", "Element Defaults cannot declare Event Handler Calls.", source,
                         node.source.begin.line, node.source.begin.column);
            break;
        }
    }

    std::unique_ptr<Element> probe = definition->create();
    if (probe) {
        applyCommonElementAttributes(input, *probe, result);
        if (definition->attributeBehavior.apply) definition->attributeBehavior.apply(input, *probe, result, context);
    }

    for (const SourceContent& content : node.content)
        if (content.node) validateDefaultNode(*content.node, result, source, context);
    return result.errors.size() == errorsBefore;
}
} // namespace

LayoutResourceCompiler::LayoutResourceCompiler(const ResourceProvider* resources) : mResources(resources) {}

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

LayoutBuildResult LayoutResourceCompiler::buildElementTreeFromResource(const std::string& resourceId, const LayoutBuildContext* context) const {
    BuildState state;
    state.context = context;
    const std::string resource = normalizeResource(resourceId);
    std::unique_ptr<Element> root = createResourceElement(resource, state);
    if (root && !state.result.hasErrors()) {
        detail::ElementCompilerAccess::setIdScopeRoot(*root);
        validateElementScope(*root, state, resource);
    }
    if (!state.result.hasErrors() && root) state.result.document = std::make_unique<Document>(std::move(root));
    return std::move(state.result);
}

LayoutBuildResult LayoutResourceCompiler::buildElementTreeFromString(const std::string& xml, const std::string& sourceName,
                                                                     const LayoutBuildContext* context) const {
    LayoutBuildResult result;
    const std::string source = normalizeResource(sourceName);
    SourceDocumentParseResult parsed = SourceDocumentParser().parse(xml, source);
    result.warnings = std::move(parsed.warnings);
    result.errors = std::move(parsed.errors);
    std::unique_ptr<SourceDocument> document = std::move(parsed.document);
    if (!document || result.hasErrors()) return result;

    BuildState state;
    state.context = context;
    state.result = std::move(result);
    std::unique_ptr<Element> root = buildDocument(*document, nullptr, state);
    if (root && !state.result.hasErrors()) {
        detail::ElementCompilerAccess::setIdScopeRoot(*root);
        validateElementScope(*root, state, source);
    }
    if (!state.result.hasErrors() && root) state.result.document = std::make_unique<Document>(std::move(root));
    return std::move(state.result);
}

void LayoutResourceCompiler::validateElementScope(Element& scope, BuildState& state, const std::string& source, bool includeRootInIdScope) const {
    std::unordered_map<std::string, Element*> ids;
    std::unordered_set<std::string> duplicates;
    std::vector<const BuildState::AuthoredElement*> authoredElementRecords;
    std::function<void(Element&, bool)> visit = [&](Element& element, bool root) {
        if (!root && element.idScopeRoot()) {
            if (!element.id().empty() && !ids.emplace(element.id(), &element).second) {
                duplicates.insert(element.id());
                state.result.error("layout.id.duplicate", "Duplicate element id: " + element.id() + ".", source);
            }
            validateElementScope(element, state, source, false);
            return;
        }
        if ((!root || includeRootInIdScope) && !element.id().empty() && !ids.emplace(element.id(), &element).second) {
            duplicates.insert(element.id());
            state.result.error("layout.id.duplicate", "Duplicate element id: " + element.id() + ".", source);
        }
        const auto authored = state.authoredElementRecords.find(&element);
        if (authored != state.authoredElementRecords.end()) authoredElementRecords.push_back(&authored->second);
        for (Element* child : element.children()) visit(*child, false);
    };
    visit(scope, true);

    const ElementScopeContext context(ids, duplicates, [](const Element& element) {
        const ElementDefinition* definition = findElementDefinition(sourceTagFromName(element.elementName()));
        return definition && definition->labelable;
    });
    for (const BuildState::AuthoredElement* authored : authoredElementRecords) {
        if (!authored->element || !authored->definition || !authored->definition->compositionBehavior.validate) continue;
        authored->definition->compositionBehavior.validate(authored->input, *authored->element, context, state.result);
    }
}

DiagnosticResult LayoutResourceCompiler::validateElementDefaults(const std::string& element, const LayoutBuildContext* context) const {
    BuildState state;
    state.context = context;
    loadElementDefaults(element, state);
    DiagnosticResult result;
    result.warnings = std::move(state.result.warnings);
    result.errors = std::move(state.result.errors);
    return result;
}

void LayoutResourceCompiler::loadElementDefaults(const std::string& element, BuildState& state) const {
    const std::string lookup = schemaNameKey(element);
    if (state.elementDefaults.find(lookup) != state.elementDefaults.end()) return;
    const ElementDefinition* definition = findElementDefinition(sourceTagFromName(element));
    const std::string canonical = definition ? definition->elementName : element;
    const std::string defaultResource = "elements/" + canonical + ".html";
    const SourceNode* defaultRoot = nullptr;
    if (!definition) {
        state.result.error("layout.defaults.element_unknown", "Element Defaults target an unsupported element: " + canonical + ".", defaultResource);
        state.elementDefaults.emplace(lookup, nullptr);
        return;
    }

    if (mResources) {
        const SourceDocument* document = loadDocument(defaultResource, state, false);
        if (!document || !document->root) {
            state.elementDefaults.emplace(lookup, nullptr);
            return;
        }
        defaultRoot = document->root.get();
        const std::size_t errorsBefore = state.result.errors.size();
        if (schemaNameKey(sourceTagName(defaultRoot->tag)) != lookup) {
            state.result.error("layout.defaults.root_invalid", "Element Defaults root must be <" + canonical + ">.", defaultResource,
                               defaultRoot->source.begin.line, defaultRoot->source.begin.column);
            defaultRoot = nullptr;
        } else {
            if (!validateDefaultNode(*defaultRoot, state.result, defaultResource, state.context)) defaultRoot = nullptr;
        }
        if (state.result.errors.size() != errorsBefore) defaultRoot = nullptr;
    }
    state.elementDefaults.emplace(lookup, defaultRoot);
}

std::unique_ptr<Element> LayoutResourceCompiler::createResourceElement(const std::string& resourceId, BuildState& state) const {
    const std::string resource = normalizeResource(resourceId);
    if (resource.empty()) {
        state.result.error("layout.resource.path_invalid", "Invalid or empty resource path: " + resourceId + ".", resourceId);
        return nullptr;
    }
    if (resource.rfind("elements/", 0) == 0) {
        state.result.error("layout.resource.reserved", "Element default resources cannot be instantiated as Layout Resources: " + resource + ".",
                           resource);
        return nullptr;
    }
    if (std::find(state.resourceStack.begin(), state.resourceStack.end(), resource) != state.resourceStack.end()) {
        state.result.error("layout.resource.cycle", "Recursive layout resource reference: " + resourceChain(state.resourceStack, resource) + ".",
                           resource);
        return nullptr;
    }
    const SourceDocument* document = loadDocument(resource, state, true);
    if (!document) return nullptr;

    state.resourceStack.push_back(resource);
    std::unique_ptr<Element> root = buildDocument(*document, nullptr, state);
    state.resourceStack.pop_back();
    if (root) detail::ElementCompilerAccess::setIdScopeRoot(*root);
    return root;
}

const SourceDocument* LayoutResourceCompiler::loadDocument(const std::string& resourceId, BuildState& state, bool required) const {
    const auto cached = state.documents.find(resourceId);
    if (cached != state.documents.end()) return cached->second.get();
    if (state.invalidDocuments.find(resourceId) != state.invalidDocuments.end()) return nullptr;

    if (!mResources) {
        if (required) state.result.error("layout.resource.provider_missing", "No Layout Resource collection is configured.", resourceId);
        return nullptr;
    }

    const std::optional<std::string> source = mResources->load(resourceId);
    if (!source) {
        if (required)
            state.result.error("layout.resource.missing", "Could not load resource chain: " + resourceChain(state.resourceStack, resourceId) + ".",
                               resourceId);
        return nullptr;
    }

    std::shared_ptr<const SourceDocument> document;
    bool valid = false;
    {
        std::lock_guard<std::mutex> lock(mDocumentCacheMutex);
        CachedDocument& cached = mDocumentCache[resourceId];
        if (!cached.initialized || cached.source != *source) {
            SourceDocumentParseResult parsed = SourceDocumentParser().parse(*source, resourceId);
            cached.initialized = true;
            cached.source = *source;
            cached.document = std::shared_ptr<const SourceDocument>(std::move(parsed.document));
            cached.warnings = std::move(parsed.warnings);
            cached.errors = std::move(parsed.errors);
        }
        state.result.warnings.insert(state.result.warnings.end(), cached.warnings.begin(), cached.warnings.end());
        state.result.errors.insert(state.result.errors.end(), cached.errors.begin(), cached.errors.end());
        document = cached.document;
        valid = document && cached.errors.empty();
    }

    if (!valid) {
        state.invalidDocuments.insert(resourceId);
        return nullptr;
    }

    const SourceDocument* result = document.get();
    state.documents.emplace(resourceId, std::move(document));
    return result;
}

std::unique_ptr<Element> LayoutResourceCompiler::buildDocument(const SourceDocument& document, std::unique_ptr<Element> root,
                                                               BuildState& state) const {
    if (!document.root) {
        state.result.error("layout.html.invalid", "Radia UI HTML must contain one root element.", document.source);
        return nullptr;
    }

    return buildNode(*document.root, document.source, std::move(root), state);
}

const ElementDefinition* LayoutResourceCompiler::lookupElementDefinition(const SourceNode& sourceNode, const std::string& source,
                                                                         BuildState& state) const {
    std::string inputType;
    if (sourceNode.tag == Tag::Input) {
        const auto type = sourceNode.attributes.find("type");
        if (type != sourceNode.attributes.end()) inputType = type->second.value;
    }
    const ElementDefinition* definition = findElementDefinition(sourceNode.tag, inputType);
    if (!definition) {
        state.result.error("layout.element.unknown", "Unsupported HTML element: " + sourceNode.authoredName + ".", source,
                           sourceNode.source.begin.line, sourceNode.source.begin.column);
        return nullptr;
    }
    if (definition->scopedOnly) {
        state.result.error("layout.element.scoped", "<" + definition->elementName + "> is valid only in its owning composite.", source,
                           sourceNode.source.begin.line, sourceNode.source.begin.column);
        return nullptr;
    }
    return definition;
}

bool LayoutResourceCompiler::resolveElementResource(const ElementBuildInput& input, const ElementDefinition& definition,
                                                    std::unique_ptr<Element>& element, BuildState& state) const {
    std::string filename;
    if (!readElementAttribute(input, "filename", filename)) {
        if (!element) element = definition.create();
        return true;
    }

    if (!definition.resourceRoot) {
        state.result.error("layout.filename.unsupported", "The filename attribute is not supported on <" + definition.elementName + ">.",
                           input.sourceName, input.source.begin.line, input.source.begin.column);
        return false;
    }

    const bool rooted = filename.rfind("xui/", 0) == 0 || (!filename.empty() && (filename.front() == '/' || filename.front() == '\\'));
    const std::string resource = normalizeResource(rooted ? filename : directoryOf(input.sourceName) + filename);
    if (resource.empty()) {
        state.result.error("layout.resource.path_invalid", "Invalid resource filename: " + filename + ".", input.sourceName, input.source.begin.line,
                           input.source.begin.column);
        return false;
    }
    element = createResourceElement(resource, state);
    if (!element) return false;
    if (schemaNameKey(element->elementName()) != schemaNameKey(definition.resourceRoot->expectedElementName)) {
        state.result.error("layout.resource.root_invalid",
                           "Referenced resource must have a <" + definition.resourceRoot->expectedElementName + "> root: " + resource + ".",
                           input.sourceName, input.source.begin.line, input.source.begin.column);
        return false;
    }
    return true;
}

std::unique_ptr<Element> LayoutResourceCompiler::buildNode(const SourceNode& sourceNode, const std::string& source, std::unique_ptr<Element> element,
                                                           BuildState& state) const {
    const ElementDefinition* definition = lookupElementDefinition(sourceNode, source, state);
    if (!definition) return nullptr;

    loadElementDefaults(definition->elementName, state);
    const auto defaults = state.elementDefaults.find(schemaNameKey(definition->elementName));
    const SourceNode* defaultRoot = defaults == state.elementDefaults.end() ? nullptr : defaults->second;
    ElementBuildInput input = makeElementInput(sourceNode, defaultRoot, source);
    if (!resolveElementResource(input, *definition, element, state)) return nullptr;

    Element* target = element.get();
    if (!target || target->elementName() != definition->elementName) {
        state.result.error("layout.builder.type_mismatch", "Registered builder does not create <" + definition->elementName + ">.", source,
                           sourceNode.source.begin.line, sourceNode.source.begin.column);
        return nullptr;
    }

    validateElementAttributes(input, definition->attributes, state.result);
    applyCommonElementAttributes(input, *target, state.result);
    if (definition->attributeBehavior.apply) definition->attributeBehavior.apply(input, *target, state.result, state.context);
    if (sourceNode.tag == Tag::Kbd) {
        std::string shortcut;
        if (!readElementAttribute(input, "shortcut", shortcut)) {
            state.result.error("layout.kbd.shortcut_required", "<kbd> requires a shortcut attribute.", source, sourceNode.source.begin.line,
                               sourceNode.source.begin.column);
        } else if (!isElementIdentifier(shortcut)) {
            const ElementAttribute* attribute = input.find("shortcut");
            state.result.error("layout.kbd.shortcut_invalid", "<kbd> shortcut must be a valid identifier.", source,
                               attribute ? attribute->source.begin.line : sourceNode.source.begin.line,
                               attribute ? attribute->source.begin.column : sourceNode.source.begin.column);
        } else {
            detail::ElementCompilerAccess::setKeybinding(*target, std::move(shortcut));
        }
    }
    if (definition->compositionBehavior.validate)
        state.authoredElementRecords.emplace(target, BuildState::AuthoredElement{target, std::move(input), definition});

    const SourceNode& contentNode = defaultRoot && !hasAuthoredContent(sourceNode) ? *defaultRoot : sourceNode;
    buildChildren(*target, contentNode, *definition, source, state);
    return element;
}

void LayoutResourceCompiler::buildChildren(Element& target, const SourceNode& node, const ElementDefinition& definition, const std::string& source,
                                           BuildState& state) const {
    ChildBuildContext context{target, definition, source, state};
    for (const SourceContent& content : node.content) {
        if (content.isText()) {
            appendTextContent(content, context);
            continue;
        }

        const SourceNode& childNode = *content.node;
        if (consumeFlowBreak(childNode, context) == ChildHandling::Handled) continue;
        if (consumeScopedElement(childNode, context) == ChildHandling::Handled) continue;
        const auto partAttributes = definition.childrenBehavior.partAttributes.find(sourceTagName(childNode.tag));
        if (partAttributes != definition.childrenBehavior.partAttributes.end())
            validateElementAttributes(makeElementInput(childNode, nullptr, source), partAttributes->second, state.result);
        if (consumeChildContainer(childNode, context) == ChildHandling::Handled) continue;
        (void)buildRegularChild(childNode, context);
    }
    if (context.pendingFlowBreak)
        state.result.error("layout.flow_break.trailing", "Flow Break requires a following layout child.", source, node.source.end.line,
                           node.source.end.column);
    if (definition.contentBehavior.mode == ElementContentMode::ElementText && !context.elementText.empty() && definition.contentBehavior.applyText)
        definition.contentBehavior.applyText(std::move(context.elementText), target, state.result, source, state.context, node.source.begin.line);
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::appendTextContent(const SourceContent& content, ChildBuildContext& context) const {
    const std::string& value = content.text;
    if (value.empty()) return ChildHandling::Handled;
    if (context.definition.contentBehavior.mode == ElementContentMode::Unsupported && trimmedText(value).empty()) return ChildHandling::Handled;
    if (context.definition.contentBehavior.mode == ElementContentMode::Unsupported) {
        context.state.result.error("layout.text.unsupported", "Text content is not supported in <" + context.definition.elementName + ">.",
                                   context.source, content.source.begin.line, content.source.begin.column);
    } else if (context.definition.contentBehavior.mode == ElementContentMode::ElementText) {
        if (!context.elementText.empty()) context.elementText += value;
        else context.elementText = std::move(value);
    } else {
        const bool contributesLayoutChild = !trimmedText(value).empty();
        const ResolvedLayoutText text =
            localizedLayoutText(std::move(value), context.state.result, context.source, context.state.context, content.source.begin.line);
        const auto appendLiteral = [&](std::string literal) {
            if (literal.empty()) return;
            Node& child = detail::appendText(context.target, std::move(literal));
            if (contributesLayoutChild) context.markLayoutChild(child);
        };
        if (!text.text) {
            if (context.definition.contentBehavior.createTextChild) {
                if (auto child = context.definition.contentBehavior.createTextChild(text.literal)) {
                    if (contributesLayoutChild) context.markLayoutChild(*child);
                    context.target.append(std::move(child));
                }
            } else appendLiteral(text.literal);
        } else {
            appendLiteral(text.prefix);
            Node& child = detail::appendLocalizedText(context.target, *text.text, context.state.context->resolveMarkup(*text.text));
            context.markLayoutChild(child);
            appendLiteral(text.suffix);
        }
    }
    return ChildHandling::Handled;
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::consumeFlowBreak(const SourceNode& childNode, ChildBuildContext& context) const {
    if (childNode.tag != Tag::Br) return ChildHandling::Unhandled;
    rejectAuthoredAttributes(childNode, context.state.result, context.source);
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
    if (auto child = buildNode(childNode, context.source, nullptr, context.state)) context.target.append(std::move(child));
    return ChildHandling::Handled;
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::consumeScopedElement(const SourceNode& childNode, ChildBuildContext& context) const {
    const auto scopedInline = context.definition.contentBehavior.scopedElements.find(schemaNameKey(sourceTagName(childNode.tag)));
    if (scopedInline == context.definition.contentBehavior.scopedElements.end()) return ChildHandling::Unhandled;
    const ElementDefinition* scopedDefinition = findElementDefinition(childNode.tag);
    const ElementBuildInput input = makeElementInput(childNode, nullptr, context.source);
    Element* scopedPart =
        scopedInline->second.create(context.target, context.state.result, context.source, childNode.source.begin.line, childNode.source.begin.column);
    if (scopedPart)
        appendInlineElements(*scopedPart, childNode.content, scopedInline->second.elementName, scopedInline->second.acceptedElements,
                             context.state.result, context.source, context.state.context);
    if (scopedDefinition) {
        validateElementAttributes(input, scopedDefinition->attributes, context.state.result);
        if (scopedPart) applyCommonElementAttributes(input, *scopedPart, context.state.result);
    } else {
        rejectAuthoredAttributes(childNode, context.state.result, context.source);
    }
    if (scopedPart) context.markLayoutChild(*scopedPart);
    return ChildHandling::Handled;
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::consumeChildContainer(const SourceNode& childNode, ChildBuildContext& context) const {
    if (!context.definition.childrenBehavior.claim) return ChildHandling::Unhandled;
    const ChildClaim claim =
        context.definition.childrenBehavior.claim(makeElementInput(childNode, nullptr, context.source), context.target, context.state.result);
    if (claim.kind() == ChildClaim::Kind::NotHandled) return ChildHandling::Unhandled;
    if (Element* container = claim.container()) {
        for (const SourceContent& nestedContent : childNode.content) {
            if (nestedContent.isText()) {
                if (!trimmedText(nestedContent.text).empty())
                    context.state.result.error("layout.text.unsupported",
                                               "Text content is not supported in <" + std::string(sourceTagName(childNode.tag)) + ">.",
                                               context.source, nestedContent.source.begin.line, nestedContent.source.begin.column);
                continue;
            }
            if (auto childElement = buildNode(*nestedContent.node, context.source, nullptr, context.state))
                container->append(std::move(childElement));
        }
    }
    return ChildHandling::Handled;
}

LayoutResourceCompiler::ChildHandling LayoutResourceCompiler::buildRegularChild(const SourceNode& childNode, ChildBuildContext& context) const {
    if (auto childElement = buildNode(childNode, context.source, nullptr, context.state)) {
        context.markLayoutChild(*childElement);
        context.target.append(std::move(childElement));
    }
    return ChildHandling::Handled;
}
} // namespace radia::ui
