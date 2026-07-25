#include "linden_common.h"
#include "rduiinlinecontent.h"
#include "rduischema.h"

namespace rdui
{
    InlineContentNode InlineContentNode::text(TextValue value)
    {
        InlineContentNode result(InlineContentKind::Text);
        result.mValue = std::move(value);
        return result;
    }

    InlineContentNode InlineContentNode::container(InlineContentKind kind, std::vector<InlineContentNode> children)
    {
        llassert_always(kind == InlineContentKind::B || kind == InlineContentKind::I || kind == InlineContentKind::S);
        InlineContentNode result(kind);
        result.mChildren = std::move(children);
        return result;
    }

    InlineContentNode InlineContentNode::kbd(std::string binding, KeybindingPresentation presentation)
    {
        InlineContentNode result(InlineContentKind::Kbd);
        result.mMetadata = std::move(binding);
        result.mKeybindingPresentation = std::move(presentation);
        return result;
    }

    InlineContentNode InlineContentNode::br()
    {
        return InlineContentNode(InlineContentKind::Br);
    }

    InlineContentNode InlineContentNode::link(std::string destination, std::vector<InlineContentNode> children)
    {
        InlineContentNode result(InlineContentKind::Link);
        result.mMetadata = std::move(destination);
        result.mChildren = std::move(children);
        return result;
    }

    InlineContent InlineContent::text(std::string value)
    {
        return text(TextValue::literal(std::move(value)));
    }

    InlineContent InlineContent::text(TextValue value)
    {
        std::vector<InlineContentNode> nodes;
        nodes.push_back(InlineContentNode::text(std::move(value)));
        return InlineContent(std::move(nodes));
    }

    namespace
    {
        InlineContentNode resolveNode(const InlineContentNode& node,
                                      const std::function<std::string(const std::string&)>& resolve)
        {
            TextValue value = node.value();
            if (value.localized()) value.updateLocalizedValue(resolve(value.localizationKey()));

            std::vector<InlineContentNode> children;
            children.reserve(node.children().size());
            for (const InlineContentNode& child : node.children()) children.push_back(resolveNode(child, resolve));

            switch (node.kind())
            {
                case InlineContentKind::Text: return InlineContentNode::text(std::move(value));
                case InlineContentKind::B:
                case InlineContentKind::I:
                case InlineContentKind::S: return InlineContentNode::container(node.kind(), std::move(children));
                case InlineContentKind::Kbd:
                    return InlineContentNode::kbd(node.metadata(), node.keybindingPresentation());
                case InlineContentKind::Br: return InlineContentNode::br();
                case InlineContentKind::Link: return InlineContentNode::link(node.metadata(), std::move(children));
            }
            llassert(false);
            return InlineContentNode::br();
        }
    }

    InlineContent InlineContent::resolveLocalized(const std::function<std::string(const std::string&)>& resolve) const
    {
        std::vector<InlineContentNode> nodes;
        nodes.reserve(mNodes.size());
        for (const InlineContentNode& node : mNodes) nodes.push_back(resolveNode(node, resolve));
        return InlineContent(std::move(nodes));
    }

    InlineContent InlineContent::resolveKeybindings(
        const std::function<KeybindingPresentation(const std::string&)>& resolve) const
    {
        const auto resolve_node = [&](auto&& self, const InlineContentNode& node) -> InlineContentNode
        {
            std::vector<InlineContentNode> children;
            children.reserve(node.children().size());
            for (const InlineContentNode& child : node.children()) children.push_back(self(self, child));

            switch (node.kind())
            {
                case InlineContentKind::Text: return InlineContentNode::text(node.value());
                case InlineContentKind::B:
                case InlineContentKind::I:
                case InlineContentKind::S: return InlineContentNode::container(node.kind(), std::move(children));
                case InlineContentKind::Kbd:
                    return InlineContentNode::kbd(node.metadata(), resolve(node.metadata()));
                case InlineContentKind::Br: return InlineContentNode::br();
                case InlineContentKind::Link: return InlineContentNode::link(node.metadata(), std::move(children));
            }
            return InlineContentNode::br();
        };

        std::vector<InlineContentNode> nodes;
        nodes.reserve(mNodes.size());
        for (const InlineContentNode& node : mNodes) nodes.push_back(resolve_node(resolve_node, node));
        return InlineContent(std::move(nodes));
    }

    const char* inlineContentElement(InlineContentKind kind)
    {
        switch (kind)
        {
            case InlineContentKind::Text: return "";
            case InlineContentKind::B: return "b";
            case InlineContentKind::I: return "i";
            case InlineContentKind::S: return "s";
            case InlineContentKind::Kbd: return "kbd";
            case InlineContentKind::Br: return "br";
            case InlineContentKind::Link: return "link";
        }
        return "";
    }

    bool inlineContentKind(const std::string& element, InlineContentKind& kind)
    {
        const std::string lookup = schemaNameKey(element);
        for (InlineContentKind candidate : {InlineContentKind::B, InlineContentKind::I, InlineContentKind::S,
                                            InlineContentKind::Kbd, InlineContentKind::Br, InlineContentKind::Link})
        {
            if (lookup != schemaNameKey(inlineContentElement(candidate))) continue;
            kind = candidate;
            return true;
        }
        return false;
    }

    bool isInlineStyleElement(const std::string& element)
    {
        return schemaNameKey(element) == schemaNameKey(inlineContentElement(InlineContentKind::Kbd));
    }

}
