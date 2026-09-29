/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <cstddef>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>
#include "CSSPseudoSelectors.h"
#include "CSSRules.h"
#include "CSSSelectorParserInternal.h"
#include "CSSTokenStream.h"

namespace Core::CSS {
namespace {
using detail::decodeIdentifier;
using detail::isTrivia;
using detail::isWhitespace;
using detail::lower;
using detail::normalizeKeyword;
using detail::skipComponent;
using detail::Token;
using detail::TokenKind;
using detail::TokenStream;
using detail::trim;
using detail::trimRange;

std::size_t findUnescaped(std::string_view value, char target, std::size_t start = 0) {
    const TokenStream stream(value);
    const auto& tokens = stream.tokens();
    for (std::size_t index = 0; index < tokens.size();) {
        const auto& token = tokens[index];
        if (token.end <= start) {
            ++index;
            continue;
        }
        if (isTrivia(token.kind)) {
            ++index;
            continue;
        }
        if (token.kind == TokenKind::OpenBracket && target == '[' && token.begin >= start)
            return token.begin;
        if (token.kind == TokenKind::CloseBracket && target == ']' && token.begin >= start)
            return token.begin;
        if (token.kind == TokenKind::Hash && target == '#' && token.begin >= start)
            return token.begin;
        if (token.kind == TokenKind::Function || token.kind == TokenKind::OpenParen || token.kind == TokenKind::OpenBracket) {
            index = skipComponent(stream, index, tokens.size());
            continue;
        }
        if (token.begin >= start && token.end == token.begin + 1 && stream.text(index)[0] == target)
            return token.begin;
        ++index;
    }
    return std::string_view::npos;
}

std::size_t findUnescapedSequence(std::string_view value, std::string_view target, std::size_t start = 0) {
    if (target.empty())
        return start <= value.size() ? start : std::string_view::npos;
    const TokenStream stream(value);
    const auto& tokens = stream.tokens();
    for (std::size_t index = 0; index < tokens.size();) {
        const auto& token = tokens[index];
        if (token.end <= start || isTrivia(token.kind)) {
            ++index;
            continue;
        }
        if (token.kind == TokenKind::Function || token.kind == TokenKind::OpenParen || token.kind == TokenKind::OpenBracket) {
            index = skipComponent(stream, index, tokens.size());
            continue;
        }
        if (token.begin >= start && token.begin + target.size() <= value.size() && value.compare(token.begin, target.size(), target) == 0)
            return token.begin;
        ++index;
    }
    return std::string_view::npos;
}

bool isColumnCombinator(const TokenStream& stream, std::size_t index, std::size_t end) {
    if (index + 1 >= end || stream.tokens()[index].kind != TokenKind::Delim || stream.text(index) != "|")
        return false;
    const Token& next = stream.tokens()[index + 1];
    return next.kind == TokenKind::Delim && stream.text(index + 1) == "|" && next.begin == stream.tokens()[index].end;
}

std::size_t consumeSelectorComponent(std::string_view value, std::size_t position) {
    const TokenStream stream(value);
    const auto& tokens = stream.tokens();
    for (std::size_t index = 0; index < tokens.size();) {
        const auto& token = tokens[index];
        if (token.end <= position) {
            ++index;
            continue;
        }
        if (isTrivia(token.kind))
            return token.begin;
        if (token.kind == TokenKind::Delim
            && (stream.text(index) == ">" || stream.text(index) == "+" || stream.text(index) == "~"
                || isColumnCombinator(stream, index, tokens.size())))
            return token.begin;
        if (token.kind == TokenKind::Function || token.kind == TokenKind::OpenParen || token.kind == TokenKind::OpenBracket) {
            index = skipComponent(stream, index, tokens.size());
            continue;
        }
        ++index;
    }
    return value.size();
}

struct LeadingSelectorCombinator {
    SelectorCombinator value;
    std::size_t length;
};

std::optional<LeadingSelectorCombinator> leadingSelectorCombinator(std::string_view value) {
    if (value.rfind("||", 0) == 0)
        return LeadingSelectorCombinator {SelectorCombinator::Column, 2};
    if (value.empty())
        return std::nullopt;
    if (value.front() == '>')
        return LeadingSelectorCombinator {SelectorCombinator::Child, 1};
    if (value.front() == '+')
        return LeadingSelectorCombinator {SelectorCombinator::NextSibling, 1};
    if (value.front() == '~')
        return LeadingSelectorCombinator {SelectorCombinator::SubsequentSibling, 1};
    return std::nullopt;
}

bool isValidIdentifier(std::string_view value) {
    const TokenStream stream(value);
    if (stream.tokens().size() != 1 || !stream.hasBalancedBlocks())
        return false;
    const auto& token = stream.tokens().front();
    return token.kind == TokenKind::Ident && token.begin == 0 && token.end == stream.source().size();
}

void appendPseudoClass(StyleSelector& selector, const std::string& name) {
    const std::optional<PseudoClass> pseudoClass = findPseudoClass(name);
    if (!pseudoClass) {
        selector.pseudoClassSyntaxInvalid = true;
        selector.invalidPseudoClass = name;
        return;
    }
    const PseudoClassDescriptor* descriptor = pseudoClassDescriptor(*pseudoClass);
    if (descriptor && descriptor->argumentRequirement != PseudoClassArgumentRequirement::Required) {
        selector.pseudoClasses.push_back(*pseudoClass);
        return;
    }
    selector.pseudoClassArgumentSyntaxInvalid = true;
}

void parsePseudoClasses(std::string& token, StyleSelector& result) {
    const std::size_t separator = findUnescaped(token, ':');
    if (separator == std::string::npos)
        return;

    const std::string pseudoClasses = token.substr(separator);
    token.erase(separator);
    const TokenStream stream(pseudoClasses);
    const std::vector<detail::TokenRange> ranges = detail::splitOnDelimiter(stream, {0, stream.tokens().size()}, ':');
    if (ranges.empty()) {
        appendPseudoClass(result, normalizeKeyword(pseudoClasses.substr(1)));
        return;
    }

    for (std::size_t index = 1; index < ranges.size(); ++index) {
        const std::string pseudoClass = trim(detail::serializeRange(stream, ranges[index]));
        const TokenStream pseudoStream(pseudoClass);
        const auto& pseudoTokens = pseudoStream.tokens();
        std::vector<std::size_t> significant;
        for (std::size_t token = 0; token < pseudoTokens.size(); ++token)
            if (!isTrivia(pseudoTokens[token].kind))
                significant.push_back(token);

        if (significant.size() >= 2 && pseudoTokens[significant.front()].kind == TokenKind::Function) {
            const std::size_t function = significant.front();
            const std::size_t close = pseudoTokens[function].matching;
            const std::size_t last = significant.back();
            const std::string_view functionText = pseudoStream.text(function);
            const std::string functionName = functionText.empty() || functionText.back() != '('
                ? std::string()
                : lower(decodeIdentifier(functionText.substr(0, functionText.size() - 1)));
            const auto functionId = findPseudoClass(functionName);
            const PseudoClassDescriptor* descriptor = functionId ? pseudoClassDescriptor(*functionId) : nullptr;
            if (descriptor) {
                if (close == detail::kNoMatchingToken || close != last || descriptor->argumentSyntax == PseudoClassArgumentSyntax::None) {
                    result.pseudoClassArgumentSyntaxInvalid = true;
                    continue;
                }

                switch (descriptor->argumentSyntax) {
                case PseudoClassArgumentSyntax::Ident: {
                    const std::string value = normalizeKeyword(pseudoStream, {function + 1, close});
                    if (functionId != PseudoClass::Dir || !isValidIdentifier(value)) {
                        result.pseudoClassArgumentSyntaxInvalid = true;
                        break;
                    }
                    auto selectorFunction = std::make_shared<StyleSelectorFunction>();
                    selectorFunction->pseudoClass = *functionId;
                    selectorFunction->identifier = value;
                    result.selectorFunctions.push_back(std::move(selectorFunction));
                    break;
                }
                case PseudoClassArgumentSyntax::ForgivingSelectorList: {
                    auto selectorFunction = std::make_shared<StyleSelectorFunction>();
                    selectorFunction->pseudoClass = *functionId;
                    for (const detail::TokenRange argumentRange : detail::splitOnDelimiter(pseudoStream, {function + 1, close}, ',')) {
                        if (argumentRange.begin == argumentRange.end)
                            continue;
                        StyleRule argument = detail::parseSelector(pseudoStream, argumentRange);
                        if (!argument.selectors.empty())
                            selectorFunction->arguments.push_back(std::move(argument));
                    }
                    result.selectorFunctions.push_back(std::move(selectorFunction));
                    break;
                }
                case PseudoClassArgumentSyntax::CompoundSelector: {
                    const std::vector<detail::TokenRange> arguments = detail::splitOnDelimiter(pseudoStream, {function + 1, close}, ',');
                    if (arguments.size() != 1 || arguments.front().begin == arguments.front().end) {
                        result.pseudoClassArgumentSyntaxInvalid = true;
                        break;
                    }
                    StyleRule argument = detail::parseSelector(pseudoStream, arguments.front());
                    if (argument.selectors.size() != 1 || !argument.combinators.empty()) {
                        result.pseudoClassArgumentSyntaxInvalid = true;
                        break;
                    }
                    auto selectorFunction = std::make_shared<StyleSelectorFunction>();
                    selectorFunction->pseudoClass = *functionId;
                    selectorFunction->arguments.push_back(std::move(argument));
                    result.selectorFunctions.push_back(std::move(selectorFunction));
                    break;
                }
                case PseudoClassArgumentSyntax::None:
                    break;
                }
                continue;
            }
            if (!functionName.empty()) {
                result.functionSyntaxUnsupported = true;
                continue;
            }
        }
        if (pseudoClass.empty()) {
            result.pseudoClassArgumentSyntaxInvalid = true;
            continue;
        }
        appendPseudoClass(result, normalizeKeyword(pseudoClass));
    }
}

StyleSelector mergeSelector(const StyleSelector& parent, const StyleSelector& child) {
    StyleSelector result;
    result.universal = child.universal ? true : parent.universal;
    result.attributeSyntaxInvalid = parent.attributeSyntaxInvalid || child.attributeSyntaxInvalid;
    result.idSyntaxInvalid = parent.idSyntaxInvalid || child.idSyntaxInvalid;
    result.classSyntaxInvalid = parent.classSyntaxInvalid || child.classSyntaxInvalid;
    result.pseudoClassArgumentSyntaxInvalid = parent.pseudoClassArgumentSyntaxInvalid || child.pseudoClassArgumentSyntaxInvalid;
    result.functionSyntaxUnsupported = parent.functionSyntaxUnsupported || child.functionSyntaxUnsupported;
    result.element = child.element.empty() ? parent.element : child.element;
    result.attributes = parent.attributes;
    result.attributes.insert(result.attributes.end(), child.attributes.begin(), child.attributes.end());
    result.ids = parent.ids;
    result.ids.insert(result.ids.end(), child.ids.begin(), child.ids.end());
    result.classNames = parent.classNames;
    result.classNames.insert(result.classNames.end(), child.classNames.begin(), child.classNames.end());
    result.pseudoClasses = parent.pseudoClasses;
    result.pseudoClasses.insert(result.pseudoClasses.end(), child.pseudoClasses.begin(), child.pseudoClasses.end());
    result.pseudoClassSyntaxInvalid = parent.pseudoClassSyntaxInvalid || child.pseudoClassSyntaxInvalid;
    result.invalidPseudoClass = parent.pseudoClassSyntaxInvalid ? parent.invalidPseudoClass : child.invalidPseudoClass;
    result.pseudoElementSyntaxInvalid = parent.pseudoElementSyntaxInvalid || child.pseudoElementSyntaxInvalid
        || (!parent.pseudoElement.empty() && !child.pseudoElement.empty());
    result.selectorFunctions = parent.selectorFunctions;
    result.selectorFunctions.insert(result.selectorFunctions.end(), child.selectorFunctions.begin(), child.selectorFunctions.end());
    if (!child.pseudoClasses.empty()) {
        if (!parent.pseudoElement.empty())
            result.pseudoElementSyntaxInvalid = true;
    }
    result.pseudoElement = child.pseudoElement.empty() ? parent.pseudoElement : child.pseudoElement;
    return result;
}

std::optional<StyleAttributeSelector> parseAttributeExpression(const TokenStream& stream, std::size_t open, std::size_t close) {
    const auto& tokens = stream.tokens();
    std::size_t operatorIndex = detail::kNoMatchingToken;
    std::size_t operatorWidth = 0;
    StyleAttributeSelector::Match match = StyleAttributeSelector::Match::Exact;
    for (std::size_t index = open + 1; index < close;) {
        if (isTrivia(tokens[index].kind)) {
            ++index;
            continue;
        }
        if (tokens[index].kind == TokenKind::Function || tokens[index].kind == TokenKind::OpenParen
            || tokens[index].kind == TokenKind::OpenBracket || tokens[index].kind == TokenKind::OpenBrace) {
            index = skipComponent(stream, index, close);
            continue;
        }
        const std::string_view text = stream.text(index);
        const char character = text.size() == 1 ? text.front() : '\0';
        const bool simpleOperator = character == '=';
        const bool compoundOperator = character == '^' || character == '$' || character == '*' || character == '~' || character == '|';
        if (!simpleOperator && !compoundOperator) {
            ++index;
            continue;
        }
        if (operatorIndex != detail::kNoMatchingToken
            || (compoundOperator && (index + 1 >= close || tokens[index + 1].kind != TokenKind::Delim || stream.text(index + 1) != "=")))
            return std::nullopt;
        operatorIndex = index;
        operatorWidth = compoundOperator ? 2 : 1;
        switch (character) {
        case '^':
            match = StyleAttributeSelector::Match::Prefix;
            break;
        case '$':
            match = StyleAttributeSelector::Match::Suffix;
            break;
        case '*':
            match = StyleAttributeSelector::Match::Substring;
            break;
        case '~':
            match = StyleAttributeSelector::Match::IncludesWord;
            break;
        case '|':
            match = StyleAttributeSelector::Match::IncludesHyphen;
            break;
        default:
            break;
        }
        index += operatorWidth;
    }

    StyleAttributeSelector attribute;
    const std::size_t nameEnd = operatorIndex == detail::kNoMatchingToken ? close : operatorIndex;
    const std::string rawName = trim(serializeRange(stream, {open + 1, nameEnd}));
    if (rawName.empty() || !isValidIdentifier(rawName))
        return std::nullopt;
    attribute.name = lower(decodeIdentifier(rawName));
    if (operatorIndex == detail::kNoMatchingToken) {
        attribute.presence = true;
        return attribute;
    }

    const std::string value = trim(serializeRange(stream, {operatorIndex + operatorWidth, close}));
    if (value.empty())
        return std::nullopt;
    const TokenStream valueStream(value);
    std::vector<std::size_t> significant;
    for (std::size_t index = 0; index < valueStream.tokens().size(); ++index)
        if (!isTrivia(valueStream.tokens()[index].kind))
            significant.push_back(index);

    if (significant.size() == 1 && valueStream.tokens()[significant.front()].kind == TokenKind::String) {
        const std::optional<std::string> decoded = detail::decodeString(valueStream.text(significant.front()));
        if (!decoded)
            return std::nullopt;
        attribute.value = *decoded;
    } else if (significant.size() == 2 && valueStream.tokens()[significant.front()].kind == TokenKind::String
        && valueStream.tokens()[significant.back()].kind == TokenKind::Ident) {
        const std::string flag = normalizeKeyword(valueStream.text(significant.back()));
        if (flag != "i" && flag != "s")
            return std::nullopt;
        const std::optional<std::string> decoded = detail::decodeString(valueStream.text(significant.front()));
        if (!decoded)
            return std::nullopt;
        attribute.value = *decoded;
        attribute.caseInsensitive = flag == "i";
        attribute.caseSensitivitySpecified = true;
    } else {
        const std::vector<detail::TokenRange> components = detail::splitComponents(valueStream, {0, valueStream.tokens().size()});
        if (components.empty() || components.size() > 2)
            return std::nullopt;
        const std::string first = trim(detail::serializeRange(valueStream, components.front()));
        if (!isValidIdentifier(first))
            return std::nullopt;
        if (components.size() == 2) {
            const std::string flag = normalizeKeyword(valueStream, components[1]);
            if (flag != "i" && flag != "s")
                return std::nullopt;
            attribute.caseInsensitive = flag == "i";
            attribute.caseSensitivitySpecified = true;
        }
        attribute.value = decodeIdentifier(first);
    }
    attribute.match = match;
    return attribute;
}

void parseAttributeSelector(std::string& token, StyleSelector& result) {
    const TokenStream stream(token);
    std::string remainder;
    for (std::size_t index = 0; index < stream.tokens().size();) {
        const Token& current = stream.tokens()[index];
        if (current.kind == TokenKind::OpenBracket) {
            if (current.matching == detail::kNoMatchingToken) {
                result.attributeSyntaxInvalid = true;
                break;
            }
            const std::optional<StyleAttributeSelector> attribute = parseAttributeExpression(stream, index, current.matching);
            if (!attribute) {
                result.attributeSyntaxInvalid = true;
                break;
            }
            result.attributes.push_back(*attribute);
            index = current.matching + 1;
            continue;
        }
        if (current.kind == TokenKind::Function || current.kind == TokenKind::OpenParen || current.kind == TokenKind::OpenBrace) {
            const std::size_t end = skipComponent(stream, index, stream.tokens().size());
            remainder.append(stream.source().substr(current.begin,
                end == stream.tokens().size() ? stream.source().size() - current.begin : stream.tokens()[end - 1].end - current.begin));
            index = end;
            continue;
        }
        remainder.append(stream.text(index));
        ++index;
    }
    token = std::move(remainder);
}

void appendSelector(StyleRule& destination, const StyleRule& suffix, SelectorCombinator combinator) {
    if (suffix.selectors.empty())
        return;
    destination.combinators.push_back(combinator);
    destination.selectors.insert(destination.selectors.end(), suffix.selectors.begin(), suffix.selectors.end());
    destination.combinators.insert(destination.combinators.end(), suffix.combinators.begin(), suffix.combinators.end());
}

} // namespace

StyleRule detail::expandNestedSelector(const StyleRule& parent, const std::string& rawSelector) {
    const std::string selector = trim(rawSelector);
    StyleRule result = parent;
    if (selector.empty())
        return {};

    if (selector.front() != '&') {
        const std::optional<LeadingSelectorCombinator> leading = leadingSelectorCombinator(selector);
        const std::string suffix = trim(selector.substr(leading ? leading->length : 0));
        if (suffix.empty())
            return {};
        appendSelector(result, detail::parseSelector(suffix), leading ? leading->value : SelectorCombinator::Descendant);
        return result;
    }

    const std::string tail = selector.substr(1);
    if (tail.empty())
        return result;
    if (isWhitespace(tail.front()) || leadingSelectorCombinator(tail)) {
        const std::string trimmedTail = trim(tail);
        const std::optional<LeadingSelectorCombinator> leading = leadingSelectorCombinator(trimmedTail);
        const std::string suffix = trim(trimmedTail.substr(leading ? leading->length : 0));
        if (suffix.empty())
            return {};
        appendSelector(result, detail::parseSelector(suffix), leading ? leading->value : SelectorCombinator::Descendant);
        return result;
    }

    const std::size_t split = consumeSelectorComponent(tail, 0);
    const StyleRule continuation = detail::parseSelector(tail.substr(0, split));
    if (!continuation.selectors.empty() && !result.selectors.empty())
        result.selectors.back() = mergeSelector(result.selectors.back(), continuation.selectors.front());
    if (split < tail.size()) {
        const std::string remainder = trim(tail.substr(split));
        const std::optional<LeadingSelectorCombinator> leading = leadingSelectorCombinator(remainder);
        const std::string suffix = trim(remainder.substr(leading ? leading->length : 0));
        if (suffix.empty())
            return {};
        appendSelector(result, detail::parseSelector(suffix), leading ? leading->value : SelectorCombinator::Descendant);
    }
    return result;
}

namespace {
StyleSelector parseSimpleSelector(const std::string& selectorText) {
    StyleSelector result;
    std::string token = trim(selectorText);

    if (const std::size_t separator = findUnescapedSequence(token, "::"); separator != std::string::npos) {
        std::string pseudoElement = trim(token.substr(separator + 2));
        std::string pseudoElementPseudoClass;
        if (const std::size_t pseudoClassSeparator = findUnescaped(pseudoElement, ':'); pseudoClassSeparator != std::string::npos) {
            const std::string rawPseudoClass = trim(pseudoElement.substr(pseudoClassSeparator + 1));
            pseudoElementPseudoClass = normalizeKeyword(rawPseudoClass);
            pseudoElement.erase(pseudoClassSeparator);
            const auto pseudoClass = findPseudoClass(pseudoElementPseudoClass);
            const PseudoClassDescriptor* descriptor = pseudoClass ? pseudoClassDescriptor(*pseudoClass) : nullptr;
            if (pseudoElementPseudoClass.empty() || findUnescaped(pseudoElementPseudoClass, ':') != std::string::npos
                || !isValidIdentifier(rawPseudoClass) || !descriptor
                || descriptor->argumentRequirement == PseudoClassArgumentRequirement::Required)
                result.pseudoElementSyntaxInvalid = true;
        }
        if (pseudoElement.empty() || findUnescapedSequence(pseudoElement, "::") != std::string::npos)
            result.pseudoElementSyntaxInvalid = true;
        else if (!isValidIdentifier(pseudoElement))
            result.pseudoElementSyntaxInvalid = true;
        else {
            result.pseudoElement = lower(decodeIdentifier(pseudoElement));
            token.erase(separator);
            if (!pseudoElementPseudoClass.empty() && !result.pseudoElementSyntaxInvalid)
                token += ":" + pseudoElementPseudoClass;
        }
        if (result.pseudoElementSyntaxInvalid)
            token.erase(separator);
    }
    parsePseudoClasses(token, result);
    parseAttributeSelector(token, result);
    const std::size_t idSeparator = findUnescaped(token, '#');
    const std::size_t classSeparator = findUnescaped(token, '.');
    const std::size_t firstIdentity = std::min(idSeparator, classSeparator);
    const std::string element = firstIdentity == std::string_view::npos ? token : token.substr(0, firstIdentity);
    if (firstIdentity != std::string_view::npos) {
        for (std::size_t separator = firstIdentity; separator < token.size();) {
            const char marker = token[separator];
            const std::size_t nextId = findUnescaped(token, '#', separator + 1);
            const std::size_t nextClass = findUnescaped(token, '.', separator + 1);
            const std::size_t next = std::min(nextId, nextClass);
            const std::string value =
                token.substr(separator + 1, next == std::string_view::npos ? std::string::npos : next - separator - 1);
            if (!isValidIdentifier(value))
                if (marker == '#')
                    result.idSyntaxInvalid = true;
                else
                    result.classSyntaxInvalid = true;
            else if (marker == '#')
                result.ids.push_back(decodeIdentifier(value));
            else
                result.classNames.push_back(decodeIdentifier(value));
            if (next == std::string_view::npos)
                break;
            separator = next;
        }
    }
    result.universal = element == "*";
    if (!result.universal) {
        const TokenStream elementStream(element);
        if (elementStream.tokens().size() == 1 && elementStream.tokens().front().kind == TokenKind::Ident)
            result.element = decodeIdentifier(elementStream.text(0));
        else
            result.element = element;
    }
    return result;
}

} // namespace

StyleRule detail::parseSelector(const std::string& selector) {
    const TokenStream stream(selector);
    return detail::parseSelector(stream, {0, stream.tokens().size()});
}

StyleRule detail::parseSelector(const TokenStream& stream, detail::TokenRange range) {
    StyleRule rule;
    if (range.begin > range.end || range.end > stream.tokens().size())
        return rule;
    range = trimRange(stream, range);
    if (range.begin == range.end)
        return rule;

    std::size_t position = range.begin;
    SelectorCombinator pending = SelectorCombinator::Descendant;
    bool expectingComponent = true;
    while (position < range.end) {
        while (position < range.end && isTrivia(stream.tokens()[position].kind))
            ++position;
        if (position >= range.end)
            break;
        if (stream.tokens()[position].kind == TokenKind::Delim
            && (stream.text(position) == ">" || stream.text(position) == "+" || stream.text(position) == "~")) {
            if (expectingComponent)
                return {};
            pending = stream.text(position) == ">" ? SelectorCombinator::Child
                : stream.text(position) == "+"     ? SelectorCombinator::NextSibling
                                                   : SelectorCombinator::SubsequentSibling;
            expectingComponent = true;
            ++position;
            continue;
        }
        if (isColumnCombinator(stream, position, range.end)) {
            if (expectingComponent)
                return {};
            pending = SelectorCombinator::Column;
            expectingComponent = true;
            position += 2;
            continue;
        }

        const std::size_t start = position;
        while (position < range.end) {
            const Token& token = stream.tokens()[position];
            if (isTrivia(token.kind))
                break;
            if (token.kind == TokenKind::Delim
                && (stream.text(position) == ">" || stream.text(position) == "+" || stream.text(position) == "~"
                    || isColumnCombinator(stream, position, range.end)))
                break;
            if (token.kind == TokenKind::Function || token.kind == TokenKind::OpenParen || token.kind == TokenKind::OpenBracket
                || token.kind == TokenKind::OpenBrace) {
                if (token.matching == detail::kNoMatchingToken || token.matching >= range.end)
                    return {};
                position = token.matching + 1;
            } else
                ++position;
        }
        if (position == start)
            return {};
        if (!rule.selectors.empty())
            rule.combinators.push_back(pending);
        rule.selectors.push_back(parseSimpleSelector(serializeRange(stream, {start, position})));
        pending = SelectorCombinator::Descendant;
        expectingComponent = false;
    }
    return expectingComponent ? StyleRule {} : rule;
}
} // namespace Core::CSS
