/**
 * @file stylesheetparser.cpp
 * @brief Parses RSL modules, imports, selectors, declarations, and diagnostics.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include <algorithm>
#include <cctype>
#include <cmath>
#include <iterator>
#include <limits>
#include <map>
#include "layout/schema.h"
#include "style/color.h"
#include "style/model.h"
#include "style/stylesheet.h"
#include "style/syntax.h"
#include "text/inlinecontent.h"
#include "widgets/widgetcontract.h"

namespace radia::ui {
namespace {
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
    return isColorSyntax(value) || startsWith(value, "var(") || startsWith(value, "token(");
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
        || state == "invalid";
}

std::optional<WidgetState> targetSpecificState(const std::string& state) {
    if (state == "checked") return WidgetState::Checked;
    if (state == "minimized") return WidgetState::Minimized;
    if (state == "invalid") return WidgetState::Invalid;
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

std::optional<std::string> normalizeImportPath(const std::string& current, const std::string& requestedPath) {
    if (requestedPath.empty()
        || requestedPath.front() == '/'
        || requestedPath.find('\\') != std::string::npos
        || requestedPath.find(':') != std::string::npos
        || requestedPath.find("//") != std::string::npos) {
        return std::nullopt;
    }

    std::vector<std::string> segments;
    const std::size_t slash = current.rfind('/');
    const std::string combined = (slash == std::string::npos ? std::string() : current.substr(0, slash + 1)) + requestedPath;
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
    constexpr const char* kRadiaExtension = ".radia";
    if (result.size() < 6 || result.compare(result.size() - 6, 6, kRadiaExtension) != 0) return std::nullopt;
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
    result.element = child.element.empty() ? parent.element : child.element;
    result.id = child.id.empty() ? parent.id : child.id;
    result.className = child.className.empty() ? parent.className : child.className;
    result.state = parent.state;
    result.partState = parent.partState;
    if (!child.state.empty()) {
        if (parent.parts.empty()) result.state = child.state;
        else result.partState = child.state;
    }
    if (!child.partState.empty()) result.partState = child.partState;
    result.parts = parent.parts;
    result.parts.insert(result.parts.end(), child.parts.begin(), child.parts.end());
    return result;
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
    if (std::isspace(static_cast<unsigned char>(tail.front())) || tail.front() == '>') {
        const std::string trimmedTail = trim(tail);
        const bool child = !trimmedTail.empty() && trimmedTail.front() == '>';
        const std::string suffix = trim(trimmedTail.substr(child ? 1 : 0));
        appendSelector(result, detail::parseSelector(suffix), child ? SelectorCombinator::Child : SelectorCombinator::Descendant);
        return result;
    }

    std::size_t split = 0;
    while (split < tail.size() && !std::isspace(static_cast<unsigned char>(tail[split])) && tail[split] != '>') ++split;
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

    if (const std::size_t separator = token.find("::"); separator != std::string::npos) {
        std::string partToken = token.substr(separator + 2);
        if (const std::size_t stateSeparator = partToken.rfind(':');
            stateSeparator != std::string::npos && (stateSeparator == 0 || partToken[stateSeparator - 1] != ':')) {
            result.partState = partToken.substr(stateSeparator + 1);
            partToken.erase(stateSeparator);
        }
        result.parts = detail::splitPartPath(partToken);
        token.erase(separator);
    }
    if (const std::size_t separator = token.find(':'); separator != std::string::npos) {
        result.state = token.substr(separator + 1);
        token.erase(separator);
    }
    if (const std::size_t separator = token.find('#'); separator != std::string::npos) {
        result.id = token.substr(separator + 1);
        token.erase(separator);
    }
    if (const std::size_t separator = token.find('.'); separator != std::string::npos) {
        result.className = token.substr(separator + 1);
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
    std::string resourceId;
    std::string requestedPath;
    std::size_t line = 1;
    std::size_t column = 1;
};

struct ParsedModule {
    std::string resourceId;
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
        return ensureParsed(entrypoint, mLayer.source, mLayer.sourceName, importStack);
    }

    template<typename Callback> void visit(const std::string& resourceId, Callback& callback) const {
        std::vector<std::string> importStack;
        std::vector<VisitEntry> moduleVisits;
        collectModules(resourceId, importStack, moduleVisits);

        const auto emit = [&](const VisitEntry& entry, bool emitTokenRules) {
            for (const ParsedRuleBlock& rule : entry.module->rules) {
                if ((rule.selector == ":root") != emitTokenRules) continue;
                const std::size_t firstError = mResult.errors.size();
                callback(rule, entry.module->sourceName);
                if (entry.importChain.size() > 1) {
                    for (std::size_t index = firstError; index < mResult.errors.size(); ++index)
                        if (mResult.errors[index].message.find("Import chain:") == std::string::npos
                            && mResult.errors[index].code != "stylesheet.import.cycle")
                            mResult.errors[index].message += " Import chain: " + importChain(entry.importChain) + ".";
                }
            }
        };
        for (const VisitEntry& entry : moduleVisits) emit(entry, true);
        for (const VisitEntry& entry : moduleVisits) emit(entry, false);
    }

private:
    struct VisitEntry {
        const ParsedModule* module = nullptr;
        std::vector<std::string> importChain;
    };

    std::optional<ParsedModule> parseSyntax(const std::string& source, const std::string& resourceId, const std::string& sourceName) {
        ParsedModule module{resourceId, sourceName};
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
                const std::optional<std::string> importedId = normalizeImportPath(resourceId, requestedPath);
                if (!importedId) {
                    mResult.error("stylesheet.import.path_invalid", "Invalid or escaping @import path: " + requestedPath + ".", sourceName, line,
                                  column);
                    return std::nullopt;
                }
                mModel.dependencies[sourceName].insert(mLayer.sourceNameFor(*importedId));
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

    bool ensureParsed(const std::string& resourceId, const std::string& source, const std::string& sourceName,
                      std::vector<std::string>& importStack) {
        if (mModules.find(resourceId) != mModules.end()) return true;
        const std::optional<ParsedModule> parsed = parseSyntax(source, resourceId, sourceName);
        if (!parsed) return false;
        mModules.emplace(resourceId, *parsed);
        importStack.push_back(resourceId);
        for (const ParsedImport& imported : parsed->imports) {
            if (std::find(importStack.begin(), importStack.end(), imported.resourceId) != importStack.end()) {
                mResult.error("stylesheet.import.cycle", "Cyclic @import: " + importChain(importStack, imported.resourceId) + ".", sourceName,
                              imported.line, imported.column);
                continue;
            }
            const std::string importedName = mLayer.sourceNameFor(imported.resourceId);
            const std::string* importedSource = nullptr;
            if (imported.resourceId == mLayer.entrypoint) importedSource = &mLayer.source;
            else if (const auto found = mLayer.modules.find(imported.resourceId); found != mLayer.modules.end()) importedSource = &found->second;
            if (!importedSource) {
                mResult.error("stylesheet.import.missing",
                              "Imported stylesheet module is missing: "
                                  + imported.requestedPath
                                  + ". Import chain: "
                                  + importChain(importStack, imported.resourceId)
                                  + ".",
                              sourceName, imported.line, imported.column);
                continue;
            }
            const std::size_t firstError = mResult.errors.size();
            ensureParsed(imported.resourceId, *importedSource, importedName, importStack);
            for (std::size_t index = firstError; index < mResult.errors.size(); ++index)
                if (mResult.errors[index].message.find("Import chain:") == std::string::npos
                    && mResult.errors[index].code != "stylesheet.import.cycle")
                    mResult.errors[index].message += " Import chain: " + importChain(importStack, imported.resourceId) + ".";
        }
        importStack.pop_back();
        return !mResult.hasErrors();
    }

    void collectModules(const std::string& resourceId, std::vector<std::string>& importStack, std::vector<VisitEntry>& moduleVisits) const {
        if (std::find(importStack.begin(), importStack.end(), resourceId) != importStack.end()) return;
        const auto module = mModules.find(resourceId);
        if (module == mModules.end()) return;
        importStack.push_back(resourceId);
        for (const ParsedImport& imported : module->second.imports) collectModules(imported.resourceId, importStack, moduleVisits);
        moduleVisits.push_back({&module->second, importStack});
        importStack.pop_back();
    }

    const ResourceLayer& mLayer;
    StyleModel& mModel;
    StyleSheetLoadResult& mResult;
    std::map<std::string, ParsedModule> mModules;
};
} // namespace

std::vector<std::string> detail::splitPartPath(const std::string& part) {
    std::vector<std::string> result;
    std::size_t start = 0;
    while (start < part.size()) {
        const std::size_t separator = part.find("::", start);
        const std::string segment = trim(part.substr(start, separator == std::string::npos ? std::string::npos : separator - start));
        if (!segment.empty()) result.push_back(segment);
        if (separator == std::string::npos) break;
        start = separator + 2;
    }
    return result;
}

StyleRule detail::parseSelector(const std::string& selector) {
    StyleRule rule;
    const std::string input = trim(selector);
    if (input.empty() || input == ":root") return rule;

    std::size_t position = 0;
    SelectorCombinator pending = SelectorCombinator::Descendant;
    while (position < input.size()) {
        while (position < input.size() && std::isspace(static_cast<unsigned char>(input[position]))) ++position;
        if (position >= input.size()) break;
        if (input[position] == '>') {
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

StyleSheetLoadResult StyleSheet::loadRadia(const std::string& stylesheetSource, const std::string& sourceName) {
    return loadRadiaLayers({ResourceLayer{sourceName, stylesheetSource}});
}

StyleSheetLoadResult StyleSheet::loadRadiaLayers(const std::vector<ResourceLayer>& layers) {
    Impl candidate;
    StyleSheetLoadResult result;
    if (layers.empty()) {
        result.error("stylesheet.layers.empty", "No stylesheet layers were provided.");
        return result;
    }
    for (const ResourceLayer& layer : layers) {
        const std::string entrypoint = layer.entrypoint.empty() ? layer.sourceName : layer.entrypoint;
        StyleSheetModuleGraph graph(layer, candidate, result);
        graph.build(entrypoint);
        if (result.hasErrors()) continue;

        auto compileModule = [&](const ParsedRuleBlock& rule, const std::string& sourceName) {
            candidate.parseBlock(rule.selector, rule.body, {}, result, sourceName);
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
        if (!component.id.empty() && !isWidgetIdentifier(component.id)) {
            result.error("stylesheet.selector.id_invalid", "Widget IDs in selectors must be valid identifiers: " + selector + ".", sourceName);
            return false;
        }
        if (!component.className.empty() && !isKebabCaseIdentifier(component.className)) {
            result.error("stylesheet.selector.class_invalid", "Widget classes in selectors must use lowercase kebab-case: " + selector + ".",
                         sourceName);
            return false;
        }
        if (std::any_of(component.parts.begin(), component.parts.end(), [](const std::string& part) { return !isKebabCaseIdentifier(part); })) {
            result.error("stylesheet.selector.part_invalid", "Widget parts in selectors must use lowercase kebab-case: " + selector + ".",
                         sourceName);
            return false;
        }
        if (!isSupportedState(component.state) || !isSupportedState(component.partState)) {
            const std::string& state = !isSupportedState(component.state) ? component.state : component.partState;
            result.error("stylesheet.selector.state_unknown", "Unknown selector state: " + state + ".", sourceName);
            return false;
        }
        if (!declarationComponent && !component.parts.empty()) {
            result.error("stylesheet.selector.part_structural", "Widget parts cannot participate in structural combinators: " + selector + ".",
                         sourceName);
            return false;
        }
        if (component.element.empty()) {
            if (!component.parts.empty() || !component.state.empty() || !component.partState.empty()) {
                result.error("stylesheet.selector.target_required", "Parts and states require an element-qualified selector: " + selector + ".",
                             sourceName);
                return false;
            }
            continue;
        }
        if (isInlineStyleElement(component.element)) {
            if (!component.id.empty() || !component.className.empty()) {
                result.error("stylesheet.selector.inline_identity_unsupported",
                             "Inline style elements do not have Widget IDs or classes: " + selector + ".", sourceName);
                return false;
            }
            component.element = inlineContentElement(InlineContentKind::Kbd);
            if (!component.parts.empty()) {
                result.error("stylesheet.selector.part_unknown", "Unknown style part for " + component.element + ".", sourceName);
                return false;
            }
            continue;
        }
        const WidgetContract* componentOwner = findWidgetContract(component.element);
        if (!componentOwner) {
            result.error("stylesheet.selector.element_unknown", "Unknown widget element: " + component.element + ".", sourceName);
            return false;
        }
        component.element = componentOwner->elementName;
        const CompositePartContract* componentPart =
            component.parts.empty() ? nullptr : findCompositePartContract(*componentOwner, component.parts);
        if (!component.parts.empty() && !componentPart) {
            std::string part;
            for (const std::string& segment : component.parts) {
                if (!part.empty()) part += "::";
                part += segment;
            }
            result.error("stylesheet.selector.part_unknown", "Unknown style part for " + component.element + ": " + part + ".", sourceName);
            return false;
        }
        if (const auto state = targetSpecificState(component.state); state && !producesState(*componentOwner, *state))
            result.warning("stylesheet.selector.state_never_matches",
                           "State :" + component.state + " is never produced by " + component.element + ".", sourceName);
        if (const auto state = targetSpecificState(component.partState); state && componentPart && !producesState(*componentPart, *state))
            result.warning("stylesheet.selector.state_never_matches",
                           "State :" + component.partState + " is never produced by the selected part of " + component.element + ".", sourceName);
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

void parseRuleBody(StyleModel& model, StyleRule& rule, const std::string& selector, const std::string& body, bool rootRule,
                   StyleSheetLoadResult& result, const std::string& sourceName) {
    std::vector<StyleDeclaration> declarations;
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
        const detail::StylePropertyDefinition* descriptor = detail::findStyleProperty(name);
        if (!descriptor) {
            result.error("stylesheet.property.unknown", "Unknown property: " + name + ".", sourceName);
            return;
        }
        if (rootRule) {
            result.error("stylesheet.property.root_unsupported", "Ordinary properties are not allowed in :root: " + name + ".", sourceName);
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
            model.parseBlock(trim(body.substr(fragment->start, *fragment->open - fragment->start)),
                             body.substr(*fragment->open + 1, *fragment->close - *fragment->open - 1), rule, result, sourceName);
            continue;
        }
        addDeclaration(body.substr(fragment->start, fragment->end - fragment->start));
    }
    if (!declarations.empty()) {
        rule.declarations = std::move(declarations);
        model.addRule(rule);
    }
}
} // namespace

void StyleModel::parseBlock(const std::string& selectorText, const std::string& body, const StyleRule& parent, StyleSheetLoadResult& result,
                            const std::string& sourceName) {
    const std::vector<std::string> selectors = splitSelectorList(selectorText);
    if (selectors.size() > 1) {
        for (const std::string& selector : selectors)
            if (!selector.empty()) parseBlock(selector, body, parent, result, sourceName);
        return;
    }

    const std::string selector = trim(selectorText);
    const bool nested = !parent.selectors.empty();
    StyleRule rule = nested ? expandNestedSelector(parent, selector) : detail::parseSelector(selector);
    const bool rootRule = !nested && selector == ":root";
    if (!rootRule && rule.selectors.empty()) {
        result.error("stylesheet.selector.empty", "Rule selector is empty.", sourceName);
        return;
    }

    if (!validateSelector(rule, selector, result, sourceName)) return;

    parseRuleBody(*this, rule, selector, body, rootRule, result, sourceName);
}
} // namespace radia::ui
