/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <cctype>
#include <cmath>
#include <cstdint>
#include <iterator>
#include <limits>
#include <map>
#include "html/elementnames.h"
#include "resource/elementdefinition.h"
#include "style/color.h"
#include "style/model.h"
#include "style/stylesheet.h"
#include "style/syntax.h"

namespace radia::ui {
namespace {
using detail::lower;
using detail::startsWith;
using detail::trim;

struct TopLevelDelimiter {
    std::size_t position = 0;
    char value = 0;
};

std::optional<TopLevelDelimiter> nextTopLevelDelimiter(const std::string& value, std::size_t start) {
    char quote = 0;
    int parentheses = 0;
    for (std::size_t index = start; index < value.size(); ++index) {
        const char character = value[index];
        if (quote) {
            if (character == quote && (index == 0 || value[index - 1] != '\\')) quote = 0;
            continue;
        }
        if (character == '\'' || character == '"') {
            quote = character;
            continue;
        }
        if (character == '(') ++parentheses;
        else if (character == ')' && parentheses > 0) --parentheses;
        else if (parentheses == 0 && (character == ';' || character == '{')) return TopLevelDelimiter{index, character};
    }
    return std::nullopt;
}

std::optional<std::size_t> topLevelDelimiter(const std::string& value, std::size_t start, char delimiter) {
    const auto found = nextTopLevelDelimiter(value, start);
    return found && found->value == delimiter ? std::optional<std::size_t>(found->position) : std::nullopt;
}

bool isColorValue(const std::string& value) {
    const std::string lowered = lower(trim(value));
    return isColorSyntax(lowered) || startsWith(lowered, "var(");
}

bool isSupportedState(const std::string& state) {
    return state.empty()
        || state == "hover"
        || state == "active"
        || state == "focus"
        || state == "focus-visible"
        || state == "disabled"
        || state == "checked"
        || state == "minimized"
        || state == "invalid"
        || state == "indeterminate";
}

bool isCSSWhitespace(char character) {
    return character == '\t' || character == '\n' || character == '\f' || character == '\r' || character == ' ';
}

bool isCSSNameStart(char character) {
    const auto value = static_cast<unsigned char>(character);
    return (value >= 'a' && value <= 'z') || (value >= 'A' && value <= 'Z') || character == '_' || value >= 0x80;
}

bool isCSSNameCodePoint(char character) {
    const auto value = static_cast<unsigned char>(character);
    return isCSSNameStart(character) || (value >= '0' && value <= '9') || character == '-';
}

bool isCSSHexDigit(char character) {
    const auto value = static_cast<unsigned char>(character);
    return (value >= '0' && value <= '9') || (value >= 'a' && value <= 'f') || (value >= 'A' && value <= 'F');
}

bool isValidCSSEscape(std::string_view value, std::size_t offset) {
    if (offset + 1 >= value.size() || value[offset] != '\\') return false;
    return value[offset + 1] != '\n' && value[offset + 1] != '\r' && value[offset + 1] != '\f';
}

std::size_t consumeCSSEscape(std::string_view value, std::size_t offset) {
    std::size_t position = offset + 1;
    if (position >= value.size() || !isCSSHexDigit(value[position])) return position + (position < value.size() ? 1 : 0);
    std::size_t digits = 0;
    while (position < value.size() && digits < 6 && isCSSHexDigit(value[position])) {
        ++position;
        ++digits;
    }
    if (position < value.size() && isCSSWhitespace(value[position])) ++position;
    return position;
}

std::size_t findUnescaped(std::string_view value, char target, std::size_t start = 0) {
    for (std::size_t position = start; position < value.size(); ++position) {
        if (value[position] == '\\' && isValidCSSEscape(value, position)) {
            position = consumeCSSEscape(value, position) - 1;
            continue;
        }
        if (value[position] == target) return position;
    }
    return std::string_view::npos;
}

std::size_t findUnescapedSequence(std::string_view value, std::string_view target, std::size_t start = 0) {
    if (target.empty()) return start <= value.size() ? start : std::string_view::npos;
    for (std::size_t position = start; position + target.size() <= value.size(); ++position) {
        if (value[position] == '\\' && isValidCSSEscape(value, position)) {
            position = consumeCSSEscape(value, position) - 1;
            continue;
        }
        if (value.compare(position, target.size(), target) == 0) return position;
    }
    return std::string_view::npos;
}

std::size_t consumeSelectorComponent(std::string_view value, std::size_t position) {
    while (position < value.size() && !isCSSWhitespace(value[position]) && value[position] != '>')
        if (value[position] == '\\' && isValidCSSEscape(value, position)) position = consumeCSSEscape(value, position);
        else ++position;
    return position;
}

bool isValidCSSIdentifier(std::string_view value) {
    if (value.empty()) return false;

    std::size_t position = 0;
    if (value[position] == '-') {
        if (value.size() == 1) return false;
        ++position;
        if (value[position] == '-') ++position;
        else if (isCSSNameStart(value[position])) ++position;
        else if (isValidCSSEscape(value, position)) position = consumeCSSEscape(value, position);
        else return false;
    } else if (isCSSNameStart(value[position])) {
        ++position;
    } else if (isValidCSSEscape(value, position)) {
        position = consumeCSSEscape(value, position);
    } else {
        return false;
    }

    while (position < value.size())
        if (isCSSNameCodePoint(value[position])) ++position;
        else if (isValidCSSEscape(value, position)) position = consumeCSSEscape(value, position);
        else return false;
    return true;
}

void appendCSSCodePoint(std::string& result, std::uint32_t codePoint) {
    if (codePoint == 0 || codePoint > 0x10ffff || (codePoint >= 0xd800 && codePoint <= 0xdfff)) codePoint = 0xfffd;
    if (codePoint <= 0x7f) result.push_back(static_cast<char>(codePoint));
    else if (codePoint <= 0x7ff) {
        result.push_back(static_cast<char>(0xc0 | (codePoint >> 6)));
        result.push_back(static_cast<char>(0x80 | (codePoint & 0x3f)));
    } else if (codePoint <= 0xffff) {
        result.push_back(static_cast<char>(0xe0 | (codePoint >> 12)));
        result.push_back(static_cast<char>(0x80 | ((codePoint >> 6) & 0x3f)));
        result.push_back(static_cast<char>(0x80 | (codePoint & 0x3f)));
    } else {
        result.push_back(static_cast<char>(0xf0 | (codePoint >> 18)));
        result.push_back(static_cast<char>(0x80 | ((codePoint >> 12) & 0x3f)));
        result.push_back(static_cast<char>(0x80 | ((codePoint >> 6) & 0x3f)));
        result.push_back(static_cast<char>(0x80 | (codePoint & 0x3f)));
    }
}

std::string decodeCSSIdentifier(std::string_view value) {
    std::string result;
    result.reserve(value.size());
    for (std::size_t position = 0; position < value.size();) {
        if (value[position] != '\\') {
            result.push_back(value[position++]);
            continue;
        }
        const std::size_t escapeStart = position++;
        if (position >= value.size()) {
            result.push_back(value[escapeStart]);
            continue;
        }
        if (!isCSSHexDigit(value[position])) {
            result.push_back(value[position++]);
            continue;
        }
        std::uint32_t codePoint = 0;
        std::size_t digits = 0;
        while (position < value.size() && digits < 6 && isCSSHexDigit(value[position])) {
            codePoint *= 16;
            const auto digit = static_cast<unsigned char>(value[position++]);
            codePoint += digit <= '9' ? digit - '0' : (digit >= 'a' && digit <= 'f' ? digit - 'a' + 10 : digit - 'A' + 10);
            ++digits;
        }
        if (position < value.size() && isCSSWhitespace(value[position])) ++position;
        appendCSSCodePoint(result, codePoint);
    }
    return result;
}

void appendSelectorState(StyleSelector& selector, const std::string& state) {
    if (selector.state.empty()) selector.state = state;
    else selector.state += ":" + state;
}

void parsePseudoClasses(std::string& token, StyleSelector& result) {
    const std::size_t separator = findUnescaped(token, ':');
    if (separator == std::string::npos) return;

    const std::string pseudoClasses = token.substr(separator);
    token.erase(separator);
    std::size_t position = 0;
    while (position < pseudoClasses.size()) {
        if (pseudoClasses[position] != ':') {
            appendSelectorState(result, pseudoClasses.substr(position));
            break;
        }
        const std::size_t start = ++position;
        int parentheses = 0;
        while (position < pseudoClasses.size()) {
            const char character = pseudoClasses[position];
            if (character == '(') ++parentheses;
            else if (character == ')' && parentheses > 0) --parentheses;
            else if (character == ':' && parentheses == 0) break;
            ++position;
        }

        const std::string pseudoClass = pseudoClasses.substr(start, position - start);
        if (lower(trim(pseudoClass)) == "root") {
            result.root = true;
            continue;
        }
        const std::size_t open = pseudoClass.find('(');
        if (open != std::string::npos && lower(trim(pseudoClass.substr(0, open))) == "dir") {
            if (pseudoClass.size() <= open + 1 || pseudoClass.back() != ')') {
                result.directionSyntaxInvalid = true;
                continue;
            }
            const std::string value = lower(trim(pseudoClass.substr(open + 1, pseudoClass.size() - open - 2)));
            if (value == "ltr" && !result.direction) result.direction = LayoutDirection::LeftToRight;
            else if (value == "rtl" && !result.direction) result.direction = LayoutDirection::RightToLeft;
            else result.directionSyntaxInvalid = true;
            continue;
        }
        appendSelectorState(result, pseudoClass);
    }
}

std::optional<ElementState> targetSpecificState(const std::string& state) {
    if (state == "checked") return ElementState::Checked;
    if (state == "minimized") return ElementState::Minimized;
    if (state == "invalid") return ElementState::Invalid;
    if (state == "indeterminate") return ElementState::Indeterminate;
    return std::nullopt;
}

std::pair<std::size_t, std::size_t> sourcePosition(const std::string& source, std::size_t offset) {
    std::size_t line = 1;
    std::size_t column = 1;
    for (std::size_t index = 0; index < offset && index < source.size(); ++index)
        if (source[index] == '\n') {
            ++line;
            column = 1;
        } else ++column;
    return {line, column};
}

std::optional<std::string> normalizeImportPath(const std::string& currentId, const std::string& requestedPath) {
    if (requestedPath.empty()
        || requestedPath.front() == '/'
        || requestedPath.find('\\') != std::string::npos
        || requestedPath.find(':') != std::string::npos
        || requestedPath.find("//") != std::string::npos) {
        return std::nullopt;
    }

    std::vector<std::string> segments;
    const std::size_t slash = currentId.rfind('/');
    const std::string combined = (slash == std::string::npos ? std::string() : currentId.substr(0, slash + 1)) + requestedPath;
    std::size_t start = 0;
    while (start <= combined.size()) {
        const std::size_t end = combined.find('/', start);
        const std::string segment = combined.substr(start, end == std::string::npos ? std::string::npos : end - start);
        if (segment.empty() || segment == ".") {
            if (segment.empty() && start != combined.size()) return std::nullopt;
        } else if (segment == "..") {
            if (segments.empty()) return std::nullopt;
            segments.pop_back();
        } else segments.push_back(segment);
        if (end == std::string::npos) break;
        start = end + 1;
    }
    if (segments.empty()) return std::nullopt;

    std::string result;
    for (const std::string& segment : segments) {
        if (!result.empty()) result += '/';
        result += segment;
    }
    constexpr const char* kCssExtension = ".css";
    if (result.size() < 4 || result.compare(result.size() - 4, 4, kCssExtension) != 0) return std::nullopt;
    return result;
}

std::string importChain(const std::vector<std::string>& stack, const std::optional<std::string>& tail = std::nullopt) {
    std::string chain;
    for (const std::string& resource : stack) {
        if (!chain.empty()) chain += " -> ";
        chain += resource;
    }
    if (tail) {
        if (!chain.empty()) chain += " -> ";
        chain += *tail;
    }
    return chain;
}

StyleSelector mergeSelector(const StyleSelector& parent, const StyleSelector& child) {
    StyleSelector result;
    result.universal = child.universal ? true : parent.universal;
    result.root = parent.root || child.root;
    result.attributeSyntaxInvalid = parent.attributeSyntaxInvalid || child.attributeSyntaxInvalid;
    result.idSyntaxInvalid = parent.idSyntaxInvalid || child.idSyntaxInvalid;
    result.classSyntaxInvalid = parent.classSyntaxInvalid || child.classSyntaxInvalid;
    result.directionSyntaxInvalid = parent.directionSyntaxInvalid || child.directionSyntaxInvalid;
    result.element = child.element.empty() ? parent.element : child.element;
    result.attributes = parent.attributes;
    result.attributes.insert(result.attributes.end(), child.attributes.begin(), child.attributes.end());
    result.id = child.id.empty() ? parent.id : child.id;
    result.className = child.className.empty() ? parent.className : child.className;
    result.state = parent.state;
    result.pseudoElementSyntaxInvalid =
        parent.pseudoElementSyntaxInvalid || child.pseudoElementSyntaxInvalid || (!parent.pseudoElement.empty() && !child.pseudoElement.empty());
    result.direction = child.direction ? child.direction : parent.direction;
    if (!child.state.empty()) {
        if (parent.pseudoElement.empty()) result.state = child.state;
        else result.pseudoElementSyntaxInvalid = true;
    }
    result.pseudoElement = child.pseudoElement.empty() ? parent.pseudoElement : child.pseudoElement;
    return result;
}

void parseAttributeSelector(std::string& token, StyleSelector& result) {
    while (true) {
        const std::size_t open = findUnescaped(token, '[');
        if (open == std::string::npos) return;

        char quote = 0;
        std::size_t close = open + 1;
        for (; close < token.size(); ++close) {
            const char character = token[close];
            if (quote) {
                if (character == quote && token[close - 1] != '\\') quote = 0;
            } else if (character == '\'' || character == '"') {
                quote = character;
            } else if (character == ']') {
                break;
            }
        }
        if (close == token.size() || quote) {
            result.attributeSyntaxInvalid = true;
            token.erase(open);
            return;
        }

        const std::string expression = token.substr(open + 1, close - open - 1);
        std::size_t equals = std::string::npos;
        quote = 0;
        for (std::size_t index = 0; index < expression.size(); ++index) {
            const char character = expression[index];
            if (quote) {
                if (character == quote && expression[index - 1] != '\\') quote = 0;
            } else if (character == '\'' || character == '"') {
                quote = character;
            } else if (character == '=') {
                if (equals != std::string::npos) {
                    result.attributeSyntaxInvalid = true;
                    token.erase(open);
                    return;
                }
                equals = index;
            }
        }

        StyleAttributeSelector attribute;
        if (equals == std::string::npos) {
            attribute.name = lower(trim(expression));
            if (attribute.name.empty()) {
                result.attributeSyntaxInvalid = true;
                token.erase(open);
                return;
            }
            attribute.presence = true;
        } else {
            attribute.name = lower(trim(expression.substr(0, equals)));
            std::string value = trim(expression.substr(equals + 1));
            if (attribute.name.empty() || value.empty()) {
                result.attributeSyntaxInvalid = true;
                token.erase(open);
                return;
            }
            if (value.front() == '\'' || value.front() == '"') {
                if (value.size() < 2 || value.back() != value.front()) {
                    result.attributeSyntaxInvalid = true;
                    token.erase(open);
                    return;
                }
                value = value.substr(1, value.size() - 2);
            } else if (!isValidCSSIdentifier(value)) {
                result.attributeSyntaxInvalid = true;
                token.erase(open);
                return;
            }
            if (value.empty()) {
                result.attributeSyntaxInvalid = true;
                token.erase(open);
                return;
            }
            attribute.value = decodeCSSIdentifier(value);
        }
        result.attributes.push_back(std::move(attribute));
        token.erase(open, close - open + 1);
    }
}

void appendSelector(StyleRule& destination, const StyleRule& suffix, SelectorCombinator combinator) {
    if (suffix.selectors.empty()) return;
    destination.combinators.push_back(combinator);
    destination.selectors.insert(destination.selectors.end(), suffix.selectors.begin(), suffix.selectors.end());
    destination.combinators.insert(destination.combinators.end(), suffix.combinators.begin(), suffix.combinators.end());
}

StyleRule expandNestedSelector(const StyleRule& parent, const std::string& rawSelector) {
    const std::string selector = trim(rawSelector);
    StyleRule result = parent;
    if (selector.empty()) return {};

    if (selector.front() != '&') {
        const bool child = selector.front() == '>';
        const std::string suffix = trim(selector.substr(child ? 1 : 0));
        appendSelector(result, detail::parseSelector(suffix), child ? SelectorCombinator::Child : SelectorCombinator::Descendant);
        return result;
    }

    const std::string tail = selector.substr(1);
    if (tail.empty()) return result;
    if (isCSSWhitespace(tail.front()) || tail.front() == '>') {
        const std::string trimmedTail = trim(tail);
        const bool child = !trimmedTail.empty() && trimmedTail.front() == '>';
        const std::string suffix = trim(trimmedTail.substr(child ? 1 : 0));
        appendSelector(result, detail::parseSelector(suffix), child ? SelectorCombinator::Child : SelectorCombinator::Descendant);
        return result;
    }

    const std::size_t split = consumeSelectorComponent(tail, 0);
    const StyleRule continuation = detail::parseSelector(tail.substr(0, split));
    if (!continuation.selectors.empty() && !result.selectors.empty())
        result.selectors.back() = mergeSelector(result.selectors.back(), continuation.selectors.front());
    if (split < tail.size()) {
        const std::string remainder = trim(tail.substr(split));
        const bool child = !remainder.empty() && remainder.front() == '>';
        appendSelector(result, detail::parseSelector(trim(remainder.substr(child ? 1 : 0))),
                       child ? SelectorCombinator::Child : SelectorCombinator::Descendant);
    }
    return result;
}

StyleSelector parseSimpleSelector(const std::string& selectorText) {
    StyleSelector result;
    std::string token = trim(selectorText);

    if (const std::size_t separator = findUnescapedSequence(token, "::"); separator != std::string::npos) {
        const std::string pseudoElement = trim(token.substr(separator + 2));
        if (pseudoElement.empty()
            || findUnescapedSequence(pseudoElement, "::") != std::string::npos
            || findUnescaped(pseudoElement, ':') != std::string::npos)
            result.pseudoElementSyntaxInvalid = true;
        else if (!isValidCSSIdentifier(pseudoElement)) result.pseudoElementSyntaxInvalid = true;
        else result.pseudoElement = decodeCSSIdentifier(pseudoElement);
        token.erase(separator);
    }
    parsePseudoClasses(token, result);
    parseAttributeSelector(token, result);
    if (const std::size_t separator = findUnescaped(token, '#'); separator != std::string::npos) {
        const std::string value = token.substr(separator + 1);
        if (!isValidCSSIdentifier(value)) result.idSyntaxInvalid = true;
        else result.id = decodeCSSIdentifier(value);
        token.erase(separator);
    }
    if (const std::size_t separator = findUnescaped(token, '.'); separator != std::string::npos) {
        const std::string value = token.substr(separator + 1);
        if (!isValidCSSIdentifier(value)) result.classSyntaxInvalid = true;
        else result.className = decodeCSSIdentifier(value);
        token.erase(separator);
    }
    result.universal = token == "*";
    if (!result.universal) result.element = token;
    return result;
}

struct ParsedRuleBlock {
    std::string selector;
    std::string body;
};

struct ParsedImport {
    std::string id;
    std::string requestedPath;
    std::size_t line = 1;
    std::size_t column = 1;
};

struct ParsedModule {
    std::string id;
    std::string sourceName;
    std::vector<ParsedImport> imports;
    std::vector<ParsedRuleBlock> rules;
};

class StyleSheetModuleGraph {
public:
    StyleSheetModuleGraph(const ResourceLayer& layer, StyleModel& model, StyleSheetLoadResult& result)
        : mLayer(layer), mModel(model), mResult(result) {}

    bool build(const std::string& entrypoint) {
        std::vector<std::string> importStack;
        return ensureParsed(entrypoint, mLayer.content, mLayer.provenance, importStack);
    }

    template<typename Callback> void visit(const std::string& id, Callback& callback) const {
        std::vector<std::string> importStack;
        std::vector<VisitEntry> moduleVisits;
        collectModules(id, importStack, moduleVisits);

        const auto emit = [&](const VisitEntry& entry, StyleParsePass pass) {
            for (const ParsedRuleBlock& rule : entry.module->rules) {
                if (pass == StyleParsePass::Tokens && rule.selector != ":root") continue;
                const std::size_t firstError = mResult.errors.size();
                callback(rule, entry.module->sourceName, pass);
                if (entry.importChain.size() > 1) {
                    for (std::size_t index = firstError; index < mResult.errors.size(); ++index)
                        if (mResult.errors[index].message.find("Import chain:") == std::string::npos
                            && mResult.errors[index].code != "stylesheet.import.cycle")
                            mResult.errors[index].message += " Import chain: " + importChain(entry.importChain) + ".";
                }
            }
        };
        for (const VisitEntry& entry : moduleVisits) emit(entry, StyleParsePass::Tokens);
        for (const VisitEntry& entry : moduleVisits) emit(entry, StyleParsePass::Rules);
    }

private:
    struct VisitEntry {
        const ParsedModule* module = nullptr;
        std::vector<std::string> importChain;
    };

    std::optional<ParsedModule> parseSyntax(const std::string& source, const std::string& id, const std::string& sourceName) {
        ParsedModule module{id, sourceName};
        bool sawRule = false;
        std::size_t position = 0;
        while (position < source.size() && !mResult.hasErrors()) {
            while (position < source.size() && std::isspace(static_cast<unsigned char>(source[position]))) ++position;
            if (position == source.size()) break;

            if (source.compare(position, 7, "@import") == 0
                && (position + 7 == source.size() || std::isspace(static_cast<unsigned char>(source[position + 7])))) {
                const auto [line, column] = sourcePosition(source, position);
                if (sawRule) {
                    mResult.error("stylesheet.import.order", "@import must precede all rules in its module.", sourceName, line, column);
                    return std::nullopt;
                }
                const std::optional<std::size_t> semicolon = topLevelDelimiter(source, position + 7, ';');
                if (!semicolon) {
                    mResult.error("stylesheet.import.syntax", "@import requires a quoted path followed by ';'.", sourceName, line, column);
                    return std::nullopt;
                }
                const std::string argument = trim(source.substr(position + 7, *semicolon - position - 7));
                if (argument.size() < 2 || (argument.front() != '\"' && argument.front() != '\'') || argument.back() != argument.front()) {
                    mResult.error("stylesheet.import.syntax", "@import requires exactly one quoted path.", sourceName, line, column);
                    return std::nullopt;
                }
                const std::string requestedPath = argument.substr(1, argument.size() - 2);
                const std::optional<std::string> importedId = normalizeImportPath(id, requestedPath);
                if (!importedId) {
                    mResult.error("stylesheet.import.path_invalid", "Invalid or escaping @import path: " + requestedPath + ".", sourceName, line,
                                  column);
                    return std::nullopt;
                }
                mModel.dependencies[sourceName].insert(mLayer.provenanceFor(*importedId));
                module.imports.push_back({*importedId, requestedPath, line, column});
                position = *semicolon + 1;
                continue;
            }
            if (source[position] == '@') {
                const auto [line, column] = sourcePosition(source, position);
                mResult.error("stylesheet.at_rule.unsupported", "Unsupported stylesheet at-rule.", sourceName, line, column);
                return std::nullopt;
            }

            sawRule = true;
            const std::optional<std::size_t> open = topLevelDelimiter(source, position, '{');
            if (!open) {
                mResult.error("stylesheet.syntax.trailing_content", "Unexpected content outside a rule: " + trim(source.substr(position)) + ".",
                              sourceName);
                return std::nullopt;
            }
            const std::string selector = trim(source.substr(position, *open - position));
            if (selector.empty()) {
                mResult.error("stylesheet.selector.empty", "Rule selector is empty.", sourceName);
                return std::nullopt;
            }
            const std::optional<std::size_t> close = detail::matchingBlock(source, *open);
            if (!close) {
                mResult.error("stylesheet.syntax.unclosed_block", "Rule block is not closed: " + selector + ".", sourceName);
                return std::nullopt;
            }
            module.rules.push_back({selector, source.substr(*open + 1, *close - *open - 1)});
            position = *close + 1;
        }
        return module;
    }

    bool ensureParsed(const std::string& id, const std::string& source, const std::string& sourceName, std::vector<std::string>& importStack) {
        if (mModules.find(id) != mModules.end()) return true;
        const std::optional<ParsedModule> parsed = parseSyntax(source, id, sourceName);
        if (!parsed) return false;
        mModules.emplace(id, *parsed);
        importStack.push_back(id);
        for (const ParsedImport& imported : parsed->imports) {
            if (std::find(importStack.begin(), importStack.end(), imported.id) != importStack.end()) {
                mResult.error("stylesheet.import.cycle", "Cyclic @import: " + importChain(importStack, imported.id) + ".", sourceName, imported.line,
                              imported.column);
                continue;
            }
            const std::string importedName = mLayer.provenanceFor(imported.id);
            const std::string* importedSource = nullptr;
            if (imported.id == mLayer.entrypoint) importedSource = &mLayer.content;
            else if (const auto found = mLayer.modules.find(imported.id); found != mLayer.modules.end()) importedSource = &found->second;
            if (!importedSource) {
                mResult.error("stylesheet.import.missing",
                              "Imported stylesheet module is missing: "
                                  + imported.requestedPath
                                  + ". Import chain: "
                                  + importChain(importStack, imported.id)
                                  + ".",
                              sourceName, imported.line, imported.column);
                continue;
            }
            const std::size_t firstError = mResult.errors.size();
            ensureParsed(imported.id, *importedSource, importedName, importStack);
            for (std::size_t index = firstError; index < mResult.errors.size(); ++index)
                if (mResult.errors[index].message.find("Import chain:") == std::string::npos
                    && mResult.errors[index].code != "stylesheet.import.cycle")
                    mResult.errors[index].message += " Import chain: " + importChain(importStack, imported.id) + ".";
        }
        importStack.pop_back();
        return !mResult.hasErrors();
    }

    void collectModules(const std::string& id, std::vector<std::string>& importStack, std::vector<VisitEntry>& moduleVisits) const {
        if (std::find(importStack.begin(), importStack.end(), id) != importStack.end()) return;
        const auto module = mModules.find(id);
        if (module == mModules.end()) return;
        importStack.push_back(id);
        for (const ParsedImport& imported : module->second.imports) collectModules(imported.id, importStack, moduleVisits);
        moduleVisits.push_back({&module->second, importStack});
        importStack.pop_back();
    }

    const ResourceLayer& mLayer;
    StyleModel& mModel;
    StyleSheetLoadResult& mResult;
    std::map<std::string, ParsedModule> mModules;
};
} // namespace

StyleRule detail::parseSelector(const std::string& selector) {
    StyleRule rule;
    const std::string input = trim(selector);
    if (input.empty()) return rule;

    std::size_t position = 0;
    SelectorCombinator pending = SelectorCombinator::Descendant;
    while (position < input.size()) {
        while (position < input.size() && isCSSWhitespace(input[position])) ++position;
        if (position >= input.size()) break;
        if (input[position] == '>') {
            pending = SelectorCombinator::Child;
            ++position;
            continue;
        }

        const std::size_t start = position;
        position = consumeSelectorComponent(input, position);
        if (!rule.selectors.empty()) rule.combinators.push_back(pending);
        rule.selectors.push_back(parseSimpleSelector(input.substr(start, position - start)));
        pending = SelectorCombinator::Descendant;
    }
    return rule;
}

StyleSheetLoadResult StyleSheet::loadRadia(const std::string& stylesheetSource, const std::string& sourceName) {
    return loadRadiaLayers({StyleLayer{StyleOrigin::Default, ResourceLayer{sourceName, stylesheetSource}}});
}

StyleSheetLoadResult StyleSheet::loadRadiaLayers(const std::vector<StyleLayer>& layers) {
    Impl candidate;
    StyleSheetLoadResult result;
    if (layers.empty()) {
        result.error("stylesheet.layers.empty", "No stylesheet layers were provided.");
        return result;
    }
    // Root tokens are collected before ordinary declarations, while the rules pass preserves source order.
    std::vector<StyleLayer> orderedLayers = layers;
    std::stable_sort(orderedLayers.begin(), orderedLayers.end(), [](const StyleLayer& left, const StyleLayer& right) {
        return static_cast<std::uint8_t>(left.origin) < static_cast<std::uint8_t>(right.origin);
    });
    for (const StyleLayer& styleLayer : orderedLayers) {
        const ResourceLayer& layer = styleLayer.resource;
        const std::string entrypoint = layer.entrypoint.empty() ? layer.provenance : layer.entrypoint;
        StyleSheetModuleGraph graph(layer, candidate, result);
        graph.build(entrypoint);
        if (result.hasErrors()) continue;

        auto compileModule = [&](const ParsedRuleBlock& rule, const std::string& sourceName, StyleParsePass pass) {
            candidate.parseBlock(rule.selector, rule.body, {}, styleLayer.origin, pass, result, sourceName);
        };
        graph.visit(entrypoint, compileModule);
    }
    if (result.ok()) {
        candidate.sortRules();
        candidate.generation = mImpl->generation + 1;
        mImpl = std::make_shared<Impl>(std::move(candidate));
    }
    return result;
}

namespace {
std::vector<std::string> splitSelectorList(const std::string& selectorText) {
    std::vector<std::string> selectors;
    std::size_t selectorStart = 0;
    while (selectorStart <= selectorText.size()) {
        const std::size_t comma = selectorText.find(',', selectorStart);
        selectors.push_back(trim(selectorText.substr(selectorStart, comma == std::string::npos ? std::string::npos : comma - selectorStart)));
        if (comma == std::string::npos) break;
        selectorStart = comma + 1;
    }
    return selectors;
}

bool validateSelector(StyleRule& rule, const std::string& selector, StyleSheetLoadResult& result, const std::string& sourceName) {
    for (std::size_t index = 0; index < rule.selectors.size(); ++index) {
        StyleSelector& component = rule.selectors[index];
        const bool declarationComponent = index + 1 == rule.selectors.size();
        if (component.attributeSyntaxInvalid) {
            result.error("stylesheet.selector.attribute_invalid",
                         "Attribute selectors must use [attribute] or [type=\"value\"] syntax: " + selector + ".", sourceName);
            return false;
        }
        if (component.pseudoElementSyntaxInvalid) {
            result.error("stylesheet.selector.pseudo_element_invalid", "Pseudo-elements cannot be followed by pseudo-classes: " + selector + ".",
                         sourceName);
            return false;
        }
        for (const StyleAttributeSelector& attribute : component.attributes)
            if (attribute.name != "type" && attribute.name != "switch" && attribute.name != "name") {
                result.error("stylesheet.selector.attribute_unsupported",
                             "Only the type, switch, and name attributes can be selected: " + selector + ".", sourceName);
                return false;
            }
        if (component.idSyntaxInvalid) {
            result.error("stylesheet.selector.id_invalid", "Element IDs in selectors must use CSS identifier syntax: " + selector + ".", sourceName);
            return false;
        }
        if (component.classSyntaxInvalid) {
            result.error("stylesheet.selector.class_invalid", "Element classes in selectors must use CSS identifier syntax: " + selector + ".",
                         sourceName);
            return false;
        }
        if (component.directionSyntaxInvalid) {
            result.error("stylesheet.selector.state_unknown", "Invalid :dir() selector: " + selector + ".", sourceName);
            return false;
        }
        if (!isSupportedState(component.state)) {
            const std::string& state = component.state;
            result.error("stylesheet.selector.state_unknown", "Unknown selector state: " + state + ".", sourceName);
            return false;
        }
        if (!declarationComponent && !component.pseudoElement.empty()) {
            result.error("stylesheet.selector.pseudo_element_structural",
                         "Pseudo-elements cannot participate in structural combinators: " + selector + ".", sourceName);
            return false;
        }
        if (component.element.empty()) {
            if (!component.attributes.empty() || !component.pseudoElement.empty()) {
                result.error("stylesheet.selector.target_required",
                             "Attributes and pseudo-elements require an element-qualified selector: " + selector + ".", sourceName);
                return false;
            }
            continue;
        }
        if (canonicalizeHTMLName(component.element) == kKbdTag.localName) {
            if (!component.attributes.empty() || !component.id.empty() || !component.className.empty()) {
                result.error("stylesheet.selector.inline_identity_unsupported",
                             "Inline style elements do not have Element IDs, classes, or attributes: " + selector + ".", sourceName);
                return false;
            }
            component.element = kKbdTag.localName;
            if (!component.pseudoElement.empty()) {
                result.error("stylesheet.selector.pseudo_element_unknown", "Unknown pseudo-element for " + component.element + ".", sourceName);
                return false;
            }
            continue;
        }
        const HTMLTag componentTag = lookupHTMLTag(component.element);
        if (!component.attributes.empty() && componentTag != HTMLTag::Input) {
            result.error("stylesheet.selector.attribute_unsupported", "Input attributes can only be selected on input: " + selector + ".",
                         sourceName);
            return false;
        }
        const ElementSelectorMetadata metadata = inspectElementSelector(componentTag, component.pseudoElement, targetSpecificState(component.state));
        if (!metadata.known) {
            result.error("stylesheet.selector.element_unknown", "Unknown element element: " + component.element + ".", sourceName);
            return false;
        }
        component.element = metadata.elementName;
        if (!metadata.pseudoElementKnown) {
            result.error("stylesheet.selector.pseudo_element_unknown",
                         "Unknown pseudo-element for " + component.element + ": " + component.pseudoElement + ".", sourceName);
            return false;
        }
        if (targetSpecificState(component.state) && !metadata.elementProducesState)
            result.warning("stylesheet.selector.state_never_matches",
                           "State :" + component.state + " is never produced by " + component.element + ".", sourceName);
    }
    return true;
}

struct RuleBodyFragment {
    std::size_t start = 0;
    std::size_t end = 0;
    std::optional<std::size_t> open;
    std::optional<std::size_t> close;
};

class RuleBodyCursor {
public:
    explicit RuleBodyCursor(const std::string& body) : mBody(body) {}

    std::optional<RuleBodyFragment> next() {
        while (mPosition < mBody.size() && std::isspace(static_cast<unsigned char>(mBody[mPosition]))) ++mPosition;
        if (mPosition >= mBody.size()) return std::nullopt;

        const std::optional<TopLevelDelimiter> delimiter = nextTopLevelDelimiter(mBody, mPosition);
        if (delimiter && delimiter->value == '{') {
            const std::optional<std::size_t> close = detail::matchingBlock(mBody, delimiter->position);
            if (!close) {
                mPosition = mBody.size();
                return RuleBodyFragment{mPosition, mPosition, delimiter->position, std::nullopt};
            }
            const RuleBodyFragment fragment{mPosition, *close + 1, delimiter->position, close};
            mPosition = *close + 1;
            return fragment;
        }

        const std::size_t end = delimiter && delimiter->value == ';' ? delimiter->position : mBody.size();
        const RuleBodyFragment fragment{mPosition, end, std::nullopt, std::nullopt};
        mPosition = delimiter && delimiter->value == ';' ? delimiter->position + 1 : mBody.size();
        return fragment;
    }

private:
    const std::string& mBody;
    std::size_t mPosition = 0;
};

void parseRuleBody(StyleModel& model, StyleRule& rule, const std::string& selector, const std::string& body, bool rootRule, StyleParsePass pass,
                   StyleSheetLoadResult& result, const std::string& sourceName) {
    std::vector<StyleDeclaration> declarations;
    const auto flushDeclarations = [&] {
        if (declarations.empty()) return;
        StyleRule declarationRule = rule;
        declarationRule.declarations = std::move(declarations);
        model.addRule(declarationRule);
        declarations.clear();
    };
    const auto addDeclaration = [&](const std::string& declaration) {
        const std::size_t colon = declaration.find(':');
        if (colon == std::string::npos) {
            result.error("stylesheet.declaration.invalid", "Declaration requires a property and value: " + trim(declaration) + ".", sourceName);
            return;
        }
        const std::string name = trim(declaration.substr(0, colon));
        const std::string value = trim(declaration.substr(colon + 1));
        if (name.empty() || value.empty()) {
            result.error("stylesheet.declaration.invalid", "Declaration property and value must not be empty.", sourceName);
            return;
        }
        if (startsWith(name, "--")) {
            if (!rootRule) {
                result.error("stylesheet.token.root_required", "Style Tokens may be declared only in :root: " + name + ".", sourceName);
                return;
            }
            if (pass == StyleParsePass::Rules) return;
            if (isColorValue(value)) {
                const Color marker(-1.f, -1.f, -1.f, -1.f);
                const Color parsed = model.parseColorValue(value, marker);
                if (parsed.a < 0.f)
                    result.error("stylesheet.token.value_invalid", "Invalid color token value for " + name + ": " + value + ".", sourceName);
                else model.setColorToken(name, parsed);
            } else {
                const float parsed = model.parseNumberValue(value, std::numeric_limits<float>::quiet_NaN());
                if (!std::isfinite(parsed))
                    result.error("stylesheet.token.value_invalid", "Invalid number token value for " + name + ": " + value + ".", sourceName);
                else model.setNumberToken(name, parsed);
            }
            return;
        }
        if (pass == StyleParsePass::Tokens) return;
        const detail::StylePropertyDefinition* descriptor = detail::findStyleProperty(name);
        if (!descriptor) {
            result.error("stylesheet.property.unknown", "Unknown property: " + name + ".", sourceName);
            return;
        }
        if (descriptor->defaultOnly && rule.origin != StyleOrigin::Default) {
            result.warning("stylesheet.property.ua_only", "Ignoring UA-only property outside the default stylesheet: " + name + ".", sourceName);
            return;
        }
        if (auto compiled = model.compileDeclaration(*descriptor, value, selector, result, sourceName))
            declarations.insert(declarations.end(), std::make_move_iterator(compiled->begin()), std::make_move_iterator(compiled->end()));
    };

    RuleBodyCursor cursor(body);
    while (const std::optional<RuleBodyFragment> fragment = cursor.next()) {
        if (fragment->open) {
            if (!fragment->close) {
                result.error("stylesheet.syntax.unclosed_block", "Nested rule block is not closed.", sourceName);
                break;
            }
            if (pass == StyleParsePass::Rules) {
                flushDeclarations();
                model.parseBlock(trim(body.substr(fragment->start, *fragment->open - fragment->start)),
                                 body.substr(*fragment->open + 1, *fragment->close - *fragment->open - 1), rule, rule.origin, pass, result,
                                 sourceName);
            }
            continue;
        }
        addDeclaration(body.substr(fragment->start, fragment->end - fragment->start));
    }
    if (pass == StyleParsePass::Rules) flushDeclarations();
}
} // namespace

void StyleModel::parseBlock(const std::string& selectorText, const std::string& body, const StyleRule& parent, StyleOrigin origin,
                            StyleParsePass pass, StyleSheetLoadResult& result, const std::string& sourceName) {
    const std::vector<std::string> selectors = splitSelectorList(selectorText);
    if (selectors.size() > 1) {
        for (const std::string& selector : selectors)
            if (!selector.empty()) parseBlock(selector, body, parent, origin, pass, result, sourceName);
        return;
    }

    const std::string selector = trim(selectorText);
    const bool nested = !parent.selectors.empty();
    StyleRule rule = nested ? expandNestedSelector(parent, selector) : detail::parseSelector(selector);
    rule.origin = origin;
    const bool rootRule = !nested && selector == ":root";
    if (rule.selectors.empty()) {
        result.error("stylesheet.selector.empty", "Rule selector is empty.", sourceName);
        return;
    }

    if (!validateSelector(rule, selector, result, sourceName)) return;

    parseRuleBody(*this, rule, selector, body, rootRule, pass, result, sourceName);
}
} // namespace radia::ui
