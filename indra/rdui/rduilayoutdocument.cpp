#include "linden_common.h"
#include "rduilayoutdocument.h"
#include "rduischema.h"
#include <expat.h>
#include <limits>

namespace rdui
{
    const LayoutAttribute* LayoutElement::attribute(const std::string& name) const
    {
        const std::string key = schemaNameKey(name);
        const auto found = mNode.attributes.find(key);
        if (found != mNode.attributes.end()) return &found->second;
        if (!mDefaults) return nullptr;
        const auto default_value = mDefaults->attributes.find(key);
        return default_value == mDefaults->attributes.end() ? nullptr : &default_value->second;
    }

    namespace
    {
        struct ParserState
        {
            XML_Parser parser = nullptr;
            DiagnosticResult* result = nullptr;
            std::unique_ptr<LayoutDocument> document;
            std::vector<LayoutNode*> elements;
        };

        SourceLocation currentLocation(XML_Parser parser)
        {
            return {
                static_cast<std::size_t>(XML_GetCurrentLineNumber(parser)),
                static_cast<std::size_t>(XML_GetCurrentColumnNumber(parser)) + 1,
                static_cast<std::size_t>(XML_GetCurrentByteIndex(parser)),
            };
        }

        SourceLocation afterText(SourceLocation location, const XML_Char* text, int length)
        {
            for (int index = 0; index < length; ++index)
            {
                ++location.offset;
                if (text[index] == '\n')
                {
                    ++location.line;
                    location.column = 1;
                }
                else ++location.column;
            }
            return location;
        }

        void XMLCALL startElement(void* user_data, const XML_Char* name, const XML_Char** attributes)
        {
            auto& state = *static_cast<ParserState*>(user_data);
            auto node = std::make_unique<LayoutNode>();
            node->name = name;
            node->source.begin = currentLocation(state.parser);
            for (const XML_Char** attribute = attributes; attribute && *attribute; attribute += 2)
            {
                LayoutAttribute value;
                value.authored_name = attribute[0];
                value.value = attribute[1] ? attribute[1] : "";
                value.source.begin = node->source.begin;
                value.source.end = node->source.begin;
                const std::string key = schemaNameKey(value.authored_name);
                if (!node->attributes.emplace(key, std::move(value)).second && state.result)
                {
                    state.result->error("view.attribute.duplicate",
                                        "Attributes differing only by ASCII case are duplicate declarations.",
                                        state.document->source, node->source.begin.line, node->source.begin.column);
                }
            }

            LayoutNode* next = node.get();
            if (state.elements.empty()) state.document->root = std::move(node);
            else
            {
                LayoutContent content;
                content.source.begin = next->source.begin;
                content.element = std::move(node);
                state.elements.back()->content.push_back(std::move(content));
            }
            state.elements.push_back(next);
        }

        void XMLCALL endElement(void* user_data, const XML_Char*)
        {
            auto& state = *static_cast<ParserState*>(user_data);
            if (state.elements.empty()) return;
            LayoutNode* node = state.elements.back();
            node->source.end = currentLocation(state.parser);
            state.elements.pop_back();
            if (!state.elements.empty() && !state.elements.back()->content.empty())
                state.elements.back()->content.back().source.end = node->source.end;
        }

        void XMLCALL appendText(void* user_data, const XML_Char* text, int length)
        {
            auto& state = *static_cast<ParserState*>(user_data);
            if (state.elements.empty() || length <= 0) return;
            LayoutNode& parent = *state.elements.back();
            const SourceLocation begin = currentLocation(state.parser);
            if (!parent.content.empty() && parent.content.back().isText())
            {
                parent.content.back().text.append(text, static_cast<std::size_t>(length));
                parent.content.back().source.end = afterText(begin, text, length);
                return;
            }
            LayoutContent content;
            content.source.begin = begin;
            content.source.end = afterText(begin, text, length);
            content.text.assign(text, static_cast<std::size_t>(length));
            parent.content.push_back(std::move(content));
        }
    }

    LayoutDocumentParseResult LayoutDocumentParser::parse(const std::string& xml, const std::string& source) const
    {
        LayoutDocumentParseResult result;
        if (xml.size() > static_cast<std::size_t>(std::numeric_limits<int>::max()))
        {
            result.error("view.xml.invalid", "Radia UI XML is too large to parse.", source);
            return result;
        }

        ParserState state;
        state.document = std::make_unique<LayoutDocument>();
        state.document->source = source;
        state.result = &result;
        state.parser = XML_ParserCreate(nullptr);
        if (!state.parser)
        {
            result.error("view.xml.invalid", "Could not create the Radia UI XML parser.", source);
            return result;
        }

        XML_SetUserData(state.parser, &state);
        XML_SetElementHandler(state.parser, startElement, endElement);
        XML_SetCharacterDataHandler(state.parser, appendText);
        const bool parsed = XML_Parse(state.parser, xml.data(), static_cast<int>(xml.size()), XML_TRUE) == XML_STATUS_OK;
        if (!parsed)
        {
            const XML_Error error = XML_GetErrorCode(state.parser);
            result.error("view.xml.invalid",
                         "Could not parse Radia UI XML: " + std::string(XML_ErrorString(error)) + ".",
                         source,
                         static_cast<std::size_t>(XML_GetCurrentLineNumber(state.parser)),
                         static_cast<std::size_t>(XML_GetCurrentColumnNumber(state.parser)) + 1);
        }
        XML_ParserFree(state.parser);

        if (parsed && state.document->root) result.document = std::move(state.document);
        else if (parsed) result.error("view.xml.invalid", "Radia UI XML must contain one root element.", source);
        return result;
    }
}
