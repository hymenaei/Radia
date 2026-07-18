#ifndef LL_RDUI_LAYOUT_DOCUMENT_H
#define LL_RDUI_LAYOUT_DOCUMENT_H

#include "rduidiagnostic.h"
#include <cstddef>
#include <memory>
#include <string>
#include <unordered_map>
#include <vector>

namespace rdui
{
    struct SourceLocation
    {
        std::size_t line = 0;
        std::size_t column = 0;
        std::size_t offset = 0;
    };

    struct SourceRange
    {
        SourceLocation begin;
        SourceLocation end;
    };

    struct LayoutAttribute
    {
        std::string value;
        SourceRange source;
    };

    struct LayoutNode;

    struct LayoutContent
    {
        SourceRange source;
        std::string text;
        std::unique_ptr<LayoutNode> element;

        bool isText() const { return !element; }
    };

    struct LayoutNode
    {
        std::string name;
        SourceRange source;
        std::unordered_map<std::string, LayoutAttribute> attributes;
        std::vector<LayoutContent> content;
    };

    struct LayoutDocument
    {
        std::string source;
        std::unique_ptr<LayoutNode> root;
    };

    using LayoutDocumentMap = std::unordered_map<std::string, std::shared_ptr<const LayoutDocument>>;

    class LayoutElement final
    {
        public:
            explicit LayoutElement(const LayoutNode& node, const LayoutNode* defaults = nullptr)
                : mNode(node), mDefaults(defaults) {}

            const std::string& name() const { return mNode.name; }
            const SourceRange& source() const { return mNode.source; }
            const std::unordered_map<std::string, LayoutAttribute>& attributes() const { return mNode.attributes; }
            const std::vector<LayoutContent>& content() const { return mNode.content; }

            const LayoutAttribute* attribute(const std::string& name) const;

        private:
            const LayoutNode& mNode;
            const LayoutNode* mDefaults = nullptr;
    };

    struct LayoutDocumentParseResult : DiagnosticResult
    {
        std::unique_ptr<LayoutDocument> document;
        bool ok() const { return !hasErrors() && document && document->root; }
    };

    class LayoutDocumentParser final
    {
        public:
            LayoutDocumentParseResult parse(const std::string& xml, const std::string& source = {}) const;
    };
}

#endif // LL_RDUI_LAYOUT_DOCUMENT_H
