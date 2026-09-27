/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <iterator>
#include <limits>
#include <map>
#include <memory>
#include <variant>
#include "css/color.h"
#include "css/rules.h"
#include "css/stylesheet.h"
#include "css/syntax.h"
#include "CSSProperties.h"
#include "CSSPseudoSelectors.h"
#include "html/elementnames.h"
#include "resource/elementdefinition.h"
#include "style/property.h"

namespace radia::ui {
namespace {
using detail::CSSToken;
using detail::CSSTokenKind;
using detail::CSSTokenStream;
using detail::decodeCSSIdentifier;
using detail::isCSSTrivia;
using detail::isCSSWhitespace;
using detail::lower;
using detail::normalizeCSSKeyword;
using detail::skipCSSComponent;
using detail::startsWith;
using detail::trim;
using detail::trimCSSRange;

std::size_t findUnescaped(std::string_view value, char target, std::size_t start = 0) {
    const CSSTokenStream stream(value);
    const auto& tokens = stream.tokens();
    for (std::size_t index = 0; index < tokens.size();) {
        const auto& token = tokens[index];
        if (token.end <= start) {
            ++index;
            continue;
        }
        if (isCSSTrivia(token.kind)) {
            ++index;
            continue;
        }
        if (token.kind == CSSTokenKind::OpenBracket && target == '[' && token.begin >= start) return token.begin;
        if (token.kind == CSSTokenKind::CloseBracket && target == ']' && token.begin >= start) return token.begin;
        if (token.kind == CSSTokenKind::Hash && target == '#' && token.begin >= start) return token.begin;
        if (token.kind == CSSTokenKind::Function || token.kind == CSSTokenKind::OpenParen || token.kind == CSSTokenKind::OpenBracket) {
            index = skipCSSComponent(stream, index, tokens.size());
            continue;
        }
        if (token.begin >= start && token.end == token.begin + 1 && stream.text(index)[0] == target) return token.begin;
        ++index;
    }
    return std::string_view::npos;
}

std::size_t findUnescapedSequence(std::string_view value, std::string_view target, std::size_t start = 0) {
    if (target.empty()) return start <= value.size() ? start : std::string_view::npos;
    const CSSTokenStream stream(value);
    const auto& tokens = stream.tokens();
    for (std::size_t index = 0; index < tokens.size();) {
        const auto& token = tokens[index];
        if (token.end <= start || isCSSTrivia(token.kind)) {
            ++index;
            continue;
        }
        if (token.kind == CSSTokenKind::Function || token.kind == CSSTokenKind::OpenParen || token.kind == CSSTokenKind::OpenBracket) {
            index = skipCSSComponent(stream, index, tokens.size());
            continue;
        }
        if (token.begin >= start && token.begin + target.size() <= value.size() && value.compare(token.begin, target.size(), target) == 0)
            return token.begin;
        ++index;
    }
    return std::string_view::npos;
}

bool isColumnCombinator(const CSSTokenStream& stream, std::size_t index, std::size_t end) {
    if (index + 1 >= end || stream.tokens()[index].kind != CSSTokenKind::Delim || stream.text(index) != "|") return false;
    const CSSToken& next = stream.tokens()[index + 1];
    return next.kind == CSSTokenKind::Delim && stream.text(index + 1) == "|" && next.begin == stream.tokens()[index].end;
}

std::size_t consumeSelectorComponent(std::string_view value, std::size_t position) {
    const CSSTokenStream stream(value);
    const auto& tokens = stream.tokens();
    for (std::size_t index = 0; index < tokens.size();) {
        const auto& token = tokens[index];
        if (token.end <= position) {
            ++index;
            continue;
        }
        if (isCSSTrivia(token.kind)) return token.begin;
        if (token.kind == CSSTokenKind::Delim
            && (stream.text(index) == ">"
                || stream.text(index) == "+"
                || stream.text(index) == "~"
                || isColumnCombinator(stream, index, tokens.size())))
            return token.begin;
        if (token.kind == CSSTokenKind::Function || token.kind == CSSTokenKind::OpenParen || token.kind == CSSTokenKind::OpenBracket) {
            index = skipCSSComponent(stream, index, tokens.size());
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
    if (value.rfind("||", 0) == 0) return LeadingSelectorCombinator{SelectorCombinator::Column, 2};
    if (value.empty()) return std::nullopt;
    if (value.front() == '>') return LeadingSelectorCombinator{SelectorCombinator::Child, 1};
    if (value.front() == '+') return LeadingSelectorCombinator{SelectorCombinator::NextSibling, 1};
    if (value.front() == '~') return LeadingSelectorCombinator{SelectorCombinator::SubsequentSibling, 1};
    return std::nullopt;
}

bool isValidCSSIdentifier(std::string_view value) {
    const CSSTokenStream stream(value);
    if (stream.tokens().size() != 1 || !stream.hasBalancedBlocks()) return false;
    const auto& token = stream.tokens().front();
    return token.kind == CSSTokenKind::Ident && token.begin == 0 && token.end == stream.source().size();
}

void appendPseudoClass(StyleSelector& selector, const std::string& name) {
    const std::optional<CSSPseudoClass> pseudoClass = findCSSPseudoClass(name);
    if (!pseudoClass) {
        selector.pseudoClassSyntaxInvalid = true;
        selector.invalidPseudoClass = name;
        return;
    }
    const CSSPseudoClassDescriptor* descriptor = cssPseudoClassDescriptor(*pseudoClass);
    if (descriptor && descriptor->argumentRequirement != CSSPseudoClassArgumentRequirement::Required) {
        selector.pseudoClasses.push_back(*pseudoClass);
        return;
    }
    selector.pseudoClassArgumentSyntaxInvalid = true;
}

void parsePseudoClasses(std::string& token, StyleSelector& result) {
    const std::size_t separator = findUnescaped(token, ':');
    if (separator == std::string::npos) return;

    const std::string pseudoClasses = token.substr(separator);
    token.erase(separator);
    const CSSTokenStream stream(pseudoClasses);
    const std::vector<detail::CSSTokenRange> ranges = detail::splitCSSOnDelimiter(stream, {0, stream.tokens().size()}, ':');
    if (ranges.empty()) {
        appendPseudoClass(result, normalizeCSSKeyword(pseudoClasses.substr(1)));
        return;
    }

    for (std::size_t index = 1; index < ranges.size(); ++index) {
        const std::string pseudoClass = trim(detail::serializeCSSRange(stream, ranges[index]));
        const CSSTokenStream pseudoStream(pseudoClass);
        const auto& pseudoTokens = pseudoStream.tokens();
        std::vector<std::size_t> significant;
        for (std::size_t token = 0; token < pseudoTokens.size(); ++token)
            if (!isCSSTrivia(pseudoTokens[token].kind)) significant.push_back(token);

        if (significant.size() >= 2 && pseudoTokens[significant.front()].kind == CSSTokenKind::Function) {
            const std::size_t function = significant.front();
            const std::size_t close = pseudoTokens[function].matching;
            const std::size_t last = significant.back();
            const std::string_view functionText = pseudoStream.text(function);
            const std::string functionName = functionText.empty() || functionText.back() != '('
                ? std::string()
                : lower(decodeCSSIdentifier(functionText.substr(0, functionText.size() - 1)));
            const auto functionId = findCSSPseudoClass(functionName);
            const CSSPseudoClassDescriptor* descriptor = functionId ? cssPseudoClassDescriptor(*functionId) : nullptr;
            if (descriptor) {
                if (close == detail::kNoMatchingCSSToken || close != last || descriptor->argumentSyntax == CSSPseudoClassArgumentSyntax::None) {
                    result.pseudoClassArgumentSyntaxInvalid = true;
                    continue;
                }

                switch (descriptor->argumentSyntax) {
                    case CSSPseudoClassArgumentSyntax::Ident: {
                        const std::string value = normalizeCSSKeyword(pseudoStream, {function + 1, close});
                        if (functionId != CSSPseudoClass::Dir || !isValidCSSIdentifier(value)) {
                            result.pseudoClassArgumentSyntaxInvalid = true;
                            break;
                        }
                        auto selectorFunction = std::make_shared<StyleSelectorFunction>();
                        selectorFunction->pseudoClass = *functionId;
                        selectorFunction->identifier = value;
                        result.selectorFunctions.push_back(std::move(selectorFunction));
                        break;
                    }
                    case CSSPseudoClassArgumentSyntax::ForgivingSelectorList: {
                        auto selectorFunction = std::make_shared<StyleSelectorFunction>();
                        selectorFunction->pseudoClass = *functionId;
                        for (const detail::CSSTokenRange argumentRange : detail::splitCSSOnDelimiter(pseudoStream, {function + 1, close}, ',')) {
                            if (argumentRange.begin == argumentRange.end) continue;
                            StyleRule argument = detail::parseSelector(pseudoStream, argumentRange);
                            if (!argument.selectors.empty()) selectorFunction->arguments.push_back(std::move(argument));
                        }
                        result.selectorFunctions.push_back(std::move(selectorFunction));
                        break;
                    }
                    case CSSPseudoClassArgumentSyntax::CompoundSelector: {
                        const std::vector<detail::CSSTokenRange> arguments = detail::splitCSSOnDelimiter(pseudoStream, {function + 1, close}, ',');
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
                    case CSSPseudoClassArgumentSyntax::None: break;
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
        appendPseudoClass(result, normalizeCSSKeyword(pseudoClass));
    }
}

std::optional<CSSPseudoClass> targetSpecificPseudoClass(const std::vector<CSSPseudoClass>& pseudoClasses) {
    for (const CSSPseudoClass pseudoClass :
         {CSSPseudoClass::Checked, CSSPseudoClass::Minimized, CSSPseudoClass::Invalid, CSSPseudoClass::Indeterminate})
        if (std::find(pseudoClasses.begin(), pseudoClasses.end(), pseudoClass) != pseudoClasses.end()) return pseudoClass;
    return std::nullopt;
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

void annotateImportDiagnostics(std::vector<Diagnostic>& diagnostics, std::size_t first, const std::string& chain) {
    for (std::size_t index = first; index < diagnostics.size(); ++index)
        if (diagnostics[index].message.find("Import chain:") == std::string::npos && diagnostics[index].code != "stylesheet.import.cycle")
            diagnostics[index].message += " Import chain: " + chain + ".";
}

bool isTopLevelStylesheetTrivia(CSSTokenKind kind) {
    return isCSSTrivia(kind) || kind == CSSTokenKind::CDO || kind == CSSTokenKind::CDC;
}

std::size_t skipStylesheetTrivia(const CSSTokenStream& stream, std::size_t index, std::size_t end) {
    while (index < end && isCSSTrivia(stream.tokens()[index].kind)) ++index;
    return index;
}

std::size_t skipTopLevelStylesheetTrivia(const CSSTokenStream& stream, std::size_t index, std::size_t end) {
    while (index < end && isTopLevelStylesheetTrivia(stream.tokens()[index].kind)) ++index;
    return index;
}

std::optional<std::size_t> nextAtRuleBoundary(const CSSTokenStream& stream, std::size_t start, std::size_t end) {
    for (std::size_t index = start; index < end;) {
        if (isCSSTrivia(stream.tokens()[index].kind)) {
            ++index;
            continue;
        }
        if (stream.tokens()[index].kind == CSSTokenKind::OpenBrace
            || stream.tokens()[index].kind == CSSTokenKind::Semicolon
            || stream.tokens()[index].kind == CSSTokenKind::CloseBrace)
            return index;
        if (stream.tokens()[index].kind == CSSTokenKind::Function
            || stream.tokens()[index].kind == CSSTokenKind::OpenParen
            || stream.tokens()[index].kind == CSSTokenKind::OpenBracket) {
            index = skipCSSComponent(stream, index, end);
            continue;
        }
        ++index;
    }
    return std::nullopt;
}

std::optional<std::size_t> nextQualifiedRuleBoundary(const CSSTokenStream& stream, std::size_t start, std::size_t end) {
    for (std::size_t index = start; index < end;) {
        if (isCSSTrivia(stream.tokens()[index].kind)) {
            ++index;
            continue;
        }
        if (stream.tokens()[index].kind == CSSTokenKind::OpenBrace || stream.tokens()[index].kind == CSSTokenKind::CloseBrace) return index;
        if (stream.tokens()[index].kind == CSSTokenKind::Function
            || stream.tokens()[index].kind == CSSTokenKind::OpenParen
            || stream.tokens()[index].kind == CSSTokenKind::OpenBracket) {
            index = skipCSSComponent(stream, index, end);
            continue;
        }
        ++index;
    }
    return std::nullopt;
}

enum class CSSAtRuleID : std::uint8_t { Unknown, Import, FontFace };

CSSAtRuleID cssAtRuleID(std::string_view name) {
    if (name.empty() || name.front() != '@') return CSSAtRuleID::Unknown;
    name.remove_prefix(1);
    const std::string decoded = lower(decodeCSSIdentifier(name));
    if (decoded == "import") return CSSAtRuleID::Import;
    if (decoded == "font-face") return CSSAtRuleID::FontFace;
    return CSSAtRuleID::Unknown;
}

struct ParsedImportTarget {
    std::string path;
    std::size_t next = 0;
};

std::optional<ParsedImportTarget> parseImportTarget(const CSSTokenStream& stream, std::size_t begin, std::size_t end) {
    const auto& tokens = stream.tokens();
    const std::size_t first = skipStylesheetTrivia(stream, begin, end);
    if (first == end) return std::nullopt;

    const CSSToken& token = tokens[first];
    if (token.kind == CSSTokenKind::String) {
        const std::optional<std::string> path = detail::decodeCSSString(stream.text(first));
        if (!path) return std::nullopt;
        return ParsedImportTarget{*path, first + 1};
    }
    if (token.kind == CSSTokenKind::Url) {
        const std::string raw(stream.text(first));
        const std::size_t open = raw.find('(');
        if (open == std::string::npos || raw.empty() || raw.back() != ')' || open + 1 > raw.size() - 1) return std::nullopt;
        return ParsedImportTarget{decodeCSSIdentifier(trim(raw.substr(open + 1, raw.size() - open - 2))), first + 1};
    }
    if (token.kind != CSSTokenKind::Function || token.matching == detail::kNoMatchingCSSToken || token.matching >= end) return std::nullopt;

    const std::string functionText(stream.text(first));
    if (functionText.empty()
        || functionText.back() != '('
        || lower(decodeCSSIdentifier(std::string_view(functionText).substr(0, functionText.size() - 1))) != "url")
        return std::nullopt;

    const std::size_t close = token.matching;
    const std::size_t content = skipStylesheetTrivia(stream, first + 1, close);
    if (content == close || tokens[content].kind != CSSTokenKind::String) return std::nullopt;
    const std::size_t afterContent = skipStylesheetTrivia(stream, content + 1, close);
    if (afterContent != close) return std::nullopt;
    const std::optional<std::string> path = detail::decodeCSSString(stream.text(content));
    if (!path) return std::nullopt;
    return ParsedImportTarget{*path, close + 1};
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
    result.pseudoElementSyntaxInvalid =
        parent.pseudoElementSyntaxInvalid || child.pseudoElementSyntaxInvalid || (!parent.pseudoElement.empty() && !child.pseudoElement.empty());
    result.selectorFunctions = parent.selectorFunctions;
    result.selectorFunctions.insert(result.selectorFunctions.end(), child.selectorFunctions.begin(), child.selectorFunctions.end());
    if (!child.pseudoClasses.empty()) {
        if (!parent.pseudoElement.empty()) result.pseudoElementSyntaxInvalid = true;
    }
    result.pseudoElement = child.pseudoElement.empty() ? parent.pseudoElement : child.pseudoElement;
    return result;
}

std::optional<StyleAttributeSelector> parseAttributeExpression(const CSSTokenStream& stream, std::size_t open, std::size_t close) {
    const auto& tokens = stream.tokens();
    std::size_t operatorIndex = detail::kNoMatchingCSSToken;
    std::size_t operatorWidth = 0;
    StyleAttributeSelector::Match match = StyleAttributeSelector::Match::Exact;
    for (std::size_t index = open + 1; index < close;) {
        if (isCSSTrivia(tokens[index].kind)) {
            ++index;
            continue;
        }
        if (tokens[index].kind == CSSTokenKind::Function
            || tokens[index].kind == CSSTokenKind::OpenParen
            || tokens[index].kind == CSSTokenKind::OpenBracket
            || tokens[index].kind == CSSTokenKind::OpenBrace) {
            index = skipCSSComponent(stream, index, close);
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
        if (operatorIndex != detail::kNoMatchingCSSToken
            || (compoundOperator && (index + 1 >= close || tokens[index + 1].kind != CSSTokenKind::Delim || stream.text(index + 1) != "=")))
            return std::nullopt;
        operatorIndex = index;
        operatorWidth = compoundOperator ? 2 : 1;
        switch (character) {
            case '^': match = StyleAttributeSelector::Match::Prefix; break;
            case '$': match = StyleAttributeSelector::Match::Suffix; break;
            case '*': match = StyleAttributeSelector::Match::Substring; break;
            case '~': match = StyleAttributeSelector::Match::IncludesWord; break;
            case '|': match = StyleAttributeSelector::Match::IncludesHyphen; break;
            default: break;
        }
        index += operatorWidth;
    }

    StyleAttributeSelector attribute;
    const std::size_t nameEnd = operatorIndex == detail::kNoMatchingCSSToken ? close : operatorIndex;
    const std::string rawName = trim(serializeCSSRange(stream, {open + 1, nameEnd}));
    if (rawName.empty() || !isValidCSSIdentifier(rawName)) return std::nullopt;
    attribute.name = lower(decodeCSSIdentifier(rawName));
    if (operatorIndex == detail::kNoMatchingCSSToken) {
        attribute.presence = true;
        return attribute;
    }

    const std::string value = trim(serializeCSSRange(stream, {operatorIndex + operatorWidth, close}));
    if (value.empty()) return std::nullopt;
    const CSSTokenStream valueStream(value);
    std::vector<std::size_t> significant;
    for (std::size_t index = 0; index < valueStream.tokens().size(); ++index)
        if (!isCSSTrivia(valueStream.tokens()[index].kind)) significant.push_back(index);

    if (significant.size() == 1 && valueStream.tokens()[significant.front()].kind == CSSTokenKind::String) {
        const std::optional<std::string> decoded = detail::decodeCSSString(valueStream.text(significant.front()));
        if (!decoded) return std::nullopt;
        attribute.value = *decoded;
    } else if (significant.size() == 2
               && valueStream.tokens()[significant.front()].kind == CSSTokenKind::String
               && valueStream.tokens()[significant.back()].kind == CSSTokenKind::Ident) {
        const std::string flag = normalizeCSSKeyword(valueStream.text(significant.back()));
        if (flag != "i" && flag != "s") return std::nullopt;
        const std::optional<std::string> decoded = detail::decodeCSSString(valueStream.text(significant.front()));
        if (!decoded) return std::nullopt;
        attribute.value = *decoded;
        attribute.caseInsensitive = flag == "i";
        attribute.caseSensitivitySpecified = true;
    } else {
        const std::vector<detail::CSSTokenRange> components = detail::splitCSSComponents(valueStream, {0, valueStream.tokens().size()});
        if (components.empty() || components.size() > 2) return std::nullopt;
        const std::string first = trim(detail::serializeCSSRange(valueStream, components.front()));
        if (!isValidCSSIdentifier(first)) return std::nullopt;
        if (components.size() == 2) {
            const std::string flag = normalizeCSSKeyword(valueStream, components[1]);
            if (flag != "i" && flag != "s") return std::nullopt;
            attribute.caseInsensitive = flag == "i";
            attribute.caseSensitivitySpecified = true;
        }
        attribute.value = decodeCSSIdentifier(first);
    }
    attribute.match = match;
    return attribute;
}

void parseAttributeSelector(std::string& token, StyleSelector& result) {
    const CSSTokenStream stream(token);
    std::string remainder;
    for (std::size_t index = 0; index < stream.tokens().size();) {
        const CSSToken& current = stream.tokens()[index];
        if (current.kind == CSSTokenKind::OpenBracket) {
            if (current.matching == detail::kNoMatchingCSSToken) {
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
        if (current.kind == CSSTokenKind::Function || current.kind == CSSTokenKind::OpenParen || current.kind == CSSTokenKind::OpenBrace) {
            const std::size_t end = skipCSSComponent(stream, index, stream.tokens().size());
            remainder.append(stream.source().substr(current.begin,
                                                    end == stream.tokens().size() ? stream.source().size() - current.begin
                                                                                  : stream.tokens()[end - 1].end - current.begin));
            index = end;
            continue;
        }
        remainder.append(stream.text(index));
        ++index;
    }
    token = std::move(remainder);
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
        const std::optional<LeadingSelectorCombinator> leading = leadingSelectorCombinator(selector);
        const std::string suffix = trim(selector.substr(leading ? leading->length : 0));
        if (suffix.empty()) return {};
        appendSelector(result, detail::parseSelector(suffix), leading ? leading->value : SelectorCombinator::Descendant);
        return result;
    }

    const std::string tail = selector.substr(1);
    if (tail.empty()) return result;
    if (isCSSWhitespace(tail.front()) || leadingSelectorCombinator(tail)) {
        const std::string trimmedTail = trim(tail);
        const std::optional<LeadingSelectorCombinator> leading = leadingSelectorCombinator(trimmedTail);
        const std::string suffix = trim(trimmedTail.substr(leading ? leading->length : 0));
        if (suffix.empty()) return {};
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
        if (suffix.empty()) return {};
        appendSelector(result, detail::parseSelector(suffix), leading ? leading->value : SelectorCombinator::Descendant);
    }
    return result;
}

StyleSelector parseSimpleSelector(const std::string& selectorText) {
    StyleSelector result;
    std::string token = trim(selectorText);

    if (const std::size_t separator = findUnescapedSequence(token, "::"); separator != std::string::npos) {
        std::string pseudoElement = trim(token.substr(separator + 2));
        std::string pseudoElementPseudoClass;
        if (const std::size_t pseudoClassSeparator = findUnescaped(pseudoElement, ':'); pseudoClassSeparator != std::string::npos) {
            const std::string rawPseudoClass = trim(pseudoElement.substr(pseudoClassSeparator + 1));
            pseudoElementPseudoClass = normalizeCSSKeyword(rawPseudoClass);
            pseudoElement.erase(pseudoClassSeparator);
            const auto pseudoClass = findCSSPseudoClass(pseudoElementPseudoClass);
            const CSSPseudoClassDescriptor* descriptor = pseudoClass ? cssPseudoClassDescriptor(*pseudoClass) : nullptr;
            if (pseudoElementPseudoClass.empty()
                || findUnescaped(pseudoElementPseudoClass, ':') != std::string::npos
                || !isValidCSSIdentifier(rawPseudoClass)
                || !descriptor
                || descriptor->argumentRequirement == CSSPseudoClassArgumentRequirement::Required)
                result.pseudoElementSyntaxInvalid = true;
        }
        if (pseudoElement.empty() || findUnescapedSequence(pseudoElement, "::") != std::string::npos) result.pseudoElementSyntaxInvalid = true;
        else if (!isValidCSSIdentifier(pseudoElement)) result.pseudoElementSyntaxInvalid = true;
        else {
            result.pseudoElement = lower(decodeCSSIdentifier(pseudoElement));
            token.erase(separator);
            if (!pseudoElementPseudoClass.empty() && !result.pseudoElementSyntaxInvalid) token += ":" + pseudoElementPseudoClass;
        }
        if (result.pseudoElementSyntaxInvalid) token.erase(separator);
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
            const std::string value = token.substr(separator + 1, next == std::string_view::npos ? std::string::npos : next - separator - 1);
            if (!isValidCSSIdentifier(value))
                if (marker == '#') result.idSyntaxInvalid = true;
                else result.classSyntaxInvalid = true;
            else if (marker == '#') result.ids.push_back(decodeCSSIdentifier(value));
            else result.classNames.push_back(decodeCSSIdentifier(value));
            if (next == std::string_view::npos) break;
            separator = next;
        }
    }
    result.universal = element == "*";
    if (!result.universal) {
        const CSSTokenStream elementStream(element);
        if (elementStream.tokens().size() == 1 && elementStream.tokens().front().kind == CSSTokenKind::Ident)
            result.element = decodeCSSIdentifier(elementStream.text(0));
        else result.element = element;
    }
    return result;
}

struct ParsedRuleBlock {
    detail::CSSTokenRange selector;
    detail::CSSTokenRange body;
};

struct ParsedFontFaceBlock {
    detail::CSSTokenRange body;
    std::size_t sourceOffset = 0;
};

using ParsedStyleEntry = std::variant<ParsedRuleBlock, ParsedFontFaceBlock>;

struct ParsedImport {
    std::string id;
    std::string requestedPath;
    std::size_t line = 1;
    std::size_t column = 1;
};

struct ParsedModule {
    std::string id;
    std::string sourceName;
    CSSTokenStream stream;
    std::vector<ParsedImport> imports;
    std::vector<ParsedStyleEntry> entries;
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

        for (const VisitEntry& entry : moduleVisits) {
            for (const ParsedStyleEntry& styleEntry : entry.module->entries) {
                const std::size_t firstWarning = mResult.warnings.size();
                const std::size_t firstError = mResult.errors.size();
                callback(entry.module->stream, styleEntry, entry.module->sourceName);
                if (entry.importChain.size() > 1) {
                    const std::string chain = importChain(entry.importChain);
                    annotateImportDiagnostics(mResult.warnings, firstWarning, chain);
                    annotateImportDiagnostics(mResult.errors, firstError, chain);
                }
            }
        }
    }

private:
    struct VisitEntry {
        const ParsedModule* module = nullptr;
        std::vector<std::string> importChain;
    };

    std::optional<ParsedModule> parseSyntax(const std::string& source, const std::string& id, const std::string& sourceName) {
        ParsedModule module{id, sourceName, CSSTokenStream(source)};
        const CSSTokenStream& stream = module.stream;
        const auto& tokens = stream.tokens();
        if (const std::optional<std::size_t> offset = stream.unclosedCommentOffset()) {
            const auto [line, column] = detail::cssSourcePosition(stream.source(), *offset);
            mResult.warning("stylesheet.syntax.unclosed_comment", "Stylesheet comment is not closed.", sourceName, line, column);
        }
        bool sawRule = false;
        std::size_t position = 0;
        while (position < tokens.size()) {
            position = skipTopLevelStylesheetTrivia(stream, position, tokens.size());
            if (position == tokens.size()) break;

            const CSSToken& token = tokens[position];
            const std::size_t offset = token.begin;
            const auto [line, column] = detail::cssSourcePosition(stream.source(), offset);
            if (token.kind == CSSTokenKind::AtKeyword) {
                switch (cssAtRuleID(stream.text(position))) {
                    case CSSAtRuleID::Import: {
                        const std::optional<std::size_t> boundary = nextAtRuleBoundary(stream, position + 1, tokens.size());
                        const auto recover = [&] {
                            if (!boundary) position = tokens.size();
                            else if (tokens[*boundary].kind == CSSTokenKind::OpenBrace && tokens[*boundary].matching != detail::kNoMatchingCSSToken)
                                position = tokens[*boundary].matching + 1;
                            else position = *boundary + 1;
                        };

                        if (sawRule) {
                            mResult.warning("stylesheet.import.order", "@import must precede all other rules in its module.", sourceName, line,
                                            column);
                            recover();
                            continue;
                        }
                        if (!boundary || tokens[*boundary].kind != CSSTokenKind::Semicolon) {
                            mResult.warning("stylesheet.import.syntax", "@import requires a string or url() target followed by ';'.", sourceName,
                                            line, column);
                            recover();
                            continue;
                        }

                        const std::optional<ParsedImportTarget> target = parseImportTarget(stream, position + 1, *boundary);
                        if (!target) {
                            mResult.warning("stylesheet.import.syntax", "@import requires one valid string or url() target.", sourceName, line,
                                            column);
                            position = *boundary + 1;
                            continue;
                        }
                        if (skipStylesheetTrivia(stream, target->next, *boundary) != *boundary) {
                            mResult.warning("stylesheet.import.unsupported", "@import conditions and layer modifiers are not supported.", sourceName,
                                            line, column);
                            position = *boundary + 1;
                            continue;
                        }
                        if (target->path.empty()) {
                            mResult.warning("stylesheet.import.syntax", "@import requires a non-empty stylesheet path.", sourceName, line, column);
                            position = *boundary + 1;
                            continue;
                        }

                        const std::optional<std::string> importedId = normalizeImportPath(id, target->path);
                        if (!importedId) {
                            mResult.error("stylesheet.import.path_invalid", "Invalid or escaping @import path: " + target->path + ".", sourceName,
                                          line, column);
                            return std::nullopt;
                        }
                        mModel.dependencies[sourceName].insert(mLayer.provenanceFor(*importedId));
                        module.imports.push_back({*importedId, target->path, line, column});
                        position = *boundary + 1;
                        continue;
                    }
                    case CSSAtRuleID::FontFace: {
                        sawRule = true;
                        const std::optional<std::size_t> boundary = nextAtRuleBoundary(stream, position + 1, tokens.size());
                        if (!boundary || tokens[*boundary].kind != CSSTokenKind::OpenBrace) {
                            mResult.warning("stylesheet.font_face.syntax", "@font-face requires a declaration block.", sourceName, line, column);
                            position = boundary ? *boundary + 1 : tokens.size();
                            continue;
                        }
                        if (tokens[*boundary].matching == detail::kNoMatchingCSSToken) {
                            mResult.warning("stylesheet.syntax.unclosed_block", "@font-face block is not closed.", sourceName, line, column);
                            position = tokens.size();
                            continue;
                        }
                        const std::size_t close = tokens[*boundary].matching;
                        module.entries.push_back(ParsedFontFaceBlock{{*boundary + 1, close}, offset});
                        position = close + 1;
                        continue;
                    }
                    case CSSAtRuleID::Unknown: break;
                }
                sawRule = true;
                const std::optional<std::size_t> boundary = nextAtRuleBoundary(stream, position + 1, tokens.size());
                if (!boundary) {
                    mResult.warning("stylesheet.at_rule.unsupported", "Unsupported stylesheet at-rule.", sourceName, line, column);
                    break;
                }
                if (tokens[*boundary].kind == CSSTokenKind::OpenBrace) {
                    if (tokens[*boundary].matching == detail::kNoMatchingCSSToken) {
                        mResult.warning("stylesheet.syntax.unclosed_block", "Unsupported at-rule block is not closed.", sourceName, line, column);
                        break;
                    }
                    mResult.warning("stylesheet.at_rule.unsupported", "Unsupported stylesheet at-rule.", sourceName, line, column);
                    position = tokens[*boundary].matching + 1;
                } else {
                    mResult.warning("stylesheet.at_rule.unsupported", "Unsupported stylesheet at-rule.", sourceName, line, column);
                    position = *boundary + 1;
                }
                continue;
            }

            const std::optional<std::size_t> boundary = nextQualifiedRuleBoundary(stream, position, tokens.size());
            if (!boundary) {
                mResult.warning("stylesheet.syntax.trailing_content",
                                "Unexpected content outside a rule: " + trim(serializeCSSRange(stream, {position, tokens.size()})) + ".", sourceName,
                                line, column);
                break;
            }
            if (tokens[*boundary].kind != CSSTokenKind::OpenBrace) {
                mResult.warning("stylesheet.syntax.trailing_content",
                                "Unexpected content outside a rule: " + trim(serializeCSSRange(stream, {position, *boundary})) + ".", sourceName,
                                line, column);
                position = *boundary + 1;
                continue;
            }
            sawRule = true;
            if (tokens[*boundary].matching == detail::kNoMatchingCSSToken) {
                mResult.warning("stylesheet.syntax.unclosed_block", "Rule block is not closed.", sourceName, line, column);
                const detail::CSSTokenRange selector = {position, *boundary};
                if (trim(serializeCSSRange(stream, selector)).empty())
                    mResult.warning("stylesheet.selector.empty", "Rule selector is empty.", sourceName, line, column);
                else module.entries.push_back(ParsedRuleBlock{selector, {*boundary + 1, tokens.size()}});
                break;
            }
            const std::size_t close = tokens[*boundary].matching;
            const detail::CSSTokenRange selector = {position, *boundary};
            if (trim(serializeCSSRange(stream, selector)).empty())
                mResult.warning("stylesheet.selector.empty", "Rule selector is empty.", sourceName, line, column);
            else module.entries.push_back(ParsedRuleBlock{selector, {*boundary + 1, close}});
            position = close + 1;
        }
        return module;
    }

    bool ensureParsed(const std::string& id, const std::string& source, const std::string& sourceName, std::vector<std::string>& importStack) {
        if (mModules.find(id) != mModules.end()) return true;
        std::optional<ParsedModule> parsed = parseSyntax(source, id, sourceName);
        if (!parsed) return false;
        mModules.emplace(id, std::move(*parsed));
        const ParsedModule& module = mModules.at(id);
        importStack.push_back(id);
        for (const ParsedImport& imported : module.imports) {
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
            const std::size_t firstWarning = mResult.warnings.size();
            const std::size_t firstError = mResult.errors.size();
            ensureParsed(imported.id, *importedSource, importedName, importStack);
            const std::string chain = importChain(importStack, imported.id);
            annotateImportDiagnostics(mResult.warnings, firstWarning, chain);
            annotateImportDiagnostics(mResult.errors, firstError, chain);
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

std::optional<std::string> parseFontFaceFamily(const CSSTokenStream& stream, detail::CSSTokenRange range) {
    range = trimCSSRange(stream, range);
    std::vector<std::size_t> significant;
    for (std::size_t index = range.begin; index < range.end; ++index)
        if (!isCSSTrivia(stream.tokens()[index].kind)) significant.push_back(index);
    if (significant.size() == 1 && stream.tokens()[significant.front()].kind == CSSTokenKind::String) {
        std::optional<std::string> family = detail::decodeCSSString(stream.text(significant.front()));
        if (family && !family->empty()) return family;
        return std::nullopt;
    }
    if (significant.empty()) return std::nullopt;
    for (const std::size_t index : significant)
        if (stream.tokens()[index].kind != CSSTokenKind::Ident) return std::nullopt;
    const std::string family = trim(decodeCSSIdentifier(detail::serializeCSSRange(stream, range)));
    return family.empty() ? std::nullopt : std::optional<std::string>(family);
}

using ParsedFontFaceSource = std::variant<FontFaceURL, FontFaceLocal>;

std::optional<std::vector<ParsedFontFaceSource>> parseFontFaceSources(const CSSTokenStream& stream, detail::CSSTokenRange range) {
    std::vector<ParsedFontFaceSource> sources;
    for (const detail::CSSTokenRange sourceRange : detail::splitCSSOnDelimiter(stream, range, ',')) {
        const std::vector<detail::CSSTokenRange> components = detail::splitCSSComponents(stream, sourceRange);
        if (components.empty()) continue;

        if (const std::optional<detail::CSSFunctionRange> function = detail::parseCSSFunction(stream, components.front());
            function && function->name == "local") {
            if (components.size() != 1) continue;
            if (const std::optional<std::string> name = parseFontFaceFamily(stream, function->body)) sources.emplace_back(FontFaceLocal{*name});
            continue;
        }

        const std::optional<std::string> url = detail::parseCSSUrl(stream, components.front());
        if (!url) continue;

        bool supported = true;
        bool hasFormat = false;
        bool hasTech = false;
        for (std::size_t index = 1; index < components.size(); ++index) {
            const std::optional<detail::CSSFunctionRange> function = detail::parseCSSFunction(stream, components[index]);
            if (!function) {
                supported = false;
                break;
            }

            const auto parseHint = [&](detail::CSSTokenRange hintRange) -> std::optional<std::string> {
                hintRange = trimCSSRange(stream, hintRange);
                if (hintRange.end != hintRange.begin + 1) return std::nullopt;
                const CSSToken& token = stream.tokens()[hintRange.begin];
                if (token.kind == CSSTokenKind::Ident) return lower(decodeCSSIdentifier(stream.text(hintRange.begin)));
                if (token.kind == CSSTokenKind::String) {
                    const std::optional<std::string> decoded = detail::decodeCSSString(stream.text(hintRange.begin));
                    if (decoded) return lower(*decoded);
                }
                return std::nullopt;
            };

            if (function->name == "format") {
                if (hasFormat) {
                    supported = false;
                    break;
                }
                hasFormat = true;
                const std::optional<std::string> format = parseHint(function->body);
                supported = format && (*format == "opentype" || *format == "truetype" || *format == "woff2");
            } else if (function->name == "tech") {
                if (hasTech) {
                    supported = false;
                    break;
                }
                hasTech = true;
                for (const detail::CSSTokenRange hintRange : detail::splitCSSOnDelimiter(stream, function->body, ',')) {
                    const std::optional<std::string> technology = parseHint(hintRange);
                    if (!technology || (*technology != "features-opentype" && *technology != "color-colrv1")) {
                        supported = false;
                        break;
                    }
                }
            } else supported = false;

            if (!supported) break;
        }
        if (supported) sources.emplace_back(FontFaceURL{*url, {}});
    }
    return sources.empty() ? std::nullopt : std::optional<std::vector<ParsedFontFaceSource>>(std::move(sources));
}

std::optional<FontWeight> parseFontFaceWeight(const CSSTokenStream& stream, detail::CSSTokenRange range) {
    range = trimCSSRange(stream, range);
    const auto& tokens = stream.tokens();
    std::vector<std::size_t> significant;
    for (std::size_t index = range.begin; index < range.end; ++index)
        if (!isCSSTrivia(tokens[index].kind)) significant.push_back(index);
    if (significant.size() != 1) return std::nullopt;
    const CSSToken& token = tokens[significant.front()];
    if (token.kind == CSSTokenKind::Number
        && token.numericValue
        && std::isfinite(*token.numericValue)
        && *token.numericValue >= 1.f
        && *token.numericValue <= 1000.f)
        return FontWeight{*token.numericValue};
    if (token.kind != CSSTokenKind::Ident) return std::nullopt;
    const std::string keyword = lower(decodeCSSIdentifier(stream.text(significant.front())));
    if (keyword == "normal") return FontWeight{400.f};
    if (keyword == "bold") return FontWeight{700.f};
    return std::nullopt;
}

std::optional<FontStyle> parseFontFaceStyle(const CSSTokenStream& stream, detail::CSSTokenRange range) {
    range = trimCSSRange(stream, range);
    const auto& tokens = stream.tokens();
    std::vector<std::size_t> significant;
    for (std::size_t index = range.begin; index < range.end; ++index)
        if (!isCSSTrivia(tokens[index].kind)) significant.push_back(index);
    if (significant.size() != 1 || tokens[significant.front()].kind != CSSTokenKind::Ident) return std::nullopt;
    const std::string keyword = lower(decodeCSSIdentifier(stream.text(significant.front())));
    if (keyword == "normal") return FontStyle::Normal;
    if (keyword == "italic") return FontStyle::Italic;
    if (keyword == "oblique") return FontStyle::Oblique;
    return std::nullopt;
}

std::optional<FontWidth> parseFontFaceWidth(const CSSTokenStream& stream, detail::CSSTokenRange range) {
    range = trimCSSRange(stream, range);
    const auto& tokens = stream.tokens();
    std::vector<std::size_t> significant;
    for (std::size_t index = range.begin; index < range.end; ++index)
        if (!isCSSTrivia(tokens[index].kind)) significant.push_back(index);
    if (significant.size() != 1) return std::nullopt;
    const CSSToken& token = tokens[significant.front()];
    if (token.kind == CSSTokenKind::Percentage && token.numericValue && std::isfinite(*token.numericValue) && *token.numericValue >= 0.f)
        return FontWidth{*token.numericValue};
    if (token.kind != CSSTokenKind::Ident) return std::nullopt;
    const std::string keyword = lower(decodeCSSIdentifier(stream.text(significant.front())));
    if (keyword == "normal") return FontWidth{100.f};
    if (keyword == "ultra-condensed") return FontWidth{50.f};
    if (keyword == "extra-condensed") return FontWidth{62.5f};
    if (keyword == "condensed") return FontWidth{75.f};
    if (keyword == "semi-condensed") return FontWidth{87.5f};
    if (keyword == "semi-expanded") return FontWidth{112.5f};
    if (keyword == "expanded") return FontWidth{125.f};
    if (keyword == "extra-expanded") return FontWidth{150.f};
    if (keyword == "ultra-expanded") return FontWidth{200.f};
    return std::nullopt;
}

void parseFontFace(const CSSTokenStream& stream, const ParsedFontFaceBlock& block, const std::string& sourceName, StyleOrigin origin,
                   StyleSheetLoadResult& result, std::vector<FontFace>& fontFaces) {
    const auto [faceLine, faceColumn] = detail::cssSourcePosition(stream.source(), block.sourceOffset);
    FontFace face;
    face.origin = origin;
    face.sourceName = sourceName;
    face.line = faceLine;
    face.column = faceColumn;
    bool hasFamily = false;
    bool hasSources = false;

    for (const detail::CSSTokenRange declarationRange : detail::splitCSSOnDelimiter(stream, block.body, ';')) {
        if (declarationRange.begin == declarationRange.end) continue;
        const auto& tokens = stream.tokens();
        std::size_t propertyIndex = declarationRange.begin;
        while (propertyIndex < declarationRange.end && isCSSTrivia(tokens[propertyIndex].kind)) ++propertyIndex;
        const std::size_t declarationOffset = propertyIndex < declarationRange.end ? tokens[propertyIndex].begin : block.sourceOffset;
        const auto [line, column] = detail::cssSourcePosition(stream.source(), declarationOffset);
        std::size_t colon = propertyIndex;
        if (colon >= declarationRange.end || tokens[colon].kind != CSSTokenKind::Ident) {
            result.warning("stylesheet.font_face.descriptor_invalid", "Invalid @font-face descriptor.", sourceName, line, column);
            continue;
        }
        const std::string property = lower(decodeCSSIdentifier(stream.text(colon)));
        ++colon;
        while (colon < declarationRange.end && isCSSTrivia(tokens[colon].kind)) ++colon;
        if (colon >= declarationRange.end || tokens[colon].kind != CSSTokenKind::Colon) {
            result.warning("stylesheet.font_face.descriptor_invalid", "Invalid @font-face descriptor: " + property + ".", sourceName, line, column);
            continue;
        }
        const detail::CSSTokenRange valueRange = trimCSSRange(stream, {colon + 1, declarationRange.end});
        bool valid = false;
        if (property == "font-family") {
            const std::optional<std::string> family = parseFontFaceFamily(stream, valueRange);
            if (family) {
                face.family = *family;
                hasFamily = true;
                valid = true;
            }
        } else if (property == "src") {
            hasSources = false;
            face.sources.clear();
            const std::optional<std::vector<ParsedFontFaceSource>> sources = parseFontFaceSources(stream, valueRange);
            if (sources) {
                for (const ParsedFontFaceSource& source : *sources) face.sources.push_back(FontFaceSource{source, sourceName, line, column});
                hasSources = true;
                valid = true;
            } else {
                result.error("stylesheet.font_face.source_unsupported", "@font-face src has no supported source.", sourceName, line, column);
                continue;
            }
        } else if (property == "font-style") {
            if (const std::optional<FontStyle> style = parseFontFaceStyle(stream, valueRange)) {
                face.selection.style = *style;
                valid = true;
            }
        } else if (property == "font-weight") {
            if (const std::optional<FontWeight> weight = parseFontFaceWeight(stream, valueRange)) {
                face.selection.weight = *weight;
                valid = true;
            }
        } else if (property == "font-width") {
            if (const std::optional<FontWidth> width = parseFontFaceWidth(stream, valueRange)) {
                face.selection.width = *width;
                valid = true;
            }
        } else {
            result.warning("stylesheet.font_face.descriptor_unsupported", "Unsupported @font-face descriptor: " + property + ".", sourceName, line,
                           column);
            continue;
        }
        if (!valid)
            result.warning("stylesheet.font_face.descriptor_invalid", "Invalid @font-face descriptor: " + property + ".", sourceName, line, column);
    }

    if (!hasFamily || !hasSources) {
        result.warning("stylesheet.font_face.invalid", "@font-face requires valid font-family and src descriptors.", sourceName, faceLine,
                       faceColumn);
        return;
    }
    fontFaces.push_back(std::move(face));
}
} // namespace

StyleRule detail::parseSelector(const std::string& selector) {
    const CSSTokenStream stream(selector);
    return detail::parseSelector(stream, {0, stream.tokens().size()});
}

StyleRule detail::parseSelector(const CSSTokenStream& stream, detail::CSSTokenRange range) {
    StyleRule rule;
    if (range.begin > range.end || range.end > stream.tokens().size()) return rule;
    range = trimCSSRange(stream, range);
    if (range.begin == range.end) return rule;

    std::size_t position = range.begin;
    SelectorCombinator pending = SelectorCombinator::Descendant;
    bool expectingComponent = true;
    while (position < range.end) {
        while (position < range.end && isCSSTrivia(stream.tokens()[position].kind)) ++position;
        if (position >= range.end) break;
        if (stream.tokens()[position].kind == CSSTokenKind::Delim
            && (stream.text(position) == ">" || stream.text(position) == "+" || stream.text(position) == "~")) {
            if (expectingComponent) return {};
            pending = stream.text(position) == ">" ? SelectorCombinator::Child
                : stream.text(position) == "+"     ? SelectorCombinator::NextSibling
                                                   : SelectorCombinator::SubsequentSibling;
            expectingComponent = true;
            ++position;
            continue;
        }
        if (isColumnCombinator(stream, position, range.end)) {
            if (expectingComponent) return {};
            pending = SelectorCombinator::Column;
            expectingComponent = true;
            position += 2;
            continue;
        }

        const std::size_t start = position;
        while (position < range.end) {
            const CSSToken& token = stream.tokens()[position];
            if (isCSSTrivia(token.kind)) break;
            if (token.kind == CSSTokenKind::Delim
                && (stream.text(position) == ">"
                    || stream.text(position) == "+"
                    || stream.text(position) == "~"
                    || isColumnCombinator(stream, position, range.end)))
                break;
            if (token.kind == CSSTokenKind::Function
                || token.kind == CSSTokenKind::OpenParen
                || token.kind == CSSTokenKind::OpenBracket
                || token.kind == CSSTokenKind::OpenBrace) {
                if (token.matching == detail::kNoMatchingCSSToken || token.matching >= range.end) return {};
                position = token.matching + 1;
            } else ++position;
        }
        if (position == start) return {};
        if (!rule.selectors.empty()) rule.combinators.push_back(pending);
        rule.selectors.push_back(parseSimpleSelector(serializeCSSRange(stream, {start, position})));
        pending = SelectorCombinator::Descendant;
        expectingComponent = false;
    }
    return expectingComponent ? StyleRule{} : rule;
}

StyleSheetLoadResult StyleSheet::loadRadia(const std::string& stylesheetSource, const std::string& sourceName) {
    return loadRadiaLayers({StyleLayer{StyleOrigin::UserAgent, ResourceLayer{sourceName, stylesheetSource}}});
}

StyleSheetLoadResult StyleSheet::loadRadiaLayers(const std::vector<StyleLayer>& layers) {
    StyleModel candidate;
    StyleSheetLoadResult result;
    std::vector<FontFace> fontFaces;
    if (layers.empty()) {
        result.error("stylesheet.layers.empty", "No stylesheet layers were provided.");
        return result;
    }
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

        auto compileModule = [&](const CSSTokenStream& stream, const ParsedStyleEntry& entry, const std::string& sourceName) {
            if (const auto* rule = std::get_if<ParsedRuleBlock>(&entry))
                candidate.parseBlock(stream, rule->selector, rule->body, {}, styleLayer.origin, result, sourceName);
            else parseFontFace(stream, std::get<ParsedFontFaceBlock>(entry), sourceName, styleLayer.origin, result, fontFaces);
        };
        graph.visit(entrypoint, compileModule);
    }
    if (result.ok()) {
        auto replacement = std::make_shared<Impl>(std::move(candidate).build());
        replacement->generation = mImpl->generation + 1;
        mImpl = std::move(replacement);
        mFontFaces = std::move(fontFaces);
    }
    return result;
}

namespace {
std::vector<detail::CSSTokenRange> splitSelectorList(const CSSTokenStream& stream, detail::CSSTokenRange range) {
    const std::vector<detail::CSSTokenRange> ranges = detail::splitCSSOnDelimiter(stream, range, ',');
    return ranges.empty() ? std::vector<detail::CSSTokenRange>{range} : ranges;
}

bool validateSelector(StyleRule& rule, const std::string& selector, StyleSheetLoadResult& result, std::string_view source, std::size_t sourceOffset,
                      const std::string& sourceName, bool forgiving = false) {
    const auto warning = [&](std::string code, std::string message) {
        if (forgiving) return;
        const auto [line, column] = detail::cssSourcePosition(source, sourceOffset);
        result.warning(std::move(code), std::move(message), sourceName, line, column);
    };
    for (std::size_t index = 0; index < rule.selectors.size(); ++index) {
        StyleSelector& component = rule.selectors[index];
        const bool declarationComponent = index + 1 == rule.selectors.size();
        if (component.attributeSyntaxInvalid) {
            warning("stylesheet.selector.attribute_invalid", "Invalid CSS attribute selector: " + selector + ".");
            return false;
        }
        if (component.pseudoElementSyntaxInvalid) {
            warning("stylesheet.selector.pseudo_element_invalid", "Pseudo-elements cannot be followed by pseudo-classes: " + selector + ".");
            return false;
        }
        if (component.idSyntaxInvalid) {
            warning("stylesheet.selector.id_invalid", "Element IDs in selectors must use CSS identifier syntax: " + selector + ".");
            return false;
        }
        if (component.classSyntaxInvalid) {
            warning("stylesheet.selector.class_invalid", "Element classes in selectors must use CSS identifier syntax: " + selector + ".");
            return false;
        }
        if (component.pseudoClassArgumentSyntaxInvalid) {
            warning("stylesheet.selector.pseudo_class_invalid", "Invalid pseudo-class function syntax: " + selector + ".");
            return false;
        }
        if (component.pseudoClassSyntaxInvalid) {
            warning("stylesheet.selector.pseudo_class_unknown", "Unknown selector pseudo-class: " + component.invalidPseudoClass + ".");
            return false;
        }
        if (component.functionSyntaxUnsupported) {
            warning("stylesheet.selector.function_unsupported", "Selector functions are not supported: " + selector + ".");
            return false;
        }
        for (const std::shared_ptr<StyleSelectorFunction>& selectorFunction : component.selectorFunctions) {
            std::vector<StyleRule> validArguments;
            validArguments.reserve(selectorFunction->arguments.size());
            const CSSPseudoClassDescriptor* descriptor = cssPseudoClassDescriptor(selectorFunction->pseudoClass);
            const bool forgiving = descriptor && descriptor->argumentSyntax == CSSPseudoClassArgumentSyntax::ForgivingSelectorList;
            for (StyleRule& argument : selectorFunction->arguments) {
                StyleSheetLoadResult ignored;
                StyleSheetLoadResult& argumentResult = forgiving ? ignored : result;
                if (validateSelector(argument, selector, argumentResult, source, sourceOffset, sourceName, forgiving))
                    validArguments.push_back(std::move(argument));
                else if (!forgiving) return false;
            }
            selectorFunction->arguments = std::move(validArguments);
        }
        if (forgiving && !component.pseudoElement.empty()) return false;
        if (!declarationComponent && !component.pseudoElement.empty()) {
            warning("stylesheet.selector.pseudo_element_structural",
                    "Pseudo-elements cannot participate in structural combinators: " + selector + ".");
            return false;
        }
        if (component.element.empty()) {
            if (!component.universal && (!component.attributes.empty() || !component.pseudoElement.empty())) {
                warning("stylesheet.selector.target_required",
                        "Attributes and pseudo-elements require an element-qualified selector: " + selector + ".");
                return false;
            }
            if (component.universal && !component.pseudoElement.empty()) {
                warning("stylesheet.selector.target_required", "Pseudo-elements require an element-qualified selector: " + selector + ".");
                return false;
            }
            continue;
        }
        if (canonicalizeHTMLName(component.element) == HTMLTagName(HTMLTag::Kbd)) {
            if (!component.attributes.empty() || !component.ids.empty() || !component.classNames.empty()) {
                warning("stylesheet.selector.inline_identity_unsupported",
                        "Inline style elements do not have Element IDs, classes, or attributes: " + selector + ".");
                return false;
            }
            component.element = HTMLTagName(HTMLTag::Kbd);
            if (!component.pseudoElement.empty()) {
                warning("stylesheet.selector.pseudo_element_unknown", "Unknown pseudo-element for " + component.element + ".");
                return false;
            }
            continue;
        }
        const HTMLTag componentTag = findHTMLTag(component.element);
        if (componentTag == HTMLTag::Unknown && component.pseudoElement.empty()) {
            warning("stylesheet.selector.element_unknown", "Unknown element in selector: " + component.element + ".");
            return false;
        }
        const std::optional<CSSPseudoClass> targetedPseudoClass = targetSpecificPseudoClass(component.pseudoClasses);
        const ElementSelectorMetadata metadata = inspectElementSelector(componentTag, component.pseudoElement, targetedPseudoClass);
        if (!metadata.known) {
            warning("stylesheet.selector.element_unknown", "Unknown element in selector: " + component.element + ".");
            return false;
        }
        component.element = metadata.elementName;
        if (!metadata.pseudoElementKnown) {
            warning("stylesheet.selector.pseudo_element_unknown",
                    "Unknown pseudo-element for " + component.element + ": " + component.pseudoElement + ".");
            return false;
        }
        if (targetedPseudoClass && !metadata.elementProducesPseudoClass)
            warning("stylesheet.selector.pseudo_class_never_matches",
                    "Pseudo-class :" + std::string(cssPseudoClassName(*targetedPseudoClass)) + " never matches " + component.element + ".");
    }
    return true;
}

std::optional<std::size_t> declarationColon(const CSSTokenStream& stream, detail::CSSTokenRange range) {
    for (std::size_t index = range.begin; index < range.end;) {
        if (isCSSTrivia(stream.tokens()[index].kind)) {
            ++index;
            continue;
        }
        if (stream.tokens()[index].kind == CSSTokenKind::Colon) return index;
        if (stream.tokens()[index].kind == CSSTokenKind::Function
            || stream.tokens()[index].kind == CSSTokenKind::OpenParen
            || stream.tokens()[index].kind == CSSTokenKind::OpenBracket
            || stream.tokens()[index].kind == CSSTokenKind::OpenBrace) {
            index = skipCSSComponent(stream, index, range.end);
            continue;
        }
        ++index;
    }
    return std::nullopt;
}

bool hasInvalidCSSComponent(const CSSTokenStream& stream, detail::CSSTokenRange range) {
    for (std::size_t index = range.begin; index < range.end; ++index) {
        const CSSToken& token = stream.tokens()[index];
        if (token.kind == CSSTokenKind::BadString
            || token.kind == CSSTokenKind::BadUrl
            || ((token.kind == CSSTokenKind::CloseParen || token.kind == CSSTokenKind::CloseBracket || token.kind == CSSTokenKind::CloseBrace)
                && token.matching == detail::kNoMatchingCSSToken))
            return true;
        if ((token.kind == CSSTokenKind::Function
             || token.kind == CSSTokenKind::OpenParen
             || token.kind == CSSTokenKind::OpenBracket
             || token.kind == CSSTokenKind::OpenBrace)
            && token.matching == detail::kNoMatchingCSSToken)
            return true;
    }
    return false;
}

bool containsCSSFunction(const CSSTokenStream& stream, detail::CSSTokenRange range, std::string_view name) {
    for (std::size_t index = range.begin; index < range.end;) {
        const CSSToken& token = stream.tokens()[index];
        if (token.kind == CSSTokenKind::Function) {
            const std::string_view text = stream.text(index);
            if (!text.empty() && text.back() == '(' && lower(decodeCSSIdentifier(text.substr(0, text.size() - 1))) == name) return true;
        }
        if (token.kind == CSSTokenKind::Function
            || token.kind == CSSTokenKind::OpenParen
            || token.kind == CSSTokenKind::OpenBracket
            || token.kind == CSSTokenKind::OpenBrace) {
            if (token.matching != detail::kNoMatchingCSSToken
                && token.matching < range.end
                && containsCSSFunction(stream, {index + 1, token.matching}, name))
                return true;
            index = skipCSSComponent(stream, index, range.end);
        } else ++index;
    }
    return false;
}

void parseRuleBody(StyleModel& model, StyleRule& rule, const std::string& selector, const CSSTokenStream& stream, detail::CSSTokenRange bodyRange,
                   StyleSheetLoadResult& result, const std::string& sourceName) {
    const auto& tokens = stream.tokens();
    const auto warning = [&](std::string code, std::string message, std::size_t tokenIndex) {
        const std::size_t offset = tokenIndex < tokens.size() ? tokens[tokenIndex].begin : stream.source().size();
        const auto [line, column] = detail::cssSourcePosition(stream.source(), offset);
        result.warning(std::move(code), std::move(message), sourceName, line, column);
    };
    std::vector<CustomPropertyDeclaration> customProperties;
    std::vector<StyleDeclaration> declarations;
    const auto flushDeclarations = [&] {
        if (declarations.empty() && customProperties.empty()) return;
        StyleRule declarationRule = rule;
        declarationRule.customProperties = std::move(customProperties);
        declarationRule.declarations = std::move(declarations);
        model.addRule(declarationRule);
        customProperties.clear();
        declarations.clear();
    };
    const auto addDeclaration = [&](detail::CSSTokenRange rawRange) {
        const detail::CSSTokenRange range = trimCSSRange(stream, rawRange);
        if (range.begin == range.end) return;
        const std::optional<std::size_t> colon = declarationColon(stream, range);
        if (!colon) {
            warning("stylesheet.declaration.invalid",
                    "Declaration requires a property and value: " + trim(detail::serializeCSSRange(stream, range)) + ".", range.begin);
            return;
        }
        const detail::CSSTokenRange nameRange = trimCSSRange(stream, {range.begin, *colon});
        const std::string serializedName = trim(detail::serializeCSSRange(stream, nameRange));
        const bool validName = nameRange.end == nameRange.begin + 1
            && nameRange.begin < stream.tokens().size()
            && stream.tokens()[nameRange.begin].kind == CSSTokenKind::Ident;
        if (!validName) {
            warning("stylesheet.declaration.invalid", "Declaration property name must be one CSS identifier: " + serializedName + ".", range.begin);
            return;
        }
        const std::string decodedName = decodeCSSIdentifier(stream.text(nameRange.begin));
        const bool customPropertyName = startsWith(decodedName, "--");
        const bool tokenDeclaration = customPropertyName && decodedName.size() > 2;
        if (customPropertyName && !tokenDeclaration) {
            warning("stylesheet.declaration.invalid", "Custom property name must contain a name after --.", range.begin);
            return;
        }
        const std::string name = tokenDeclaration ? decodedName : lower(decodedName);
        if (hasInvalidCSSComponent(stream, range)) {
            warning("stylesheet.declaration.invalid", "Declaration contains invalid CSS syntax.", range.begin);
            return;
        }
        const detail::CSSTokenRange valueRange = trimCSSRange(stream, {*colon + 1, range.end});
        const std::string value = trim(detail::serializeCSSRange(stream, valueRange));
        if (name.empty() || (!tokenDeclaration && (valueRange.begin == valueRange.end || value.empty()))) {
            warning("stylesheet.declaration.invalid", "Declaration property and value must not be empty.", range.begin);
            return;
        }
        if (tokenDeclaration) {
            customProperties.push_back({name, CustomPropertyValue{value}});
            return;
        }
        const auto id = findProperty(name);
        const detail::StylePropertyDefinition* legacy = detail::findLegacyProperty(name);
        if (!id && !legacy) {
            warning("stylesheet.property.unknown", "Unknown property: " + name + ".", range.begin);
            return;
        }
        if (legacy && legacy->userAgentOnly && rule.origin != StyleOrigin::UserAgent) {
            warning("stylesheet.property.ua_only", "Ignoring UA-only property outside the user-agent stylesheet: " + name + ".", range.begin);
            return;
        }
        StyleSheetLoadResult declarationResult;
        if (containsCSSFunction(stream, valueRange, "var")) declarations.emplace_back(name, DeferredStyleValue{value});
        else if (auto compiled = StyleModel::compileDeclaration(name, stream, valueRange, selector, declarationResult, sourceName))
            declarations.insert(declarations.end(), std::make_move_iterator(compiled->begin()), std::make_move_iterator(compiled->end()));
        result.append(std::move(declarationResult));
    };

    std::size_t start = bodyRange.begin;
    for (std::size_t index = bodyRange.begin; index < bodyRange.end;) {
        if (isCSSTrivia(tokens[index].kind)) {
            ++index;
            continue;
        }
        if (tokens[index].kind == CSSTokenKind::Semicolon) {
            addDeclaration({start, index});
            start = index + 1;
            ++index;
            continue;
        }
        if (tokens[index].kind == CSSTokenKind::CloseBrace) {
            addDeclaration({start, index});
            warning("stylesheet.syntax.unexpected_close", "Unexpected closing brace in rule body.", index);
            start = index + 1;
            ++index;
            continue;
        }
        if (tokens[index].kind == CSSTokenKind::OpenBrace) {
            if (tokens[index].matching == detail::kNoMatchingCSSToken) {
                warning("stylesheet.syntax.unclosed_block", "Nested rule block is not closed.", index);
                break;
            }
            const std::size_t close = tokens[index].matching;
            const detail::CSSTokenRange nestedSelectorRange = trimCSSRange(stream, {start, index});
            const std::string nestedSelector = trim(detail::serializeCSSRange(stream, nestedSelectorRange));
            flushDeclarations();
            if (nestedSelector.empty()) warning("stylesheet.selector.empty", "Nested rule selector is empty.", index);
            else model.parseBlock(stream, nestedSelectorRange, {index + 1, close}, rule, rule.origin, result, sourceName);
            start = close + 1;
            index = close + 1;
            continue;
        }
        if (tokens[index].kind == CSSTokenKind::Function
            || tokens[index].kind == CSSTokenKind::OpenParen
            || tokens[index].kind == CSSTokenKind::OpenBracket) {
            index = skipCSSComponent(stream, index, tokens.size());
            continue;
        }
        ++index;
    }
    if (start < bodyRange.end) addDeclaration({start, bodyRange.end});
    flushDeclarations();
}
} // namespace

void StyleModel::parseBlock(const CSSTokenStream& stream, detail::CSSTokenRange selectorRange, detail::CSSTokenRange bodyRange,
                            const StyleRule& parent, StyleOrigin origin, StyleSheetLoadResult& result, const std::string& sourceName) {
    const std::vector<detail::CSSTokenRange> selectorRanges = splitSelectorList(stream, selectorRange);
    const auto selectorOffset = [&](detail::CSSTokenRange range) {
        return range.begin < stream.tokens().size() ? stream.tokens()[range.begin].begin : stream.source().size();
    };
    if (selectorRanges.size() > 1) {
        for (const detail::CSSTokenRange range : selectorRanges) {
            const std::string selector = trim(detail::serializeCSSRange(stream, range));
            if (selector.empty()) {
                const auto [line, column] = detail::cssSourcePosition(stream.source(), selectorOffset(range));
                result.warning("stylesheet.selector.invalid", "Selector list contains an empty selector.", sourceName, line, column);
                return;
            }
            const bool nested = !parent.selectors.empty();
            StyleRule candidate = nested ? expandNestedSelector(parent, selector) : detail::parseSelector(stream, range);
            StyleSheetLoadResult validation;
            if (candidate.selectors.empty()
                || !validateSelector(candidate, selector, validation, stream.source(), selectorOffset(range), sourceName)) {
                if (validation.warnings.empty() && validation.errors.empty())
                    result.warning("stylesheet.selector.invalid", "Selector list contains an invalid selector: " + selector + ".", sourceName,
                                   detail::cssSourcePosition(stream.source(), selectorOffset(range)).first,
                                   detail::cssSourcePosition(stream.source(), selectorOffset(range)).second);
                else result.append(std::move(validation));
                return;
            }
        }
        for (const detail::CSSTokenRange range : selectorRanges) parseBlock(stream, range, bodyRange, parent, origin, result, sourceName);
        return;
    }

    const std::string selector = trim(detail::serializeCSSRange(stream, selectorRange));
    const bool nested = !parent.selectors.empty();
    StyleRule rule = nested ? expandNestedSelector(parent, selector) : detail::parseSelector(stream, selectorRange);
    rule.origin = origin;
    if (rule.selectors.empty()) {
        const auto [line, column] = detail::cssSourcePosition(stream.source(), selectorOffset(selectorRange));
        result.warning("stylesheet.selector.empty", "Rule selector is empty.", sourceName, line, column);
        return;
    }

    if (!validateSelector(rule, selector, result, stream.source(), selectorOffset(selectorRange), sourceName)) return;

    parseRuleBody(*this, rule, selector, stream, bodyRange, result, sourceName);
}
} // namespace radia::ui
