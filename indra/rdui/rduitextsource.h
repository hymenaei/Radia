#ifndef LL_RDUI_TEXT_SOURCE_H
#define LL_RDUI_TEXT_SOURCE_H

#include "rduiinlinecontent.h"
#include "rduilocalizationvalue.h"

#include <functional>
#include <string>
#include <variant>
#include <vector>

namespace rdui
{
    class TextSourceNode
    {
        public:
            struct Literal
            {
                std::string value;
            };

            struct Localized
            {
                LocalizationRequest request;
                InlineContent fallback;
            };

            struct Container
            {
                InlineContentKind kind;
                std::vector<TextSourceNode> children;
            };

            struct Keybinding
            {
                std::string binding;
                KeybindingPresentation presentation;
            };

            struct Break {};

            struct Link
            {
                std::string destination;
                std::vector<TextSourceNode> children;
            };

            using Value = std::variant<Literal, Localized, Container, Keybinding, Break, Link>;

            static TextSourceNode text(std::string value);
            static TextSourceNode localized(LocalizationRequest request, InlineContent fallback);
            static TextSourceNode container(InlineContentKind kind, std::vector<TextSourceNode> children);
            static TextSourceNode kbd(std::string binding, KeybindingPresentation presentation = {});
            static TextSourceNode br();
            static TextSourceNode link(std::string destination, std::vector<TextSourceNode> children);

            const Value& value() const { return mValue; }

        private:
            explicit TextSourceNode(Value value) : mValue(std::move(value)) {}

            Value mValue;
    };

    class TextSource
    {
        public:
            TextSource() = default;
            explicit TextSource(std::vector<TextSourceNode> nodes) : mNodes(std::move(nodes)) {}

            static TextSource literal(InlineContent content);
            static TextSource text(std::string value);
            static TextSource localized(LocalizationRequest request, InlineContent fallback);

            const std::vector<TextSourceNode>& nodes() const { return mNodes; }
            InlineContent materialize() const;
            InlineContent materialize(const std::function<InlineContent(const LocalizationRequest&)>& resolve) const;

        private:
            std::vector<TextSourceNode> mNodes;
    };
}

#endif // LL_RDUI_TEXT_SOURCE_H
