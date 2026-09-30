/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <iterator>
#include <map>
#include <memory>
#include <variant>
#include "CSSProperties.h"
#include "CSSPseudoSelectors.h"
#include "CSSRules.h"
#include "CSSSelectorParserInternal.h"
#include "CSSTokenStream.h"
#include "HTMLName.h"
#include "ResourceElementDefinition.h"
#include "StyleProperty.h"
#include "StyleSheet.h"

namespace Core::CSS {
namespace {
using detail::decodeIdentifier;
using detail::isTrivia;
using detail::lower;
using detail::skipComponent;
using detail::startsWith;
using detail::Token;
using detail::TokenKind;
using detail::TokenStream;
using detail::trim;
using detail::trimRange;

std::optional<PseudoClass> targetSpecificPseudoClass(const std::vector<PseudoClass>& pseudoClasses) {
    for (const PseudoClass pseudoClass : {PseudoClass::Checked, PseudoClass::Minimized, PseudoClass::Invalid, PseudoClass::Indeterminate})
        if (std::find(pseudoClasses.begin(), pseudoClasses.end(), pseudoClass) != pseudoClasses.end())
            return pseudoClass;
    return std::nullopt;
}

std::optional<std::string> normalizeImportPath(const std::string& currentId, const std::string& requestedPath) {
    if (requestedPath.empty() || requestedPath.front() == '/' || requestedPath.find('\\') != std::string::npos
        || requestedPath.find(':') != std::string::npos || requestedPath.find("//") != std::string::npos) {
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
            if (segment.empty() && start != combined.size())
                return std::nullopt;
        } else if (segment == "..") {
            if (segments.empty())
                return std::nullopt;
            segments.pop_back();
        } else
            segments.push_back(segment);
        if (end == std::string::npos)
            break;
        start = end + 1;
    }
    if (segments.empty())
        return std::nullopt;

    std::string result;
    for (const std::string& segment : segments) {
        if (!result.empty())
            result += '/';
        result += segment;
    }
    constexpr const char* kCssExtension = ".css";
    if (result.size() < 4 || result.compare(result.size() - 4, 4, kCssExtension) != 0)
        return std::nullopt;
    return result;
}

std::string importChain(const std::vector<std::string>& stack, const std::optional<std::string>& tail = std::nullopt) {
    std::string chain;
    for (const std::string& resource : stack) {
        if (!chain.empty())
            chain += " -> ";
        chain += resource;
    }
    if (tail) {
        if (!chain.empty())
            chain += " -> ";
        chain += *tail;
    }
    return chain;
}

void annotateImportDiagnostics(std::vector<Diagnostic>& diagnostics, std::size_t first, const std::string& chain) {
    for (std::size_t index = first; index < diagnostics.size(); ++index)
        if (diagnostics[index].message.find("Import chain:") == std::string::npos && diagnostics[index].code != "stylesheet.import.cycle")
            diagnostics[index].message += " Import chain: " + chain + ".";
}

bool isTopLevelStylesheetTrivia(TokenKind kind) { return isTrivia(kind) || kind == TokenKind::CDO || kind == TokenKind::CDC; }

std::size_t skipStylesheetTrivia(const TokenStream& stream, std::size_t index, std::size_t end) {
    while (index < end && isTrivia(stream.tokens()[index].kind))
        ++index;
    return index;
}

std::size_t skipTopLevelStylesheetTrivia(const TokenStream& stream, std::size_t index, std::size_t end) {
    while (index < end && isTopLevelStylesheetTrivia(stream.tokens()[index].kind))
        ++index;
    return index;
}

std::optional<std::size_t> nextAtRuleBoundary(const TokenStream& stream, std::size_t start, std::size_t end) {
    for (std::size_t index = start; index < end;) {
        if (isTrivia(stream.tokens()[index].kind)) {
            ++index;
            continue;
        }
        if (stream.tokens()[index].kind == TokenKind::OpenBrace || stream.tokens()[index].kind == TokenKind::Semicolon
            || stream.tokens()[index].kind == TokenKind::CloseBrace)
            return index;
        if (stream.tokens()[index].kind == TokenKind::Function || stream.tokens()[index].kind == TokenKind::OpenParen
            || stream.tokens()[index].kind == TokenKind::OpenBracket) {
            index = skipComponent(stream, index, end);
            continue;
        }
        ++index;
    }
    return std::nullopt;
}

std::optional<std::size_t> nextQualifiedRuleBoundary(const TokenStream& stream, std::size_t start, std::size_t end) {
    for (std::size_t index = start; index < end;) {
        if (isTrivia(stream.tokens()[index].kind)) {
            ++index;
            continue;
        }
        if (stream.tokens()[index].kind == TokenKind::OpenBrace || stream.tokens()[index].kind == TokenKind::CloseBrace)
            return index;
        if (stream.tokens()[index].kind == TokenKind::Function || stream.tokens()[index].kind == TokenKind::OpenParen
            || stream.tokens()[index].kind == TokenKind::OpenBracket) {
            index = skipComponent(stream, index, end);
            continue;
        }
        ++index;
    }
    return std::nullopt;
}

enum class AtRuleID : std::uint8_t {
    Unknown,
    Import,
    FontFace
};

AtRuleID atRuleID(std::string_view name) {
    if (name.empty() || name.front() != '@')
        return AtRuleID::Unknown;
    name.remove_prefix(1);
    const std::string decoded = lower(decodeIdentifier(name));
    if (decoded == "import")
        return AtRuleID::Import;
    if (decoded == "font-face")
        return AtRuleID::FontFace;
    return AtRuleID::Unknown;
}

struct ParsedImportTarget {
    std::string path;
    std::size_t next = 0;
};

std::optional<ParsedImportTarget> parseImportTarget(const TokenStream& stream, std::size_t begin, std::size_t end) {
    const auto& tokens = stream.tokens();
    const std::size_t first = skipStylesheetTrivia(stream, begin, end);
    if (first == end)
        return std::nullopt;

    const Token& token = tokens[first];
    if (token.kind == TokenKind::String) {
        const std::optional<std::string> path = detail::decodeString(stream.text(first));
        if (!path)
            return std::nullopt;
        return ParsedImportTarget {*path, first + 1};
    }
    if (token.kind == TokenKind::Url) {
        const std::string raw(stream.text(first));
        const std::size_t open = raw.find('(');
        if (open == std::string::npos || raw.empty() || raw.back() != ')' || open + 1 > raw.size() - 1)
            return std::nullopt;
        return ParsedImportTarget {decodeIdentifier(trim(raw.substr(open + 1, raw.size() - open - 2))), first + 1};
    }
    if (token.kind != TokenKind::Function || token.matching == detail::kNoMatchingToken || token.matching >= end)
        return std::nullopt;

    const std::string functionText(stream.text(first));
    if (functionText.empty() || functionText.back() != '('
        || lower(decodeIdentifier(std::string_view(functionText).substr(0, functionText.size() - 1))) != "url")
        return std::nullopt;

    const std::size_t close = token.matching;
    const std::size_t content = skipStylesheetTrivia(stream, first + 1, close);
    if (content == close || tokens[content].kind != TokenKind::String)
        return std::nullopt;
    const std::size_t afterContent = skipStylesheetTrivia(stream, content + 1, close);
    if (afterContent != close)
        return std::nullopt;
    const std::optional<std::string> path = detail::decodeString(stream.text(content));
    if (!path)
        return std::nullopt;
    return ParsedImportTarget {*path, close + 1};
}

struct ParsedRuleBlock {
    detail::TokenRange selector;
    detail::TokenRange body;
};

struct ParsedFontFaceBlock {
    detail::TokenRange body;
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
    TokenStream stream;
    std::vector<ParsedImport> imports;
    std::vector<ParsedStyleEntry> entries;
};

class StyleSheetModuleGraph {
public:
    StyleSheetModuleGraph(const ResourceLayer& layer, StyleModel& model, StyleSheetLoadResult& result)
        : mLayer(layer)
        , mModel(model)
        , mResult(result) {}

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
        ParsedModule module {id, sourceName, TokenStream(source)};
        const TokenStream& stream = module.stream;
        const auto& tokens = stream.tokens();
        if (const std::optional<std::size_t> offset = stream.unclosedCommentOffset()) {
            const auto [line, column] = detail::sourcePosition(stream.source(), *offset);
            mResult.warning("stylesheet.syntax.unclosed_comment", "Stylesheet comment is not closed.", sourceName, line, column);
        }
        bool sawRule = false;
        std::size_t position = 0;
        while (position < tokens.size()) {
            position = skipTopLevelStylesheetTrivia(stream, position, tokens.size());
            if (position == tokens.size())
                break;

            const Token& token = tokens[position];
            const std::size_t offset = token.begin;
            const auto [line, column] = detail::sourcePosition(stream.source(), offset);
            if (token.kind == TokenKind::AtKeyword) {
                switch (atRuleID(stream.text(position))) {
                case AtRuleID::Import: {
                    const std::optional<std::size_t> boundary = nextAtRuleBoundary(stream, position + 1, tokens.size());
                    const auto recover = [&] {
                        if (!boundary)
                            position = tokens.size();
                        else if (tokens[*boundary].kind == TokenKind::OpenBrace && tokens[*boundary].matching != detail::kNoMatchingToken)
                            position = tokens[*boundary].matching + 1;
                        else
                            position = *boundary + 1;
                    };

                    if (sawRule) {
                        mResult.warning("stylesheet.import.order", "@import must precede all other rules in its module.", sourceName, line,
                            column);
                        recover();
                        continue;
                    }
                    if (!boundary || tokens[*boundary].kind != TokenKind::Semicolon) {
                        mResult.warning("stylesheet.import.syntax", "@import requires a string or url() target followed by ';'.",
                            sourceName, line, column);
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
                        mResult.warning("stylesheet.import.unsupported", "@import conditions and layer modifiers are not supported.",
                            sourceName, line, column);
                        position = *boundary + 1;
                        continue;
                    }
                    if (target->path.empty()) {
                        mResult.warning("stylesheet.import.syntax", "@import requires a non-empty stylesheet path.", sourceName, line,
                            column);
                        position = *boundary + 1;
                        continue;
                    }

                    const std::optional<std::string> importedId = normalizeImportPath(id, target->path);
                    if (!importedId) {
                        mResult.error("stylesheet.import.path_invalid", "Invalid or escaping @import path: " + target->path + ".",
                            sourceName, line, column);
                        return std::nullopt;
                    }
                    mModel.dependencies[sourceName].insert(mLayer.provenanceFor(*importedId));
                    module.imports.push_back({*importedId, target->path, line, column});
                    position = *boundary + 1;
                    continue;
                }
                case AtRuleID::FontFace: {
                    sawRule = true;
                    const std::optional<std::size_t> boundary = nextAtRuleBoundary(stream, position + 1, tokens.size());
                    if (!boundary || tokens[*boundary].kind != TokenKind::OpenBrace) {
                        mResult.warning("stylesheet.font_face.syntax", "@font-face requires a declaration block.", sourceName, line,
                            column);
                        position = boundary ? *boundary + 1 : tokens.size();
                        continue;
                    }
                    if (tokens[*boundary].matching == detail::kNoMatchingToken) {
                        mResult.warning("stylesheet.syntax.unclosed_block", "@font-face block is not closed.", sourceName, line, column);
                        position = tokens.size();
                        continue;
                    }
                    const std::size_t close = tokens[*boundary].matching;
                    module.entries.push_back(ParsedFontFaceBlock {{*boundary + 1, close}, offset});
                    position = close + 1;
                    continue;
                }
                case AtRuleID::Unknown:
                    break;
                }
                sawRule = true;
                const std::optional<std::size_t> boundary = nextAtRuleBoundary(stream, position + 1, tokens.size());
                if (!boundary) {
                    mResult.warning("stylesheet.at_rule.unsupported", "Unsupported stylesheet at-rule.", sourceName, line, column);
                    break;
                }
                if (tokens[*boundary].kind == TokenKind::OpenBrace) {
                    if (tokens[*boundary].matching == detail::kNoMatchingToken) {
                        mResult.warning("stylesheet.syntax.unclosed_block", "Unsupported at-rule block is not closed.", sourceName, line,
                            column);
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
                    "Unexpected content outside a rule: " + trim(serializeRange(stream, {position, tokens.size()})) + ".", sourceName, line,
                    column);
                break;
            }
            if (tokens[*boundary].kind != TokenKind::OpenBrace) {
                mResult.warning("stylesheet.syntax.trailing_content",
                    "Unexpected content outside a rule: " + trim(serializeRange(stream, {position, *boundary})) + ".", sourceName, line,
                    column);
                position = *boundary + 1;
                continue;
            }
            sawRule = true;
            if (tokens[*boundary].matching == detail::kNoMatchingToken) {
                mResult.warning("stylesheet.syntax.unclosed_block", "Rule block is not closed.", sourceName, line, column);
                const detail::TokenRange selector = {position, *boundary};
                if (trim(serializeRange(stream, selector)).empty())
                    mResult.warning("stylesheet.selector.empty", "Rule selector is empty.", sourceName, line, column);
                else
                    module.entries.push_back(ParsedRuleBlock {selector, {*boundary + 1, tokens.size()}});
                break;
            }
            const std::size_t close = tokens[*boundary].matching;
            const detail::TokenRange selector = {position, *boundary};
            if (trim(serializeRange(stream, selector)).empty())
                mResult.warning("stylesheet.selector.empty", "Rule selector is empty.", sourceName, line, column);
            else
                module.entries.push_back(ParsedRuleBlock {selector, {*boundary + 1, close}});
            position = close + 1;
        }
        return module;
    }

    bool ensureParsed(const std::string& id, const std::string& source, const std::string& sourceName,
        std::vector<std::string>& importStack) {
        if (mModules.find(id) != mModules.end())
            return true;
        std::optional<ParsedModule> parsed = parseSyntax(source, id, sourceName);
        if (!parsed)
            return false;
        mModules.emplace(id, std::move(*parsed));
        const ParsedModule& module = mModules.at(id);
        importStack.push_back(id);
        for (const ParsedImport& imported : module.imports) {
            if (std::find(importStack.begin(), importStack.end(), imported.id) != importStack.end()) {
                mResult.error("stylesheet.import.cycle", "Cyclic @import: " + importChain(importStack, imported.id) + ".", sourceName,
                    imported.line, imported.column);
                continue;
            }
            const std::string importedName = mLayer.provenanceFor(imported.id);
            const std::string* importedSource = nullptr;
            if (imported.id == mLayer.entrypoint)
                importedSource = &mLayer.content;
            else if (const auto found = mLayer.modules.find(imported.id); found != mLayer.modules.end())
                importedSource = &found->second;
            if (!importedSource) {
                mResult.error("stylesheet.import.missing",
                    "Imported stylesheet module is missing: " + imported.requestedPath
                        + ". Import chain: " + importChain(importStack, imported.id) + ".",
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
        if (std::find(importStack.begin(), importStack.end(), id) != importStack.end())
            return;
        const auto module = mModules.find(id);
        if (module == mModules.end())
            return;
        importStack.push_back(id);
        for (const ParsedImport& imported : module->second.imports)
            collectModules(imported.id, importStack, moduleVisits);
        moduleVisits.push_back({&module->second, importStack});
        importStack.pop_back();
    }

    const ResourceLayer& mLayer;
    StyleModel& mModel;
    StyleSheetLoadResult& mResult;
    std::map<std::string, ParsedModule> mModules;
};

std::optional<std::string> parseFontFaceFamily(const TokenStream& stream, detail::TokenRange range) {
    range = trimRange(stream, range);
    std::vector<std::size_t> significant;
    for (std::size_t index = range.begin; index < range.end; ++index)
        if (!isTrivia(stream.tokens()[index].kind))
            significant.push_back(index);
    if (significant.size() == 1 && stream.tokens()[significant.front()].kind == TokenKind::String) {
        std::optional<std::string> family = detail::decodeString(stream.text(significant.front()));
        if (family && !family->empty())
            return family;
        return std::nullopt;
    }
    if (significant.empty())
        return std::nullopt;
    for (const std::size_t index : significant)
        if (stream.tokens()[index].kind != TokenKind::Ident)
            return std::nullopt;
    const std::string family = trim(decodeIdentifier(detail::serializeRange(stream, range)));
    return family.empty() ? std::nullopt : std::optional<std::string>(family);
}

using ParsedFontFaceSource = std::variant<FontFaceURL, FontFaceLocal>;

std::optional<std::vector<ParsedFontFaceSource>> parseFontFaceSources(const TokenStream& stream, detail::TokenRange range) {
    std::vector<ParsedFontFaceSource> sources;
    for (const detail::TokenRange sourceRange : detail::splitOnDelimiter(stream, range, ',')) {
        const std::vector<detail::TokenRange> components = detail::splitComponents(stream, sourceRange);
        if (components.empty())
            continue;

        if (const std::optional<detail::FunctionRange> function = detail::parseFunction(stream, components.front());
            function && function->name == "local") {
            if (components.size() != 1)
                continue;
            if (const std::optional<std::string> name = parseFontFaceFamily(stream, function->body))
                sources.emplace_back(FontFaceLocal {*name});
            continue;
        }

        const std::optional<std::string> url = detail::parseUrl(stream, components.front());
        if (!url)
            continue;

        bool supported = true;
        bool hasFormat = false;
        bool hasTech = false;
        for (std::size_t index = 1; index < components.size(); ++index) {
            const std::optional<detail::FunctionRange> function = detail::parseFunction(stream, components[index]);
            if (!function) {
                supported = false;
                break;
            }

            const auto parseHint = [&](detail::TokenRange hintRange) -> std::optional<std::string> {
                hintRange = trimRange(stream, hintRange);
                if (hintRange.end != hintRange.begin + 1)
                    return std::nullopt;
                const Token& token = stream.tokens()[hintRange.begin];
                if (token.kind == TokenKind::Ident)
                    return lower(decodeIdentifier(stream.text(hintRange.begin)));
                if (token.kind == TokenKind::String) {
                    const std::optional<std::string> decoded = detail::decodeString(stream.text(hintRange.begin));
                    if (decoded)
                        return lower(*decoded);
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
                for (const detail::TokenRange hintRange : detail::splitOnDelimiter(stream, function->body, ',')) {
                    const std::optional<std::string> technology = parseHint(hintRange);
                    if (!technology || (*technology != "features-opentype" && *technology != "color-colrv1")) {
                        supported = false;
                        break;
                    }
                }
            } else
                supported = false;

            if (!supported)
                break;
        }
        if (supported)
            sources.emplace_back(FontFaceURL {*url, {}});
    }
    return sources.empty() ? std::nullopt : std::optional<std::vector<ParsedFontFaceSource>>(std::move(sources));
}

std::optional<Style::FontWeight> parseFontFaceWeight(const TokenStream& stream, detail::TokenRange range) {
    range = trimRange(stream, range);
    const auto& tokens = stream.tokens();
    std::vector<std::size_t> significant;
    for (std::size_t index = range.begin; index < range.end; ++index)
        if (!isTrivia(tokens[index].kind))
            significant.push_back(index);
    if (significant.size() != 1)
        return std::nullopt;
    const Token& token = tokens[significant.front()];
    if (token.kind == TokenKind::Number && token.numericValue && std::isfinite(*token.numericValue) && *token.numericValue >= 1.f
        && *token.numericValue <= 1000.f)
        return Style::FontWeight {*token.numericValue};
    if (token.kind != TokenKind::Ident)
        return std::nullopt;
    const std::string keyword = lower(decodeIdentifier(stream.text(significant.front())));
    if (keyword == "normal")
        return Style::FontWeight {400.f};
    if (keyword == "bold")
        return Style::FontWeight {700.f};
    return std::nullopt;
}

std::optional<Style::FontStyle> parseFontFaceStyle(const TokenStream& stream, detail::TokenRange range) {
    range = trimRange(stream, range);
    const auto& tokens = stream.tokens();
    std::vector<std::size_t> significant;
    for (std::size_t index = range.begin; index < range.end; ++index)
        if (!isTrivia(tokens[index].kind))
            significant.push_back(index);
    if (significant.size() != 1 || tokens[significant.front()].kind != TokenKind::Ident)
        return std::nullopt;
    const std::string keyword = lower(decodeIdentifier(stream.text(significant.front())));
    if (keyword == "normal")
        return Style::FontStyle::Normal;
    if (keyword == "italic")
        return Style::FontStyle::Italic;
    if (keyword == "oblique")
        return Style::FontStyle::Oblique;
    return std::nullopt;
}

std::optional<Style::FontWidth> parseFontFaceWidth(const TokenStream& stream, detail::TokenRange range) {
    range = trimRange(stream, range);
    const auto& tokens = stream.tokens();
    std::vector<std::size_t> significant;
    for (std::size_t index = range.begin; index < range.end; ++index)
        if (!isTrivia(tokens[index].kind))
            significant.push_back(index);
    if (significant.size() != 1)
        return std::nullopt;
    const Token& token = tokens[significant.front()];
    if (token.kind == TokenKind::Percentage && token.numericValue && std::isfinite(*token.numericValue) && *token.numericValue >= 0.f)
        return Style::FontWidth {*token.numericValue};
    if (token.kind != TokenKind::Ident)
        return std::nullopt;
    const std::string keyword = lower(decodeIdentifier(stream.text(significant.front())));
    if (keyword == "normal")
        return Style::FontWidth {100.f};
    if (keyword == "ultra-condensed")
        return Style::FontWidth {50.f};
    if (keyword == "extra-condensed")
        return Style::FontWidth {62.5f};
    if (keyword == "condensed")
        return Style::FontWidth {75.f};
    if (keyword == "semi-condensed")
        return Style::FontWidth {87.5f};
    if (keyword == "semi-expanded")
        return Style::FontWidth {112.5f};
    if (keyword == "expanded")
        return Style::FontWidth {125.f};
    if (keyword == "extra-expanded")
        return Style::FontWidth {150.f};
    if (keyword == "ultra-expanded")
        return Style::FontWidth {200.f};
    return std::nullopt;
}

void parseFontFace(const TokenStream& stream, const ParsedFontFaceBlock& block, const std::string& sourceName, StyleOrigin origin,
    StyleSheetLoadResult& result, std::vector<FontFace>& fontFaces) {
    const auto [faceLine, faceColumn] = detail::sourcePosition(stream.source(), block.sourceOffset);
    FontFace face;
    face.origin = origin;
    face.sourceName = sourceName;
    face.line = faceLine;
    face.column = faceColumn;
    bool hasFamily = false;
    bool hasSources = false;

    for (const detail::TokenRange declarationRange : detail::splitOnDelimiter(stream, block.body, ';')) {
        if (declarationRange.begin == declarationRange.end)
            continue;
        const auto& tokens = stream.tokens();
        std::size_t propertyIndex = declarationRange.begin;
        while (propertyIndex < declarationRange.end && isTrivia(tokens[propertyIndex].kind))
            ++propertyIndex;
        const std::size_t declarationOffset = propertyIndex < declarationRange.end ? tokens[propertyIndex].begin : block.sourceOffset;
        const auto [line, column] = detail::sourcePosition(stream.source(), declarationOffset);
        std::size_t colon = propertyIndex;
        if (colon >= declarationRange.end || tokens[colon].kind != TokenKind::Ident) {
            result.warning("stylesheet.font_face.descriptor_invalid", "Invalid @font-face descriptor.", sourceName, line, column);
            continue;
        }
        const std::string property = lower(decodeIdentifier(stream.text(colon)));
        ++colon;
        while (colon < declarationRange.end && isTrivia(tokens[colon].kind))
            ++colon;
        if (colon >= declarationRange.end || tokens[colon].kind != TokenKind::Colon) {
            result.warning("stylesheet.font_face.descriptor_invalid", "Invalid @font-face descriptor: " + property + ".", sourceName, line,
                column);
            continue;
        }
        const detail::TokenRange valueRange = trimRange(stream, {colon + 1, declarationRange.end});
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
                for (const ParsedFontFaceSource& source : *sources)
                    face.sources.push_back(FontFaceSource {source, sourceName, line, column});
                hasSources = true;
                valid = true;
            } else {
                result.error("stylesheet.font_face.source_unsupported", "@font-face src has no supported source.", sourceName, line,
                    column);
                continue;
            }
        } else if (property == "font-style") {
            if (const std::optional<Style::FontStyle> style = parseFontFaceStyle(stream, valueRange)) {
                face.selection.style = *style;
                valid = true;
            }
        } else if (property == "font-weight") {
            if (const std::optional<Style::FontWeight> weight = parseFontFaceWeight(stream, valueRange)) {
                face.selection.weight = *weight;
                valid = true;
            }
        } else if (property == "font-width") {
            if (const std::optional<Style::FontWidth> width = parseFontFaceWidth(stream, valueRange)) {
                face.selection.width = *width;
                valid = true;
            }
        } else {
            result.warning("stylesheet.font_face.descriptor_unsupported", "Unsupported @font-face descriptor: " + property + ".",
                sourceName, line, column);
            continue;
        }
        if (!valid)
            result.warning("stylesheet.font_face.descriptor_invalid", "Invalid @font-face descriptor: " + property + ".", sourceName, line,
                column);
    }

    if (!hasFamily || !hasSources) {
        result.warning("stylesheet.font_face.invalid", "@font-face requires valid font-family and src descriptors.", sourceName, faceLine,
            faceColumn);
        return;
    }
    fontFaces.push_back(std::move(face));
}
} // namespace

StyleSheetLoadResult StyleSheet::loadRadia(const std::string& stylesheetSource, const std::string& sourceName) {
    return loadRadiaLayers({StyleLayer {StyleOrigin::UserAgent, ResourceLayer {sourceName, stylesheetSource}}});
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
        if (result.hasErrors())
            continue;

        auto compileModule = [&](const TokenStream& stream, const ParsedStyleEntry& entry, const std::string& sourceName) {
            if (const auto* rule = std::get_if<ParsedRuleBlock>(&entry))
                candidate.parseBlock(stream, rule->selector, rule->body, {}, styleLayer.origin, result, sourceName);
            else
                parseFontFace(stream, std::get<ParsedFontFaceBlock>(entry), sourceName, styleLayer.origin, result, fontFaces);
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
std::vector<detail::TokenRange> splitSelectorList(const TokenStream& stream, detail::TokenRange range) {
    const std::vector<detail::TokenRange> ranges = detail::splitOnDelimiter(stream, range, ',');
    return ranges.empty() ? std::vector<detail::TokenRange> {range} : ranges;
}

bool validateSelector(StyleRule& rule, const std::string& selector, StyleSheetLoadResult& result, std::string_view source,
    std::size_t sourceOffset, const std::string& sourceName, bool forgiving = false) {
    const auto warning = [&](std::string code, std::string message) {
        if (forgiving)
            return;
        const auto [line, column] = detail::sourcePosition(source, sourceOffset);
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
            warning("stylesheet.selector.pseudo_element_invalid",
                "Pseudo-elements cannot be followed by pseudo-classes: " + selector + ".");
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
            const PseudoClassDescriptor* descriptor = pseudoClassDescriptor(selectorFunction->pseudoClass);
            const bool forgiving = descriptor && descriptor->argumentSyntax == PseudoClassArgumentSyntax::ForgivingSelectorList;
            for (StyleRule& argument : selectorFunction->arguments) {
                StyleSheetLoadResult ignored;
                StyleSheetLoadResult& argumentResult = forgiving ? ignored : result;
                if (validateSelector(argument, selector, argumentResult, source, sourceOffset, sourceName, forgiving))
                    validArguments.push_back(std::move(argument));
                else if (!forgiving)
                    return false;
            }
            selectorFunction->arguments = std::move(validArguments);
        }
        if (forgiving && !component.pseudoElement.empty())
            return false;
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
        const std::optional<PseudoClass> targetedPseudoClass = targetSpecificPseudoClass(component.pseudoClasses);
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
                "Pseudo-class :" + std::string(pseudoClassName(*targetedPseudoClass)) + " never matches " + component.element + ".");
    }
    return true;
}

std::optional<std::size_t> declarationColon(const TokenStream& stream, detail::TokenRange range) {
    for (std::size_t index = range.begin; index < range.end;) {
        if (isTrivia(stream.tokens()[index].kind)) {
            ++index;
            continue;
        }
        if (stream.tokens()[index].kind == TokenKind::Colon)
            return index;
        if (stream.tokens()[index].kind == TokenKind::Function || stream.tokens()[index].kind == TokenKind::OpenParen
            || stream.tokens()[index].kind == TokenKind::OpenBracket || stream.tokens()[index].kind == TokenKind::OpenBrace) {
            index = skipComponent(stream, index, range.end);
            continue;
        }
        ++index;
    }
    return std::nullopt;
}

bool hasInvalidComponent(const TokenStream& stream, detail::TokenRange range) {
    std::size_t rangeEnd = range.end;
    while (rangeEnd < stream.tokens().size() && isTrivia(stream.tokens()[rangeEnd].kind))
        ++rangeEnd;
    const bool endsAtEOF = rangeEnd == stream.tokens().size();
    for (std::size_t index = range.begin; index < range.end; ++index) {
        const Token& token = stream.tokens()[index];
        if (token.kind == TokenKind::BadString || token.kind == TokenKind::BadUrl
            || ((token.kind == TokenKind::CloseParen || token.kind == TokenKind::CloseBracket || token.kind == TokenKind::CloseBrace)
                && token.matching == detail::kNoMatchingToken))
            return true;
        if ((token.kind == TokenKind::Function || token.kind == TokenKind::OpenParen || token.kind == TokenKind::OpenBracket
                || token.kind == TokenKind::OpenBrace)
            && token.matching == detail::kNoMatchingToken && !endsAtEOF)
            return true;
    }
    return false;
}

bool containsFunction(const TokenStream& stream, detail::TokenRange range, std::string_view name) {
    for (std::size_t index = range.begin; index < range.end;) {
        const Token& token = stream.tokens()[index];
        if (token.kind == TokenKind::Function) {
            const std::string_view text = stream.text(index);
            if (!text.empty() && text.back() == '(' && lower(decodeIdentifier(text.substr(0, text.size() - 1))) == name)
                return true;
        }
        if (token.kind == TokenKind::Function || token.kind == TokenKind::OpenParen || token.kind == TokenKind::OpenBracket
            || token.kind == TokenKind::OpenBrace) {
            if (token.matching != detail::kNoMatchingToken && token.matching < range.end
                && containsFunction(stream, {index + 1, token.matching}, name))
                return true;
            index = skipComponent(stream, index, range.end);
        } else
            ++index;
    }
    return false;
}

void parseRuleBody(StyleModel& model, StyleRule& rule, const std::string& selector, const TokenStream& stream, detail::TokenRange bodyRange,
    StyleSheetLoadResult& result, const std::string& sourceName) {
    const auto& tokens = stream.tokens();
    const auto warning = [&](std::string code, std::string message, std::size_t tokenIndex) {
        const std::size_t offset = tokenIndex < tokens.size() ? tokens[tokenIndex].begin : stream.source().size();
        const auto [line, column] = detail::sourcePosition(stream.source(), offset);
        result.warning(std::move(code), std::move(message), sourceName, line, column);
    };
    std::vector<CustomPropertyDeclaration> customProperties;
    std::vector<StyleDeclaration> declarations;
    const auto flushDeclarations = [&] {
        if (declarations.empty() && customProperties.empty())
            return;
        StyleRule declarationRule = rule;
        declarationRule.customProperties = std::move(customProperties);
        declarationRule.declarations = std::move(declarations);
        model.addRule(declarationRule);
        customProperties.clear();
        declarations.clear();
    };
    const auto addDeclaration = [&](detail::TokenRange rawRange) {
        const detail::TokenRange range = trimRange(stream, rawRange);
        if (range.begin == range.end)
            return;
        const std::optional<std::size_t> colon = declarationColon(stream, range);
        if (!colon) {
            warning("stylesheet.declaration.invalid",
                "Declaration requires a property and value: " + trim(detail::serializeRange(stream, range)) + ".", range.begin);
            return;
        }
        const detail::TokenRange nameRange = trimRange(stream, {range.begin, *colon});
        const std::string serializedName = trim(detail::serializeRange(stream, nameRange));
        const bool validName = nameRange.end == nameRange.begin + 1 && nameRange.begin < stream.tokens().size()
            && stream.tokens()[nameRange.begin].kind == TokenKind::Ident;
        if (!validName) {
            warning("stylesheet.declaration.invalid", "Declaration property name must be one CSS identifier: " + serializedName + ".",
                range.begin);
            return;
        }
        const std::string decodedName = decodeIdentifier(stream.text(nameRange.begin));
        const bool customPropertyName = startsWith(decodedName, "--");
        const bool tokenDeclaration = customPropertyName && decodedName.size() > 2;
        if (customPropertyName && !tokenDeclaration) {
            warning("stylesheet.declaration.invalid", "Custom property name must contain a name after --.", range.begin);
            return;
        }
        const std::string name = tokenDeclaration ? decodedName : lower(decodedName);
        if (hasInvalidComponent(stream, range)) {
            warning("stylesheet.declaration.invalid", "Declaration contains invalid CSS syntax.", range.begin);
            return;
        }
        const detail::TokenRange valueRange = trimRange(stream, {*colon + 1, range.end});
        const std::string value = trim(detail::serializeRange(stream, valueRange));
        if (name.empty() || (!tokenDeclaration && (valueRange.begin == valueRange.end || value.empty()))) {
            warning("stylesheet.declaration.invalid", "Declaration property and value must not be empty.", range.begin);
            return;
        }
        if (tokenDeclaration) {
            customProperties.push_back({name, Style::CustomPropertyValue {value}});
            return;
        }
        const auto id = findProperty(name);
        const Style::detail::PropertyDefinition* legacy = Style::detail::findLegacyProperty(name);
        if (!id && !legacy) {
            warning("stylesheet.property.unknown", "Unknown property: " + name + ".", range.begin);
            return;
        }
        if (legacy && legacy->userAgentOnly && rule.origin != StyleOrigin::UserAgent) {
            warning("stylesheet.property.ua_only", "Ignoring UA-only property outside the user-agent stylesheet: " + name + ".",
                range.begin);
            return;
        }
        StyleSheetLoadResult declarationResult;
        if (containsFunction(stream, valueRange, "var"))
            declarations.emplace_back(name, DeferredStyleValue {value});
        else if (auto compiled = StyleModel::compileDeclaration(name, stream, valueRange, selector, declarationResult, sourceName))
            declarations.insert(declarations.end(), std::make_move_iterator(compiled->begin()), std::make_move_iterator(compiled->end()));
        result.append(std::move(declarationResult));
    };

    std::size_t start = bodyRange.begin;
    for (std::size_t index = bodyRange.begin; index < bodyRange.end;) {
        if (isTrivia(tokens[index].kind)) {
            ++index;
            continue;
        }
        if (tokens[index].kind == TokenKind::Semicolon) {
            addDeclaration({start, index});
            start = index + 1;
            ++index;
            continue;
        }
        if (tokens[index].kind == TokenKind::CloseBrace) {
            addDeclaration({start, index});
            warning("stylesheet.syntax.unexpected_close", "Unexpected closing brace in rule body.", index);
            start = index + 1;
            ++index;
            continue;
        }
        if (tokens[index].kind == TokenKind::OpenBrace) {
            if (tokens[index].matching == detail::kNoMatchingToken) {
                warning("stylesheet.syntax.unclosed_block", "Nested rule block is not closed.", index);
                break;
            }
            const std::optional<std::size_t> colon = declarationColon(stream, {start, index});
            if (colon) {
                const detail::TokenRange nameRange = trimRange(stream, {start, *colon});
                if (nameRange.end == nameRange.begin + 1 && tokens[nameRange.begin].kind == TokenKind::Ident
                    && startsWith(decodeIdentifier(stream.text(nameRange.begin)), "--")) {
                    index = tokens[index].matching + 1;
                    continue;
                }
            }
            const std::size_t close = tokens[index].matching;
            const detail::TokenRange nestedSelectorRange = trimRange(stream, {start, index});
            const std::string nestedSelector = trim(detail::serializeRange(stream, nestedSelectorRange));
            flushDeclarations();
            if (nestedSelector.empty())
                warning("stylesheet.selector.empty", "Nested rule selector is empty.", index);
            else
                model.parseBlock(stream, nestedSelectorRange, {index + 1, close}, rule, rule.origin, result, sourceName);
            start = close + 1;
            index = close + 1;
            continue;
        }
        if (tokens[index].kind == TokenKind::Function || tokens[index].kind == TokenKind::OpenParen
            || tokens[index].kind == TokenKind::OpenBracket) {
            index = skipComponent(stream, index, tokens.size());
            continue;
        }
        ++index;
    }
    if (start < bodyRange.end)
        addDeclaration({start, bodyRange.end});
    flushDeclarations();
}
} // namespace

void StyleModel::parseBlock(const TokenStream& stream, detail::TokenRange selectorRange, detail::TokenRange bodyRange,
    const StyleRule& parent, StyleOrigin origin, StyleSheetLoadResult& result, const std::string& sourceName) {
    const std::vector<detail::TokenRange> selectorRanges = splitSelectorList(stream, selectorRange);
    const auto selectorOffset = [&](detail::TokenRange range) {
        return range.begin < stream.tokens().size() ? stream.tokens()[range.begin].begin : stream.source().size();
    };
    if (selectorRanges.size() > 1) {
        for (const detail::TokenRange range : selectorRanges) {
            const std::string selector = trim(detail::serializeRange(stream, range));
            if (selector.empty()) {
                const auto [line, column] = detail::sourcePosition(stream.source(), selectorOffset(range));
                result.warning("stylesheet.selector.invalid", "Selector list contains an empty selector.", sourceName, line, column);
                return;
            }
            const bool nested = !parent.selectors.empty();
            StyleRule candidate = nested ? detail::expandNestedSelector(parent, selector) : detail::parseSelector(stream, range);
            StyleSheetLoadResult validation;
            if (candidate.selectors.empty()
                || !validateSelector(candidate, selector, validation, stream.source(), selectorOffset(range), sourceName)) {
                if (validation.warnings.empty() && validation.errors.empty())
                    result.warning("stylesheet.selector.invalid", "Selector list contains an invalid selector: " + selector + ".",
                        sourceName, detail::sourcePosition(stream.source(), selectorOffset(range)).first,
                        detail::sourcePosition(stream.source(), selectorOffset(range)).second);
                else
                    result.append(std::move(validation));
                return;
            }
        }
        for (const detail::TokenRange range : selectorRanges)
            parseBlock(stream, range, bodyRange, parent, origin, result, sourceName);
        return;
    }

    const std::string selector = trim(detail::serializeRange(stream, selectorRange));
    const bool nested = !parent.selectors.empty();
    StyleRule rule = nested ? detail::expandNestedSelector(parent, selector) : detail::parseSelector(stream, selectorRange);
    rule.origin = origin;
    if (rule.selectors.empty()) {
        const auto [line, column] = detail::sourcePosition(stream.source(), selectorOffset(selectorRange));
        result.warning("stylesheet.selector.empty", "Rule selector is empty.", sourceName, line, column);
        return;
    }

    if (!validateSelector(rule, selector, result, stream.source(), selectorOffset(selectorRange), sourceName))
        return;

    parseRuleBody(*this, rule, selector, stream, bodyRange, result, sourceName);
}
} // namespace Core::CSS
