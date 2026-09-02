/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */
#pragma once

#include <cstdint>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>
#include "layout/buildresult.h"
#include "resourceprovider.h"

namespace radia::ui {
class Element;
class ResourceBuildContext;
struct ElementBuildInput;
struct ResourceElementDefinition;
struct SourceContent;
struct SourceDocument;
struct SourceNode;

class ResourceCompiler final {
public:
    explicit ResourceCompiler(const ResourceProvider* provider = nullptr);

    ResourceBuildResult buildElementTreeFromResource(const ResourceId& id, const ResourceBuildContext* context = nullptr) const;
    ResourceBuildResult buildElementTreeFromString(const std::string& html, const std::string& sourceName = {},
                                                   const ResourceBuildContext* context = nullptr, ResourceId baseId = {}) const;
    DiagnosticResult validateElementDefaults(const std::string& elementName, const ResourceBuildContext* context = nullptr) const;

private:
    struct CachedDocument {
        bool initialized = false;
        std::string content;
        std::string provenance;
        std::shared_ptr<const SourceDocument> document;
        std::vector<Diagnostic> warnings;
        std::vector<Diagnostic> errors;
    };
    struct BuildState;
    struct ChildBuildContext;
    enum class ChildHandling : uint8_t { Unhandled, Handled };

    std::unique_ptr<Element> buildDocument(const SourceDocument& document, std::unique_ptr<Element> root, BuildState& state) const;
    std::unique_ptr<Element> buildNode(const SourceNode& node, const std::string& sourceName, std::unique_ptr<Element> element,
                                       BuildState& state) const;
    const ResourceElementDefinition* lookupElementDefinition(const SourceNode& node, const std::string& sourceName, BuildState& state) const;
    bool resolveElementResource(const ElementBuildInput& input, const ResourceElementDefinition& definition, std::unique_ptr<Element>& element,
                                const ResourceId& baseId, BuildState& state) const;
    void buildChildren(Element& target, const SourceNode& node, const ResourceElementDefinition& definition, const std::string& sourceName,
                       BuildState& state) const;
    ChildHandling appendTextContent(const SourceContent& content, ChildBuildContext& context) const;
    ChildHandling consumeFlowBreak(const SourceNode& childNode, ChildBuildContext& context) const;
    ChildHandling consumeScopedElement(const SourceNode& childNode, ChildBuildContext& context) const;
    ChildHandling consumeChildContainer(const SourceNode& childNode, ChildBuildContext& context) const;
    ChildHandling buildRegularChild(const SourceNode& childNode, ChildBuildContext& context) const;
    std::unique_ptr<Element> createResourceElement(const ResourceId& id, BuildState& state) const;
    const SourceDocument* loadDocument(const ResourceId& id, BuildState& state, bool required) const;
    void loadElementDefaults(const std::string& elementName, BuildState& state) const;
    void validateElementScope(Element& scope, BuildState& state, const std::string& sourceName, bool includeRootInIdScope = true) const;

    const ResourceProvider* mProvider = nullptr;
    mutable std::mutex mDocumentCacheMutex;
    mutable std::map<ResourceId, CachedDocument> mDocumentCache;
};
} // namespace radia::ui
