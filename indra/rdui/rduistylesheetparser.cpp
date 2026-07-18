#include "linden_common.h"
#include "rduistylecompiler.h"
#include "rduicolor.h"
#include "rduischema.h"
#include "rduistylesheet.h"
#include "rduiviewcontract.h"
#include <algorithm>
#include <cctype>
#include <cmath>
#include <functional>
#include <iterator>
#include <limits>

namespace rdui
{
    namespace
    {
        std::string trim(const std::string& value)
        {
            std::size_t begin = 0;
            while (begin < value.size() && std::isspace(static_cast<unsigned char>(value[begin]))) ++begin;
            std::size_t end = value.size();
            while (end > begin && std::isspace(static_cast<unsigned char>(value[end - 1]))) --end;
            return value.substr(begin, end - begin);
        }

        bool startsWith(const std::string& value, const std::string& prefix)
        {
            return value.rfind(prefix, 0) == 0;
        }

        bool isColorValue(const std::string& value)
        {
            return isColorSyntax(value) || startsWith(value, "var(") || startsWith(value, "token(");
        }

        bool isSupportedState(const std::string& state)
        {
            return state.empty() || state == "hover" || state == "active" || state == "focus"
                || state == "focus-visible" || state == "disabled" || state == "checked"
                || state == "minimized";
        }

        std::optional<WidgetState> targetSpecificState(const std::string& state)
        {
            if (state == "checked") return WidgetState::Checked;
            if (state == "minimized") return WidgetState::Minimized;
            return std::nullopt;
        }

        std::size_t matchingClose(const std::string& css, std::size_t open)
        {
            int depth = 0;
            for (std::size_t i = open; i < css.size(); ++i)
            {
                if (css[i] == '{') ++depth;
                else if (css[i] == '}' && --depth == 0) return i;
            }
            return std::string::npos;
        }

        std::pair<std::size_t, std::size_t> sourcePosition(const std::string& source, std::size_t offset)
        {
            std::size_t line = 1;
            std::size_t column = 1;
            for (std::size_t index = 0; index < offset && index < source.size(); ++index)
            {
                if (source[index] == '\n')
                {
                    ++line;
                    column = 1;
                }
                else ++column;
            }
            return {line, column};
        }

        std::optional<std::string> normalizeImportPath(const std::string& current,
                                                       const std::string& requested)
        {
            if (requested.empty() || requested.front() == '/' || requested.find('\\') != std::string::npos
                || requested.find(':') != std::string::npos || requested.find("//") != std::string::npos)
            {
                return std::nullopt;
            }

            std::vector<std::string> segments;
            const std::size_t slash = current.rfind('/');
            const std::string combined = (slash == std::string::npos ? std::string() : current.substr(0, slash + 1)) + requested;
            std::size_t start = 0;
            while (start <= combined.size())
            {
                const std::size_t end = combined.find('/', start);
                const std::string segment = combined.substr(start, end == std::string::npos ? std::string::npos : end - start);
                if (segment.empty() || segment == ".")
                {
                    if (segment.empty() && start != combined.size()) return std::nullopt;
                }
                else if (segment == "..")
                {
                    if (segments.empty()) return std::nullopt;
                    segments.pop_back();
                }
                else segments.push_back(segment);
                if (end == std::string::npos) break;
                start = end + 1;
            }
            if (segments.empty()) return std::nullopt;

            std::string result;
            for (const std::string& segment : segments)
            {
                if (!result.empty()) result += '/';
                result += segment;
            }
            constexpr const char* extension = ".radia";
            if (result.size() < 6 || result.compare(result.size() - 6, 6, extension) != 0)
                return std::nullopt;
            return result;
        }

        std::string importChain(const std::vector<std::string>& stack,
                                const std::optional<std::string>& tail = std::nullopt)
        {
            std::string chain;
            for (const std::string& resource : stack)
            {
                if (!chain.empty()) chain += " -> ";
                chain += resource;
            }
            if (tail)
            {
                if (!chain.empty()) chain += " -> ";
                chain += *tail;
            }
            return chain;
        }

        StyleSelector mergeSelector(const StyleSelector& parent, const StyleSelector& child)
        {
            StyleSelector result;
            result.universal = child.universal ? true : parent.universal;
            result.element = child.element.empty() ? parent.element : child.element;
            result.id = child.id.empty() ? parent.id : child.id;
            result.class_name = child.class_name.empty() ? parent.class_name : child.class_name;
            result.state = parent.state;
            result.part_state = parent.part_state;
            if (!child.state.empty())
            {
                if (parent.parts.empty()) result.state = child.state;
                else result.part_state = child.state;
            }
            if (!child.part_state.empty()) result.part_state = child.part_state;
            result.parts = parent.parts;
            result.parts.insert(result.parts.end(), child.parts.begin(), child.parts.end());
            return result;
        }

        void appendSelector(StyleRule& destination, const StyleRule& suffix, SelectorCombinator combinator)
        {
            if (suffix.selectors.empty()) return;
            destination.combinators.push_back(combinator);
            destination.selectors.insert(destination.selectors.end(), suffix.selectors.begin(), suffix.selectors.end());
            destination.combinators.insert(destination.combinators.end(), suffix.combinators.begin(), suffix.combinators.end());
        }

        StyleRule expandNestedSelector(const StyleRule& parent, const std::string& raw_selector)
        {
            const std::string selector = trim(raw_selector);
            StyleRule result = parent;
            if (selector.empty()) return {};

            if (selector.front() != '&')
            {
                const bool child = selector.front() == '>';
                const std::string suffix = trim(selector.substr(child ? 1 : 0));
                appendSelector(result, detail::parseSelector(suffix), child ? SelectorCombinator::Child : SelectorCombinator::Descendant);
                return result;
            }

            const std::string tail = selector.substr(1);
            if (tail.empty()) return result;
            if (std::isspace(static_cast<unsigned char>(tail.front())) || tail.front() == '>')
            {
                const std::string trimmed_tail = trim(tail);
                const bool child = !trimmed_tail.empty() && trimmed_tail.front() == '>';
                const std::string suffix = trim(trimmed_tail.substr(child ? 1 : 0));
                appendSelector(result, detail::parseSelector(suffix), child ? SelectorCombinator::Child : SelectorCombinator::Descendant);
                return result;
            }

            std::size_t split = 0;
            while (split < tail.size() && !std::isspace(static_cast<unsigned char>(tail[split])) && tail[split] != '>') ++split;
            const StyleRule continuation = detail::parseSelector(tail.substr(0, split));
            if (!continuation.selectors.empty() && !result.selectors.empty())
                result.selectors.back() = mergeSelector(result.selectors.back(), continuation.selectors.front());
            if (split < tail.size())
            {
                const std::string remainder = trim(tail.substr(split));
                const bool child = !remainder.empty() && remainder.front() == '>';
                appendSelector(result, detail::parseSelector(trim(remainder.substr(child ? 1 : 0))),
                               child ? SelectorCombinator::Child : SelectorCombinator::Descendant);
            }
            return result;
        }

        StyleSelector parseSimpleSelector(const std::string& selector)
        {
            StyleSelector rule;
            std::string token = trim(selector);

            if (const std::size_t separator = token.find("::"); separator != std::string::npos)
            {
                std::string part_token = token.substr(separator + 2);
                if (const std::size_t state_separator = part_token.rfind(':'); state_separator != std::string::npos
                    && (state_separator == 0 || part_token[state_separator - 1] != ':'))
                {
                    rule.part_state = part_token.substr(state_separator + 1);
                    part_token.erase(state_separator);
                }
                rule.parts = detail::splitPartPath(part_token);
                token.erase(separator);
            }
            if (const std::size_t separator = token.find(':'); separator != std::string::npos)
            {
                rule.state = token.substr(separator + 1);
                token.erase(separator);
            }
            if (const std::size_t separator = token.find('#'); separator != std::string::npos)
            {
                rule.id = token.substr(separator + 1);
                token.erase(separator);
            }
            if (const std::size_t separator = token.find('.'); separator != std::string::npos)
            {
                rule.class_name = token.substr(separator + 1);
                token.erase(separator);
            }
            rule.universal = token == "*";
            if (!rule.universal) rule.element = token;
            return rule;
        }
    }

    std::vector<std::string> detail::splitPartPath(const std::string& part)
    {
        std::vector<std::string> result;
        std::size_t start = 0;
        while (start < part.size())
        {
            const std::size_t separator = part.find("::", start);
            const std::string segment = trim(part.substr(start, separator == std::string::npos ? std::string::npos : separator - start));
            if (!segment.empty()) result.push_back(segment);
            if (separator == std::string::npos) break;
            start = separator + 2;
        }
        return result;
    }

    StyleRule detail::parseSelector(const std::string& selector)
    {
        StyleRule rule;
        const std::string input = trim(selector);
        if (input.empty() || input == ":root") return rule;

        std::size_t position = 0;
        SelectorCombinator pending = SelectorCombinator::Descendant;
        while (position < input.size())
        {
            while (position < input.size() && std::isspace(static_cast<unsigned char>(input[position])))
            {
                ++position;
            }
            if (position >= input.size()) break;
            if (input[position] == '>')
            {
                pending = SelectorCombinator::Child;
                ++position;
                continue;
            }

            const std::size_t start = position;
            while (position < input.size() && !std::isspace(static_cast<unsigned char>(input[position])) && input[position] != '>') ++position;
            if (!rule.selectors.empty()) rule.combinators.push_back(pending);
            rule.selectors.push_back(parseSimpleSelector(input.substr(start, position - start)));
            pending = SelectorCombinator::Descendant;
        }
        return rule;
    }

    StyleSheetLoadResult StyleSheet::loadCss(const std::string& css, const std::string& source_name)
    {
        return loadCssLayers({ResourceLayer{source_name, css}});
    }

    StyleSheetLoadResult StyleSheet::loadCssLayers(const std::vector<ResourceLayer>& layers)
    {
        Impl candidate;
        StyleSheetLoadResult result;
        if (layers.empty())
        {
            result.error("stylesheet.layers.empty", "No stylesheet layers were provided.");
            return result;
        }
        for (const ResourceLayer& layer : layers)
        {
            // Collect tokens from every reachable module before declarations are
            // compiled. This lets a conventional top-of-file @import consume
            // tokens declared by its owning entrypoint without exposing tokens
            // from unrelated or later Skin layers.
            std::vector<std::string> token_stack;
            std::function<void(const std::string&, const std::string&, const std::string&)> collectTokens;
            collectTokens = [&](const std::string& source, const std::string& resource_id,
                                const std::string& source_name)
            {
                if (std::find(token_stack.begin(), token_stack.end(), resource_id) != token_stack.end()) return;
                token_stack.push_back(resource_id);

                std::size_t position = 0;
                while (position < source.size() && !result.hasErrors())
                {
                    while (position < source.size() && std::isspace(static_cast<unsigned char>(source[position]))) ++position;
                    if (position == source.size()) break;
                    if (source.compare(position, 7, "@import") == 0
                        && (position + 7 == source.size()
                            || std::isspace(static_cast<unsigned char>(source[position + 7]))))
                    {
                        const std::size_t semicolon = source.find(';', position + 7);
                        if (semicolon == std::string::npos) break;
                        const std::string argument = trim(source.substr(position + 7, semicolon - position - 7));
                        if (argument.size() < 2 || (argument.front() != '"' && argument.front() != '\'')
                            || argument.back() != argument.front()) break;
                        const std::optional<std::string> imported_id =
                            normalizeImportPath(resource_id, argument.substr(1, argument.size() - 2));
                        if (!imported_id) break;
                        const std::string imported_name = layer.sourceNameFor(*imported_id);
                        if (*imported_id == layer.entrypoint)
                            collectTokens(layer.source, *imported_id, layer.source_name);
                        else if (const auto imported = layer.modules.find(*imported_id); imported != layer.modules.end())
                            collectTokens(imported->second, *imported_id, imported_name);
                        position = semicolon + 1;
                        continue;
                    }
                    if (source[position] == '@') break;

                    const std::size_t open = source.find('{', position);
                    if (open == std::string::npos) break;
                    const std::size_t close = matchingClose(source, open);
                    if (close == std::string::npos) break;
                    if (trim(source.substr(position, open - position)) == ":root")
                        candidate.parseBlock(":root", source.substr(open + 1, close - open - 1), {}, result, source_name);
                    position = close + 1;
                }
                token_stack.pop_back();
            };

            const std::string entrypoint = layer.entrypoint.empty() ? layer.source_name : layer.entrypoint;
            collectTokens(layer.source, entrypoint, layer.source_name);

            std::vector<std::string> import_stack;
            std::function<void(const std::string&, const std::string&, const std::string&)> parseModule;
            parseModule = [&](const std::string& source, const std::string& resource_id,
                              const std::string& source_name)
            {
                if (std::find(import_stack.begin(), import_stack.end(), resource_id) != import_stack.end())
                {
                    result.error("stylesheet.import.cycle",
                                 "Cyclic @import: " + importChain(import_stack, resource_id) + ".",
                                 source_name);
                    return;
                }
                const std::size_t first_error = result.errors.size();
                import_stack.push_back(resource_id);

                bool saw_rule = false;
                std::size_t position = 0;
                while (position < source.size() && !result.hasErrors())
                {
                    while (position < source.size() && std::isspace(static_cast<unsigned char>(source[position]))) ++position;
                    if (position == source.size()) break;

                    if (source.compare(position, 7, "@import") == 0
                        && (position + 7 == source.size()
                            || std::isspace(static_cast<unsigned char>(source[position + 7]))))
                    {
                        const auto [line, column] = sourcePosition(source, position);
                        if (saw_rule)
                        {
                            result.error("stylesheet.import.order", "@import must precede all rules in its module.",
                                         source_name, line, column);
                            break;
                        }
                        const std::size_t semicolon = source.find(';', position + 7);
                        if (semicolon == std::string::npos)
                        {
                            result.error("stylesheet.import.syntax", "@import requires a quoted path followed by ';'.",
                                         source_name, line, column);
                            break;
                        }
                        const std::string argument = trim(source.substr(position + 7, semicolon - position - 7));
                        if (argument.size() < 2 || (argument.front() != '"' && argument.front() != '\'')
                            || argument.back() != argument.front())
                        {
                            result.error("stylesheet.import.syntax", "@import requires exactly one quoted path.",
                                         source_name, line, column);
                            break;
                        }
                        const std::string requested = argument.substr(1, argument.size() - 2);
                        const std::optional<std::string> imported_id = normalizeImportPath(resource_id, requested);
                        if (!imported_id)
                        {
                            result.error("stylesheet.import.path_invalid",
                                         "Invalid or escaping @import path: " + requested + ".",
                                         source_name, line, column);
                            break;
                        }

                        const std::string imported_name = layer.sourceNameFor(*imported_id);
                        candidate.dependencies[source_name].insert(imported_name);
                        if (std::find(import_stack.begin(), import_stack.end(), *imported_id) != import_stack.end())
                            result.error("stylesheet.import.cycle",
                                         "Cyclic @import: " + importChain(import_stack, *imported_id) + ".",
                                         source_name, line, column);
                        else if (*imported_id == layer.entrypoint)
                            parseModule(layer.source, *imported_id, layer.source_name);
                        else if (const auto imported = layer.modules.find(*imported_id); imported != layer.modules.end())
                            parseModule(imported->second, *imported_id, imported_name);
                        else
                            result.error("stylesheet.import.missing",
                                         "Imported stylesheet module is missing: " + requested
                                             + ". Import chain: " + importChain(import_stack, *imported_id) + ".",
                                         source_name, line, column);
                        position = semicolon + 1;
                        continue;
                    }
                    if (source[position] == '@')
                    {
                        const auto [line, column] = sourcePosition(source, position);
                        result.error("stylesheet.at_rule.unsupported", "Unsupported stylesheet at-rule.",
                                     source_name, line, column);
                        break;
                    }

                    saw_rule = true;
                    const std::size_t open = source.find('{', position);
                    if (open == std::string::npos)
                    {
                        result.error("stylesheet.syntax.trailing_content", "Unexpected content outside a rule: " + trim(source.substr(position)) + ".", source_name);
                        break;
                    }
                    const std::string selector = trim(source.substr(position, open - position));
                    if (selector.empty())
                    {
                        result.error("stylesheet.selector.empty", "Rule selector is empty.", source_name);
                        break;
                    }
                    const std::size_t close = matchingClose(source, open);
                    if (close == std::string::npos)
                    {
                        result.error("stylesheet.syntax.unclosed_block", "Rule block is not closed: " + selector + ".", source_name);
                        break;
                    }
                    candidate.parseBlock(selector, source.substr(open + 1, close - open - 1), {}, result, source_name);
                    position = close + 1;
                }
                if (import_stack.size() > 1)
                {
                    for (std::size_t index = first_error; index < result.errors.size(); ++index)
                    {
                        if (result.errors[index].message.find("Import chain:") == std::string::npos
                            && result.errors[index].code != "stylesheet.import.cycle")
                        {
                            result.errors[index].message += " Import chain: " + importChain(import_stack) + ".";
                        }
                    }
                }
                import_stack.pop_back();
            };

            parseModule(layer.source, entrypoint, layer.source_name);
        }
        if (result.ok())
        {
            candidate.generation = mImpl->generation + 1;
            *mImpl = std::move(candidate);
        }
        return result;
    }

    void StyleSheet::Impl::parseBlock(const std::string& selector_text,
                           const std::string& body,
                           const StyleRule& parent,
                           StyleSheetLoadResult& result,
                           const std::string& source_name)
    {
        std::vector<std::string> selectors;
        std::size_t selector_start = 0;
        while (selector_start <= selector_text.size())
        {
            const std::size_t comma = selector_text.find(',', selector_start);
            selectors.push_back(trim(selector_text.substr(selector_start, comma == std::string::npos ? std::string::npos : comma - selector_start)));
            if (comma == std::string::npos) break;
            selector_start = comma + 1;
        }
        if (selectors.size() > 1)
        {
            for (const std::string& selector : selectors)
                if (!selector.empty()) parseBlock(selector, body, parent, result, source_name);
            return;
        }

        const std::string selector = trim(selector_text);
        const bool nested = !parent.selectors.empty();
        StyleRule rule = nested ? expandNestedSelector(parent, selector) : detail::parseSelector(selector);
        const bool root_rule = !nested && selector == ":root";
        if (!root_rule && rule.selectors.empty())
        {
            result.error("stylesheet.selector.empty", "Rule selector is empty.", source_name);
            return;
        }

        for (std::size_t index = 0; index < rule.selectors.size(); ++index)
        {
            StyleSelector& component = rule.selectors[index];
            const bool declaration_component = index + 1 == rule.selectors.size();
            if ((!component.id.empty() && !isLocalIdentifier(component.id))
                || (!component.class_name.empty() && !isLocalIdentifier(component.class_name)))
            {
                result.error("stylesheet.selector.identifier_invalid",
                             "Widget IDs and classes in selectors must use lowercase kebab-case: " + selector + ".",
                             source_name);
                return;
            }
            if (std::any_of(component.parts.begin(), component.parts.end(),
                            [](const std::string& part) { return !isLocalIdentifier(part); }))
            {
                result.error("stylesheet.selector.part_invalid",
                             "Widget parts in selectors must use lowercase kebab-case: " + selector + ".",
                             source_name);
                return;
            }
            if (!isSupportedState(component.state))
            {
                result.error("stylesheet.selector.state_unknown", "Unknown selector state: " + component.state + ".", source_name);
                return;
            }
            if (!isSupportedState(component.part_state))
            {
                result.error("stylesheet.selector.state_unknown", "Unknown selector state: " + component.part_state + ".", source_name);
                return;
            }
            if (!declaration_component && !component.parts.empty())
            {
                result.error("stylesheet.selector.part_structural", "Widget parts cannot participate in structural combinators: " + selector + ".", source_name);
                return;
            }
            if (component.element.empty())
            {
                if (!component.parts.empty() || !component.state.empty() || !component.part_state.empty())
                {
                    result.error("stylesheet.selector.target_required",
                                 "Parts and states require an element-qualified selector: " + selector + ".", source_name);
                    return;
                }
                continue;
            }

            const WidgetContract* component_owner = findWidgetContract(component.element);
            if (!component_owner)
            {
                result.error("stylesheet.selector.element_unknown", "Unknown widget element: " + component.element + ".", source_name);
                return;
            }
            component.element = component_owner->element;
            const CompositePartContract* component_part = component.parts.empty()
                                                       ? nullptr
                                                       : findCompositePartContract(*component_owner, component.parts);
            if (!component.parts.empty() && !component_part)
            {
                std::string part;
                for (const std::string& segment : component.parts)
                {
                    if (!part.empty()) part += "::";
                    part += segment;
                }
                result.error("stylesheet.selector.part_unknown",
                             "Unknown style part for " + component.element + ": " + part + ".", source_name);
                return;
            }
            if (const auto state = targetSpecificState(component.state);
                state && !producesState(*component_owner, *state))
            {
                result.warning("stylesheet.selector.state_never_matches",
                               "State :" + component.state + " is never produced by " + component.element + ".",
                               source_name);
            }
            if (const auto state = targetSpecificState(component.part_state);
                state && component_part && !producesState(*component_part, *state))
            {
                result.warning("stylesheet.selector.state_never_matches",
                               "State :" + component.part_state + " is never produced by the selected part of "
                                   + component.element + ".",
                               source_name);
            }
        }

        std::vector<StyleDeclaration> declarations;
        auto addDeclaration = [&](const std::string& declaration)
        {
            const std::size_t colon = declaration.find(':');
            if (colon == std::string::npos)
            {
                result.error("stylesheet.declaration.invalid", "Declaration requires a property and value: " + trim(declaration) + ".", source_name);
                return;
            }
            const std::string name = trim(declaration.substr(0, colon));
            const std::string value = trim(declaration.substr(colon + 1));
            if (name.empty() || value.empty())
            {
                result.error("stylesheet.declaration.invalid", "Declaration property and value must not be empty.", source_name);
                return;
            }
            if (startsWith(name, "--"))
            {
                if (!root_rule)
                {
                    result.error("stylesheet.token.root_required", "Style Tokens may be declared only in :root: " + name + ".", source_name);
                    return;
                }
                if (isColorValue(value))
                {
                    const Color marker(-1.f, -1.f, -1.f, -1.f);
                    const Color parsed = parseColorValue(value, marker);
                    if (parsed.a < 0.f) result.error("stylesheet.token.value_invalid", "Invalid color token value for " + name + ": " + value + ".", source_name);
                    else setColorToken(name, parsed);
                }
                else
                {
                    const float parsed = parseNumberValue(value, std::numeric_limits<float>::quiet_NaN());
                    if (!std::isfinite(parsed)) result.error("stylesheet.token.value_invalid", "Invalid number token value for " + name + ": " + value + ".", source_name);
                    else setNumberToken(name, parsed);
                }
                return;
            }
            const detail::StylePropertyDescriptor* descriptor = detail::findStyleProperty(name);
            if (!descriptor)
            {
                result.error("stylesheet.property.unknown", "Unknown property: " + name + ".", source_name);
                return;
            }
            if (root_rule)
            {
                result.error("stylesheet.property.root_unsupported", "Ordinary properties are not allowed in :root: " + name + ".", source_name);
                return;
            }
            if (auto compiled = compileDeclaration(descriptor->property, value, selector, result, source_name))
            {
                declarations.insert(declarations.end(),
                                    std::make_move_iterator(compiled->begin()),
                                    std::make_move_iterator(compiled->end()));
            }
        };

        std::size_t position = 0;
        while (position < body.size())
        {
            while (position < body.size() && std::isspace(static_cast<unsigned char>(body[position]))) ++position;
            if (position >= body.size()) break;
            const std::size_t semicolon = body.find(';', position);
            const std::size_t brace = body.find('{', position);
            if (brace != std::string::npos && (semicolon == std::string::npos || brace < semicolon))
            {
                const std::size_t close = matchingClose(body, brace);
                if (close == std::string::npos)
                {
                    result.error("stylesheet.syntax.unclosed_block", "Nested rule block is not closed.", source_name);
                    break;
                }
                parseBlock(trim(body.substr(position, brace - position)), body.substr(brace + 1, close - brace - 1), rule, result, source_name);
                position = close + 1;
                continue;
            }
            if (semicolon == std::string::npos)
            {
                addDeclaration(body.substr(position));
                break;
            }
            addDeclaration(body.substr(position, semicolon - position));
            position = semicolon + 1;
        }

        if (!declarations.empty())
        {
            rule.declarations = std::move(declarations);
            addRule(rule);
        }
    }

}
