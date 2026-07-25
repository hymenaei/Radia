#include "linden_common.h"
#include "rduiinlinecontentcompiler.h"
#include "rduischema.h"
#include "rduiviewcontract.h"
#include <algorithm>
#include <cctype>
#include <functional>
#include <iterator>

namespace rdui
{
    namespace
    {
        std::string contentKey(const std::string& text)
        {
            const auto first = std::find_if_not(text.begin(), text.end(),
                [](unsigned char character) { return std::isspace(character); });
            if (first == text.end()) return {};
            const auto last = std::find_if_not(text.rbegin(), text.rend(),
                [](unsigned char character) { return std::isspace(character); }).base();
            return std::string(first, last);
        }

        bool hasAuthoredContent(const LayoutNode& node)
        {
            for (const LayoutContent& content : node.content)
                if (content.element || !contentKey(content.text).empty()) return true;
            return false;
        }
    }

    InlineContent compileInlineContent(const std::vector<LayoutContent>& content_items,
                                       const std::string& host,
                                       const std::vector<InlineContentKind>& accepted,
                                       ViewBuildResult& result,
                                       const std::string& source,
                                       const ViewBuildContext* context)
    {
        bool emitted_literal_text = false;
        bool pending_literal_space = false;
        const auto inlineText = [&](const std::string& authored, std::size_t line)
        {
            std::vector<InlineContentNode> nodes;
            if (context)
            {
                const std::string key = contentKey(authored);
                const bool has_leading_space = !authored.empty()
                    && std::isspace(static_cast<unsigned char>(authored.front()));
                const bool has_trailing_space = !authored.empty()
                    && std::isspace(static_cast<unsigned char>(authored.back()));
                if (key.empty())
                {
                    if (emitted_literal_text && (has_leading_space || has_trailing_space))
                        pending_literal_space = true;
                    return nodes;
                }
                if (emitted_literal_text && (pending_literal_space || has_leading_space))
                    nodes.push_back(InlineContentNode::text(TextValue::literal(" ")));
                nodes.push_back(InlineContentNode::text(localizedViewText(
                    key, result, source, context, line)));
                emitted_literal_text = true;
                pending_literal_space = has_trailing_space;
                return nodes;
            }

            std::string value;
            for (unsigned char character : authored)
            {
                if (std::isspace(character))
                {
                    if (emitted_literal_text) pending_literal_space = true;
                    continue;
                }
                if (pending_literal_space)
                {
                    value += ' ';
                    pending_literal_space = false;
                }
                value += static_cast<char>(character);
                emitted_literal_text = true;
            }
            if (!value.empty()) nodes.push_back(InlineContentNode::text(TextValue::literal(std::move(value))));
            return nodes;
        };
        const auto accepts = [&accepted](InlineContentKind kind)
        {
            return std::find(accepted.begin(), accepted.end(), kind) != accepted.end();
        };
        std::function<std::vector<InlineContentNode>(const std::vector<LayoutContent>&)> buildInline;
        buildInline = [&](const std::vector<LayoutContent>& content)
        {
            std::vector<InlineContentNode> nodes;
            for (const LayoutContent& item : content)
            {
                if (item.isText())
                {
                    std::vector<InlineContentNode> text_nodes = inlineText(item.text, item.source.begin.line);
                    nodes.insert(nodes.end(), std::make_move_iterator(text_nodes.begin()),
                                 std::make_move_iterator(text_nodes.end()));
                    continue;
                }

                const LayoutNode& inline_node = *item.element;
                InlineContentKind kind = InlineContentKind::Text;
                if (!inlineContentKind(inline_node.name, kind))
                {
                    result.error("view.inline.element_unknown",
                                 "Unsupported inline element in <" + host + ">: " + inline_node.name + ".",
                                 source, inline_node.source.begin.line, inline_node.source.begin.column);
                    continue;
                }
                if (!accepts(kind))
                {
                    result.error("view.inline.unsupported",
                                 "Inline <" + std::string(inlineContentElement(kind))
                                 + "> is not supported in <" + host + ">.",
                                 source, inline_node.source.begin.line, inline_node.source.begin.column);
                    continue;
                }
                if (kind == InlineContentKind::Link)
                {
                    result.error("view.inline.not_implemented",
                                 "Inline <" + std::string(inlineContentElement(kind))
                                 + "> is not implemented yet.", source,
                                 inline_node.source.begin.line, inline_node.source.begin.column);
                    continue;
                }
                if (kind == InlineContentKind::Kbd)
                {
                    const auto binding = inline_node.attributes.find("binding");
                    for (const auto& [name, attribute] : inline_node.attributes)
                        if (name != "binding")
                            result.error("view.inline.attribute_unknown",
                                         "Unknown attribute on inline <kbd>: "
                                         + attribute.authored_name + ".", source,
                                         attribute.source.begin.line, attribute.source.begin.column);
                    if (binding == inline_node.attributes.end())
                        result.error("view.inline.kbd.binding_required",
                                     "Inline <kbd> requires a binding attribute.", source,
                                     inline_node.source.begin.line, inline_node.source.begin.column);
                    else if (!isLocalIdentifier(binding->second.value))
                        result.error("view.inline.kbd.binding_invalid",
                                     "Inline <kbd> binding must be a lowercase kebab-case command id.", source,
                                     binding->second.source.begin.line, binding->second.source.begin.column);
                    if (hasAuthoredContent(inline_node))
                        result.error("view.inline.children_unsupported",
                                     "Inline <kbd> cannot contain authored content.", source,
                                     inline_node.source.begin.line, inline_node.source.begin.column);
                    if (binding != inline_node.attributes.end() && isLocalIdentifier(binding->second.value))
                    {
                        if (emitted_literal_text && pending_literal_space)
                            nodes.push_back(InlineContentNode::text(TextValue::literal(" ")));
                        nodes.push_back(InlineContentNode::kbd(binding->second.value));
                        emitted_literal_text = true;
                        pending_literal_space = false;
                    }
                    continue;
                }
                for (const auto& [name, attribute] : inline_node.attributes)
                    result.error("view.inline.attribute_unknown",
                                 "Unknown attribute on inline <" + std::string(inlineContentElement(kind))
                                 + ">: " + attribute.authored_name + ".",
                                 source, attribute.source.begin.line, attribute.source.begin.column);

                if (kind == InlineContentKind::Br)
                {
                    if (hasAuthoredContent(inline_node))
                        result.error("view.inline.children_unsupported",
                                     "Inline <br> cannot contain content.", source,
                                     inline_node.source.begin.line, inline_node.source.begin.column);
                    nodes.push_back(InlineContentNode::br());
                    emitted_literal_text = false;
                    pending_literal_space = false;
                }
                else
                {
                    if (emitted_literal_text && pending_literal_space)
                    {
                        nodes.push_back(InlineContentNode::text(TextValue::literal(" ")));
                        pending_literal_space = false;
                    }
                    nodes.push_back(InlineContentNode::container(kind, buildInline(inline_node.content)));
                }
            }
            return nodes;
        };
        return InlineContent(buildInline(content_items));
    }
}
