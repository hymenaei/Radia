/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */
#pragma once

#include <cstdint>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>
#include "layout/buildresult.h"

namespace radia::ui {
class Element;
class LayoutBuildContext;
class ResourceProvider;
struct ElementBuildInput;
struct ElementDefinition;
struct SourceContent;
struct SourceDocument;
struct SourceNode;

class LayoutResourceCompiler final {
public:
    explicit LayoutResourceCompiler(const ResourceProvider* resources = nullptr);

    LayoutBuildResult buildElementTreeFromResource(const std::string& resourceId, const LayoutBuildContext* context = nullptr) const;
    LayoutBuildResult buildElementTreeFromString(const std::string& xml, const std::string& sourceName = {},
                                                 const LayoutBuildContext* context = nullptr) const;
    DiagnosticResult validateElementDefaults(const std::string& element, const LayoutBuildContext* context = nullptr) const;

private:
    struct CachedDocument {
        bool initialized = false;
        std::string source;
        std::shared_ptr<const SourceDocument> document;
        std::vector<Diagnostic> warnings;
        std::vector<Diagnostic> errors;
    };
    struct BuildState;
    struct ChildBuildContext;
    enum class ChildHandling : uint8_t { Unhandled, Handled };

    static std::string normalizeResource(std::string resourceId);
    std::unique_ptr<Element> buildDocument(const SourceDocument& document, std::unique_ptr<Element> root, BuildState& state) const;
    std::unique_ptr<Element> buildNode(const SourceNode& sourceNode, const std::string& source, std::unique_ptr<Element> element,
                                       BuildState& state) const;
    const ElementDefinition* lookupElementDefinition(const SourceNode& sourceNode, const std::string& source, BuildState& state) const;
    bool resolveElementResource(const ElementBuildInput& input, const ElementDefinition& definition, std::unique_ptr<Element>& element,
                                BuildState& state) const;
    void buildChildren(Element& target, const SourceNode& node, const ElementDefinition& definition, const std::string& source,
                       BuildState& state) const;
    ChildHandling appendTextContent(const SourceContent& content, ChildBuildContext& context) const;
    ChildHandling consumeFlowBreak(const SourceNode& childNode, ChildBuildContext& context) const;
    ChildHandling consumeScopedElement(const SourceNode& childNode, ChildBuildContext& context) const;
    ChildHandling consumeChildContainer(const SourceNode& childNode, ChildBuildContext& context) const;
    ChildHandling buildRegularChild(const SourceNode& childNode, ChildBuildContext& context) const;
    std::unique_ptr<Element> createResourceElement(const std::string& resourceId, BuildState& state) const;
    const SourceDocument* loadDocument(const std::string& resourceId, BuildState& state, bool required) const;
    void loadElementDefaults(const std::string& element, BuildState& state) const;
    void validateElementScope(Element& scope, BuildState& state, const std::string& source, bool includeRootInIdScope = true) const;

    const ResourceProvider* mResources = nullptr;
    mutable std::mutex mDocumentCacheMutex;
    mutable std::unordered_map<std::string, CachedDocument> mDocumentCache;
};
} // namespace radia::ui
