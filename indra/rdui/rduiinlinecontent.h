#ifndef LL_RDUI_INLINE_CONTENT_H
#define LL_RDUI_INLINE_CONTENT_H

#include "rduilocalization.h"
#include <cstdint>
#include <functional>
#include <string>
#include <vector>

namespace rdui
{
    struct KeybindingPresentation
    {
        std::vector<std::string> keys;

        bool operator==(const KeybindingPresentation& other) const { return keys == other.keys; }
    };

    enum class InlineContentKind : uint8_t
    {
        Text,
        B,
        I,
        S,
        Kbd,
        Br,
        Link,
    };

    class InlineContentNode
    {
        public:
            static InlineContentNode text(TextValue value);
            static InlineContentNode container(InlineContentKind kind, std::vector<InlineContentNode> children);
            static InlineContentNode kbd(std::string binding, KeybindingPresentation presentation = {});
            static InlineContentNode br();
            static InlineContentNode link(std::string destination, std::vector<InlineContentNode> children);

            InlineContentKind kind() const { return mKind; }
            const TextValue& value() const { return mValue; }
            const std::string& metadata() const { return mMetadata; }
            const KeybindingPresentation& keybindingPresentation() const { return mKeybindingPresentation; }
            const std::vector<InlineContentNode>& children() const { return mChildren; }

        private:
            explicit InlineContentNode(InlineContentKind kind) : mKind(kind) {}

            InlineContentKind mKind;
            TextValue mValue;
            std::string mMetadata;
            KeybindingPresentation mKeybindingPresentation;
            std::vector<InlineContentNode> mChildren;
    };

    class InlineContent
    {
        public:
            InlineContent() = default;
            explicit InlineContent(std::vector<InlineContentNode> nodes) : mNodes(std::move(nodes)) {}

            static InlineContent text(std::string value);
            static InlineContent text(TextValue value);

            const std::vector<InlineContentNode>& nodes() const { return mNodes; }
            bool empty() const { return mNodes.empty(); }
            InlineContent resolveLocalized(const std::function<std::string(const std::string&)>& resolve) const;
            InlineContent resolveKeybindings(
                const std::function<KeybindingPresentation(const std::string&)>& resolve) const;

        private:
            std::vector<InlineContentNode> mNodes;
    };

    const char* inlineContentElement(InlineContentKind kind);
    bool inlineContentKind(const std::string& element, InlineContentKind& kind);
    bool isInlineStyleElement(const std::string& element);
}

#endif // LL_RDUI_INLINE_CONTENT_H
