/**
 * @file source.h
 * @brief
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

#ifndef RD_TEXT_SOURCE_H
#define RD_TEXT_SOURCE_H

#include <functional>
#include <string>
#include <variant>
#include <vector>
#include "localization/value.h"
#include "text/inlinecontent.h"

namespace rdui {
class TextSourceNode {
public:
    struct Literal {
        std::string value;
    };

    struct Localized {
        LocalizationRequest request;
        InlineContent fallback;
    };

    struct Container {
        InlineContentKind kind;
        std::vector<TextSourceNode> children;
    };

    struct Keybinding {
        std::string binding;
        KeybindingPresentation presentation;
    };

    struct Break {};

    struct Link {
        std::string destination;
        std::vector<TextSourceNode> children;
    };

    using Value = std::variant<Literal, Localized, Container, Keybinding, Break, Link>;

    static TextSourceNode text(std::string value);
    static TextSourceNode localized(LocalizationRequest request, InlineContent fallback);
    static TextSourceNode container(InlineContentKind kind, std::vector<TextSourceNode> children);
    static TextSourceNode kbd(std::string binding, KeybindingPresentation presentation = {});
    static TextSourceNode br();
    static TextSourceNode link(std::string destination, std::vector<TextSourceNode> children);

    const Value& value() const { return mValue; }

private:
    explicit TextSourceNode(Value value) : mValue(std::move(value)) {}

    Value mValue;
};

class TextSource {
public:
    TextSource() = default;
    explicit TextSource(std::vector<TextSourceNode> nodes) : mNodes(std::move(nodes)) {}

    static TextSource literal(InlineContent content);
    static TextSource text(std::string value);
    static TextSource localized(LocalizationRequest request, InlineContent fallback);

    const std::vector<TextSourceNode>& nodes() const { return mNodes; }
    InlineContent materialize() const;
    InlineContent materialize(const std::function<InlineContent(const LocalizationRequest&)>& resolve) const;

private:
    std::vector<TextSourceNode> mNodes;
};
} // namespace rdui
#endif // RD_TEXT_SOURCE_H
