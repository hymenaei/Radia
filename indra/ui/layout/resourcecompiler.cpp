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
#include <set>
#include <unordered_set>
#include "dom/elementinternal.h"
#include "eventcall.h"
#include "html/element.h"
#include "html/elementfactory.h"
#include "html/elementnames.h"
#include "layout/document.h"
#include "resource/elementdefinition.h"
#include "resourceprovider.h"
#include "text/inlineelements.h"

namespace radia::ui {
using detail::appendLocalizedText;
using detail::appendText;
using detail::ElementCompilerAccess;
using detail::HTMLElementFactory;
using detail::NodeAccess;

struct ResourceCompiler::BuildState {
    struct AuthoredElement {
        Element* element = nullptr;
        ElementBuildInput input;
        const ResourceElementDefinition* definition = nullptr;
    };

    ResourceBuildResult result;
    std::vector<ResourceId> resourceStack;
    std::map<ResourceId, std::shared_ptr<const SourceDocument>> documents;
    std::set<ResourceId> invalidDocuments;
    std::unordered_map<std::string, const SourceNode*> elementDefaults;
    std::unordered_map<const Element*, AuthoredElement> authoredElementRecords;
    std::unique_ptr<ElementBuildContext> buildContext;
    ResourceId baseId;
};

struct ResourceCompiler::ChildBuildContext {
    Element& target;
    const ResourceElementDefinition& definition;
    const std::string& sourceName;
    BuildState& state;
    ElementBuildContext& buildContext;
    std::string elementText;
    bool pendingFlowBreak = false;
    bool hasLayoutChild = false;

    void markLayoutChild(Node& child) {
        NodeAccess::setFlowBreakBefore(child, pendingFlowBreak);
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

std::string resourceChain(const std::vector<ResourceId>& stack, const ResourceId& id) {
    std::string result;
    for (const ResourceId& item : stack) {
        if (!result.empty()) result += " -> ";
        result += item.value();
    }
    if (!result.empty()) result += " -> ";
    return result + id.value();
}

bool hasAuthoredContent(const SourceNode& node) {
    for (const SourceContent& content : node.content)
        if (content.node || !trimmedText(content.text).empty()) return true;
    return false;
}

ElementBuildInput makeElementInput(const SourceNode& node, const SourceNode* defaults, const std::string& sourceName) {
    ElementBuildInput input;
    input.tag = node.tag;
    input.authoredName = node.authoredName;
    input.source = node.source;
    input.sourceName = sourceName;
    input.attributes.reserve(node.attributes.size() + (defaults ? defaults->attributes.size() : 0));
    for (const auto& [name, attribute] : node.attributes)
        input.attributes.emplace(name, ElementAttribute{attribute.authoredName, attribute.value, attribute.source});
    if (defaults)
        for (const auto& [name, attribute] : defaults->attributes)
            input.attributes.try_emplace(name, ElementAttribute{attribute.authoredName, attribute.value, attribute.source});
    return input;
}

void rejectAuthoredAttributes(const SourceNode& node, ElementBuildContext& context, const std::string& sourceName) {
    for (const auto& [name, attribute] : node.attributes)
        context.error("layout.attribute.unknown",
                      "Unknown attribute on <" + std::string(htmlTagName(node.tag)) + ">: " + attribute.authoredName + ".", sourceName,
                      attribute.source.begin.line, attribute.source.begin.column);
}

bool validateDefaultNode(const SourceNode& node, ElementBuildContext& context, const std::string& sourceName) {
    const ResourceElementDefinition* definition = findElementDefinition(node.tag);
    if (!definition) {
        context.error("layout.defaults.element_unknown", "Element Defaults contain an unsupported element: " + node.authoredName + ".", sourceName,
                      node.source.begin.line, node.source.begin.column);
        return false;
    }

    const std::size_t errorsBefore = context.errorCount();
    const ElementBuildInput input = makeElementInput(node, nullptr, sourceName);
    validateElementAttributes(input, definition->attributes, context);

    std::string ignored;
    if (readElementAttribute(input, "id", ignored)
        || readElementAttribute(input, "filename", ignored)
        || readElementAttribute(input, "for", ignored)) {
        context.error("layout.defaults.controller_attribute", "Element Defaults cannot declare IDs, relationships, includes, or controller behavior.",
                      sourceName, node.source.begin.line, node.source.begin.column);
    }
    for (const AuthoredEventDescriptor& descriptor : kAuthoredEventDescriptors) {
        if (readElementAttribute(input, descriptor.attribute, ignored)) {
            context.error("layout.defaults.controller_attribute", "Element Defaults cannot declare Event Handler Calls.", sourceName,
                          node.source.begin.line, node.source.begin.column);
            break;
        }
    }

    std::unique_ptr<Element> probe = HTMLElementFactory::Create(htmlTagName(node.tag));
    if (probe) {
        applyCommonElementAttributes(input, *probe, context);
        if (definition->attributeBehavior.apply) definition->attributeBehavior.apply(input, *probe, context);
    }

    for (const SourceContent& content : node.content)
        if (content.node) validateDefaultNode(*content.node, context, sourceName);
    return context.errorCount() == errorsBefore;
}
} // namespace

ResourceCompiler::ResourceCompiler(const ResourceProvider* provider) : mProvider(provider) {}

ResourceBuildResult ResourceCompiler::buildElementTreeFromResource(const ResourceId& id, const ResourceBuildContext* context) const {
    BuildState state;
    state.buildContext = std::make_unique<ElementBuildContext>(state.result, context);
    if (!id.valid()) {
        state.result.error("layout.resource.path_invalid", "Invalid or empty resource path.", id.value());
        return std::move(state.result);
    }
    std::unique_ptr<Element> root = createResourceElement(id, state);
    if (root && !state.result.hasErrors()) {
        ElementCompilerAccess::setIdScopeRoot(*root);
        validateElementScope(*root, state, id.value());
    }
    if (!state.result.hasErrors() && root) state.result.document = std::make_unique<Document>(std::move(root));
    return std::move(state.result);
}

ResourceBuildResult ResourceCompiler::buildElementTreeFromString(const std::string& html, const std::string& sourceName,
                                                                 const ResourceBuildContext* context, ResourceId baseId) const {
    ResourceBuildResult result;
    SourceDocumentParseResult parsed = SourceDocumentParser().parse(html, sourceName);
    result.warnings = std::move(parsed.warnings);
    result.errors = std::move(parsed.errors);
    const std::shared_ptr<const SourceDocument> document = parsed.document;
    if (!document || result.hasErrors()) return result;

    BuildState state;
    state.result = std::move(result);
    state.buildContext = std::make_unique<ElementBuildContext>(state.result, context);
    state.baseId = std::move(baseId);
    std::unique_ptr<Element> root = buildDocument(*document, nullptr, state);
    if (root && !state.result.hasErrors()) {
        ElementCompilerAccess::setIdScopeRoot(*root);
        validateElementScope(*root, state, sourceName);
    }
    if (!state.result.hasErrors() && root) state.result.document = std::make_unique<Document>(std::move(root));
    return std::move(state.result);
}

void ResourceCompiler::validateElementScope(Element& scope, BuildState& state, const std::string& sourceName, bool includeRootInIdScope) const {
    std::unordered_map<std::string, Element*> ids;
    std::unordered_set<std::string> duplicates;
    std::vector<const BuildState::AuthoredElement*> authoredElementRecords;
    std::function<void(Element&, bool)> visit = [&](Element& element, bool root) {
        if (!root && element.idScopeRoot()) {
            if (!element.id().empty() && !ids.emplace(element.id(), &element).second) duplicates.insert(element.id());
            validateElementScope(element, state, sourceName, false);
            return;
        }
        if ((!root || includeRootInIdScope) && !element.id().empty() && !ids.emplace(element.id(), &element).second) duplicates.insert(element.id());
        const auto authored = state.authoredElementRecords.find(&element);
        if (authored != state.authoredElementRecords.end()) authoredElementRecords.push_back(&authored->second);
        for (Element* child : element.children()) visit(*child, false);
    };
    visit(scope, true);

    const ElementScopeContext context(ids, duplicates, [](const Element& element) {
        const ResourceElementDefinition* definition = findElementDefinition(lookupHTMLTag(element.elementName()));
        return definition && definition->labelable;
    });
    for (const BuildState::AuthoredElement* authored : authoredElementRecords) {
        if (!authored->element || !authored->definition || !authored->definition->compositionBehavior.validate) continue;
        authored->definition->compositionBehavior.validate(authored->input, *authored->element, context, *state.buildContext);
    }
}

DiagnosticResult ResourceCompiler::validateElementDefaults(const std::string& elementName, const ResourceBuildContext* context) const {
    BuildState state;
    state.buildContext = std::make_unique<ElementBuildContext>(state.result, context);
    loadElementDefaults(elementName, state);
    DiagnosticResult result;
    result.warnings = std::move(state.result.warnings);
    result.errors = std::move(state.result.errors);
    return result;
}

void ResourceCompiler::loadElementDefaults(const std::string& elementName, BuildState& state) const {
    const std::string lookup = canonicalizeHTMLName(elementName);
    if (state.elementDefaults.find(lookup) != state.elementDefaults.end()) return;
    const ResourceElementDefinition* definition = findElementDefinition(lookupHTMLTag(elementName));
    const std::string canonical = definition ? definition->elementName : elementName;
    const ResourceId defaultId("elements/" + canonical + ".html");
    const SourceNode* defaultRoot = nullptr;
    if (!definition) {
        state.result.error("layout.defaults.element_unknown", "Element Defaults target an unsupported element: " + canonical + ".",
                           defaultId.value());
        state.elementDefaults.emplace(lookup, nullptr);
        return;
    }

    if (mProvider) {
        const SourceDocument* document = loadDocument(defaultId, state, false);
        if (!document || !document->root) {
            state.elementDefaults.emplace(lookup, nullptr);
            return;
        }
        defaultRoot = document->root.get();
        const std::size_t errorsBefore = state.result.errors.size();
        if (canonicalizeHTMLName(htmlTagName(defaultRoot->tag)) != lookup) {
            state.result.error("layout.defaults.root_invalid", "Element Defaults root must be <" + canonical + ">.", document->sourceName,
                               defaultRoot->source.begin.line, defaultRoot->source.begin.column);
            defaultRoot = nullptr;
        } else {
            if (!validateDefaultNode(*defaultRoot, *state.buildContext, document->sourceName)) defaultRoot = nullptr;
        }
        if (state.result.errors.size() != errorsBefore) defaultRoot = nullptr;
    }
    state.elementDefaults.emplace(lookup, defaultRoot);
}

std::unique_ptr<Element> ResourceCompiler::createResourceElement(const ResourceId& id, BuildState& state) const {
    if (!id.valid()) {
        state.result.error("layout.resource.path_invalid", "Invalid or empty resource path.", id.value());
        return nullptr;
    }
    if (id.value().rfind("elements/", 0) == 0) {
        state.result.error("layout.resource.reserved", "Element default resources cannot be instantiated as Layout Resources: " + id.value() + ".",
                           id.value());
        return nullptr;
    }
    if (std::find(state.resourceStack.begin(), state.resourceStack.end(), id) != state.resourceStack.end()) {
        state.result.error("layout.resource.cycle", "Recursive layout resource reference: " + resourceChain(state.resourceStack, id) + ".",
                           id.value());
        return nullptr;
    }
    const SourceDocument* document = loadDocument(id, state, true);
    if (!document) return nullptr;

    state.resourceStack.push_back(id);
    std::unique_ptr<Element> root = buildDocument(*document, nullptr, state);
    state.resourceStack.pop_back();
    if (root) ElementCompilerAccess::setIdScopeRoot(*root);
    return root;
}

const SourceDocument* ResourceCompiler::loadDocument(const ResourceId& id, BuildState& state, bool required) const {
    const auto cached = state.documents.find(id);
    if (cached != state.documents.end()) return cached->second.get();
    if (state.invalidDocuments.find(id) != state.invalidDocuments.end()) return nullptr;

    if (!mProvider) {
        if (required) state.result.error("layout.resource.provider_missing", "No Layout Resource collection is configured.", id.value());
        return nullptr;
    }

    const std::optional<ResourceSource> loaded = mProvider->load(id);
    if (!loaded) {
        if (required)
            state.result.error("layout.resource.missing", "Could not load resource chain: " + resourceChain(state.resourceStack, id) + ".",
                               id.value());
        return nullptr;
    }

    std::shared_ptr<const SourceDocument> document;
    bool valid = false;
    {
        std::lock_guard<std::mutex> lock(mDocumentCacheMutex);
        CachedDocument& cached = mDocumentCache[id];
        if (!cached.initialized || cached.content != loaded->content || cached.provenance != loaded->provenance) {
            SourceDocumentParseResult parsed = SourceDocumentParser().parse(loaded->content, loaded->provenance);
            cached.initialized = true;
            cached.content = loaded->content;
            cached.provenance = loaded->provenance;
            cached.document = parsed.document;
            cached.warnings = std::move(parsed.warnings);
            cached.errors = std::move(parsed.errors);
        }
        state.result.warnings.insert(state.result.warnings.end(), cached.warnings.begin(), cached.warnings.end());
        state.result.errors.insert(state.result.errors.end(), cached.errors.begin(), cached.errors.end());
        document = cached.document;
        valid = document && cached.errors.empty();
    }

    if (!valid) {
        state.invalidDocuments.insert(id);
        return nullptr;
    }

    const SourceDocument* result = document.get();
    state.documents.emplace(id, std::move(document));
    return result;
}

std::unique_ptr<Element> ResourceCompiler::buildDocument(const SourceDocument& document, std::unique_ptr<Element> root, BuildState& state) const {
    if (!document.root) {
        state.result.error("layout.html.invalid", "Radia UI HTML must contain one root element.", document.sourceName);
        return nullptr;
    }

    return buildNode(*document.root, document.sourceName, std::move(root), state);
}

const ResourceElementDefinition* ResourceCompiler::lookupElementDefinition(const SourceNode& node, const std::string& sourceName,
                                                                           BuildState& state) const {
    const ResourceElementDefinition* definition = findElementDefinition(node.tag);
    if (!definition) {
        state.result.error("layout.element.unknown", "Unsupported HTML element: " + node.authoredName + ".", sourceName, node.source.begin.line,
                           node.source.begin.column);
        return nullptr;
    }
    if (definition->scopedOnly) {
        state.result.error("layout.element.scoped", "<" + definition->elementName + "> is valid only in its owning composite.", sourceName,
                           node.source.begin.line, node.source.begin.column);
        return nullptr;
    }
    return definition;
}

bool ResourceCompiler::resolveElementResource(const ElementBuildInput& input, const ResourceElementDefinition& definition,
                                              std::unique_ptr<Element>& element, const ResourceId& baseId, BuildState& state) const {
    std::string filename;
    if (!readElementAttribute(input, "filename", filename)) {
        if (!element) element = HTMLElementFactory::Create(htmlTagName(input.tag));
        return true;
    }

    if (!definition.resourceRoot) {
        state.result.error("layout.filename.unsupported", "The filename attribute is not supported on <" + definition.elementName + ">.",
                           input.sourceName, input.source.begin.line, input.source.begin.column);
        return false;
    }

    const ResourceId id = ResourceId::resolve(baseId, filename);
    if (!id.valid()) {
        state.result.error("layout.resource.path_invalid", "Invalid resource filename: " + filename + ".", input.sourceName, input.source.begin.line,
                           input.source.begin.column);
        return false;
    }
    element = createResourceElement(id, state);
    if (!element) return false;
    if (canonicalizeHTMLName(element->elementName()) != canonicalizeHTMLName(definition.resourceRoot->expectedElementName)) {
        state.result.error("layout.resource.root_invalid",
                           "Referenced resource must have a <" + definition.resourceRoot->expectedElementName + "> root: " + id.value() + ".",
                           input.sourceName, input.source.begin.line, input.source.begin.column);
        return false;
    }
    return true;
}

std::unique_ptr<Element> ResourceCompiler::buildNode(const SourceNode& node, const std::string& sourceName, std::unique_ptr<Element> element,
                                                     BuildState& state) const {
    const ResourceElementDefinition* definition = lookupElementDefinition(node, sourceName, state);
    if (!definition) return nullptr;

    loadElementDefaults(definition->elementName, state);
    const auto defaults = state.elementDefaults.find(canonicalizeHTMLName(definition->elementName));
    const SourceNode* defaultRoot = defaults == state.elementDefaults.end() ? nullptr : defaults->second;
    ElementBuildInput input = makeElementInput(node, defaultRoot, sourceName);
    const ResourceId& baseId = state.resourceStack.empty() ? state.baseId : state.resourceStack.back();
    if (!resolveElementResource(input, *definition, element, baseId, state)) return nullptr;

    Element* target = element.get();
    if (!target || target->elementName() != definition->elementName) {
        state.result.error("layout.builder.type_mismatch", "Registered builder does not create <" + definition->elementName + ">.", sourceName,
                           node.source.begin.line, node.source.begin.column);
        return nullptr;
    }

    validateElementAttributes(input, definition->attributes, *state.buildContext);
    applyCommonElementAttributes(input, *target, *state.buildContext);
    if (definition->attributeBehavior.apply) definition->attributeBehavior.apply(input, *target, *state.buildContext);
    if (node.tag == HTMLTag::Kbd) {
        std::string shortcut;
        if (!readElementAttribute(input, "shortcut", shortcut)) {
            state.result.error("layout.kbd.shortcut_required", "<kbd> requires a shortcut attribute.", sourceName, node.source.begin.line,
                               node.source.begin.column);
        } else if (shortcut.empty()) {
            const ElementAttribute* attribute = input.find("shortcut");
            state.result.error("layout.kbd.shortcut_invalid", "<kbd> shortcut must be non-empty.", sourceName,
                               attribute ? attribute->source.begin.line : node.source.begin.line,
                               attribute ? attribute->source.begin.column : node.source.begin.column);
        } else {
            static_cast<HTMLElement&>(*target).setKeybinding(std::move(shortcut));
        }
    }
    if (definition->compositionBehavior.validate)
        state.authoredElementRecords.emplace(target, BuildState::AuthoredElement{target, std::move(input), definition});

    const SourceNode& contentNode = defaultRoot && !hasAuthoredContent(node) ? *defaultRoot : node;
    buildChildren(*target, contentNode, *definition, sourceName, state);
    return element;
}

void ResourceCompiler::buildChildren(Element& target, const SourceNode& node, const ResourceElementDefinition& definition,
                                     const std::string& sourceName, BuildState& state) const {
    ChildBuildContext context{target, definition, sourceName, state, *state.buildContext};
    for (const SourceContent& content : node.content) {
        if (content.isText()) {
            appendTextContent(content, context);
            continue;
        }

        const SourceNode& childNode = *content.node;
        if (consumeFlowBreak(childNode, context) == ChildHandling::Handled) continue;
        if (consumeScopedElement(childNode, context) == ChildHandling::Handled) continue;
        const auto partAttributes = definition.childrenBehavior.partAttributes.find(std::string(htmlTagName(childNode.tag)));
        if (partAttributes != definition.childrenBehavior.partAttributes.end())
            validateElementAttributes(makeElementInput(childNode, nullptr, sourceName), partAttributes->second, *state.buildContext);
        if (consumeChildContainer(childNode, context) == ChildHandling::Handled) continue;
        (void)buildRegularChild(childNode, context);
    }
    if (context.pendingFlowBreak)
        state.result.error("layout.flow_break.trailing", "Flow Break requires a following layout child.", sourceName, node.source.end.line,
                           node.source.end.column);
    if (definition.contentBehavior.mode == ElementContentMode::ElementText && !context.elementText.empty() && definition.contentBehavior.applyText)
        definition.contentBehavior.applyText(std::move(context.elementText), target, *state.buildContext, sourceName, node.source.begin.line);
}

ResourceCompiler::ChildHandling ResourceCompiler::appendTextContent(const SourceContent& content, ChildBuildContext& context) const {
    const std::string& value = content.text;
    if (value.empty()) return ChildHandling::Handled;
    if (context.definition.contentBehavior.mode == ElementContentMode::Unsupported && trimmedText(value).empty()) return ChildHandling::Handled;
    if (context.definition.contentBehavior.mode == ElementContentMode::Unsupported) {
        context.state.result.error("layout.text.unsupported", "Text content is not supported in <" + context.definition.elementName + ">.",
                                   context.sourceName, content.source.begin.line, content.source.begin.column);
    } else if (context.definition.contentBehavior.mode == ElementContentMode::ElementText) {
        if (!context.elementText.empty()) context.elementText += value;
        else context.elementText = std::move(value);
    } else {
        const bool contributesLayoutChild = !trimmedText(value).empty();
        const ResolvedLayoutText text = localizedLayoutText(std::move(value), context.buildContext, context.sourceName, content.source.begin.line);
        const auto appendLiteral = [&](std::string literal) {
            if (literal.empty()) return;
            Node& child = appendText(context.target, std::move(literal));
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
            Node& child = appendLocalizedText(context.target, *text.text, context.buildContext.resolveHTML(*text.text));
            context.markLayoutChild(child);
            appendLiteral(text.suffix);
        }
    }
    return ChildHandling::Handled;
}

ResourceCompiler::ChildHandling ResourceCompiler::consumeFlowBreak(const SourceNode& childNode, ChildBuildContext& context) const {
    if (childNode.tag != HTMLTag::Br) return ChildHandling::Unhandled;
    rejectAuthoredAttributes(childNode, context.buildContext, context.sourceName);
    if (hasAuthoredContent(childNode))
        context.state.result.error("layout.flow_break.children_unsupported", "Flow Break <br> cannot contain content.", context.sourceName,
                                   childNode.source.begin.line, childNode.source.begin.column);
    if (!context.hasLayoutChild)
        context.state.result.error("layout.flow_break.leading", "Flow Break requires a preceding layout child.", context.sourceName,
                                   childNode.source.begin.line, childNode.source.begin.column);
    else if (context.pendingFlowBreak)
        context.state.result.error("layout.flow_break.consecutive", "Consecutive Flow Break directives are not supported.", context.sourceName,
                                   childNode.source.begin.line, childNode.source.begin.column);
    else context.pendingFlowBreak = true;
    if (auto child = buildNode(childNode, context.sourceName, nullptr, context.state)) context.target.append(std::move(child));
    return ChildHandling::Handled;
}

ResourceCompiler::ChildHandling ResourceCompiler::consumeScopedElement(const SourceNode& childNode, ChildBuildContext& context) const {
    const auto scopedInline = context.definition.contentBehavior.scopedElements.find(canonicalizeHTMLName(htmlTagName(childNode.tag)));
    if (scopedInline == context.definition.contentBehavior.scopedElements.end()) return ChildHandling::Unhandled;
    const ResourceElementDefinition* scopedDefinition = findElementDefinition(childNode.tag);
    const ElementBuildInput input = makeElementInput(childNode, nullptr, context.sourceName);
    Element* scopedPart = scopedInline->second.create(context.target, context.buildContext, context.sourceName, childNode.source.begin.line,
                                                      childNode.source.begin.column);
    if (scopedPart)
        appendInlineElements(*scopedPart, childNode.content, scopedInline->second.elementName, scopedInline->second.acceptedTags,
                             context.buildContext, context.sourceName);
    if (scopedDefinition) {
        validateElementAttributes(input, scopedDefinition->attributes, context.buildContext);
        if (scopedPart) applyCommonElementAttributes(input, *scopedPart, context.buildContext);
    } else {
        rejectAuthoredAttributes(childNode, context.buildContext, context.sourceName);
    }
    if (scopedPart) context.markLayoutChild(*scopedPart);
    return ChildHandling::Handled;
}

ResourceCompiler::ChildHandling ResourceCompiler::consumeChildContainer(const SourceNode& childNode, ChildBuildContext& context) const {
    if (!context.definition.childrenBehavior.claim) return ChildHandling::Unhandled;
    const ChildClaim claim =
        context.definition.childrenBehavior.claim(makeElementInput(childNode, nullptr, context.sourceName), context.target, context.buildContext);
    if (claim.kind() == ChildClaim::Kind::NotHandled) return ChildHandling::Unhandled;
    if (Element* container = claim.container()) {
        for (const SourceContent& nestedContent : childNode.content) {
            if (nestedContent.isText()) {
                if (!trimmedText(nestedContent.text).empty())
                    context.state.result.error("layout.text.unsupported",
                                               "Text content is not supported in <" + std::string(htmlTagName(childNode.tag)) + ">.",
                                               context.sourceName, nestedContent.source.begin.line, nestedContent.source.begin.column);
                continue;
            }
            if (auto childElement = buildNode(*nestedContent.node, context.sourceName, nullptr, context.state))
                container->append(std::move(childElement));
        }
    }
    return ChildHandling::Handled;
}

ResourceCompiler::ChildHandling ResourceCompiler::buildRegularChild(const SourceNode& childNode, ChildBuildContext& context) const {
    if (auto childElement = buildNode(childNode, context.sourceName, nullptr, context.state)) {
        context.markLayoutChild(*childElement);
        context.target.append(std::move(childElement));
    }
    return ChildHandling::Handled;
}
} // namespace radia::ui
