/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <string>
#include <string_view>

namespace radia::ui {
enum class HTMLTag : std::uint8_t {
    Unknown,
    Abbr,
    B,
    Button,
    Br,
    Cite,
    Code,
    Dfn,
    Del,
    Div,
    Em,
    Fieldset,
    Floater,
    Head,
    Header,
    I,
    Icon,
    Ins,
    Kbd,
    Label,
    Legend,
    Link,
    Mark,
    Minimize,
    Close,
    Panel,
    Paragraph,
    Q,
    S,
    Small,
    Strong,
    Title,
    U,
    Input,
    Body
};

struct HTMLTagName {
    std::string_view localName;
};

inline constexpr HTMLTagName kAbbrTag{"abbr"};
inline constexpr HTMLTagName kBTag{"b"};
inline constexpr HTMLTagName kButtonTag{"button"};
inline constexpr HTMLTagName kBrTag{"br"};
inline constexpr HTMLTagName kCiteTag{"cite"};
inline constexpr HTMLTagName kCodeTag{"code"};
inline constexpr HTMLTagName kDfnTag{"dfn"};
inline constexpr HTMLTagName kDelTag{"del"};
inline constexpr HTMLTagName kDivTag{"div"};
inline constexpr HTMLTagName kEmTag{"em"};
inline constexpr HTMLTagName kFieldsetTag{"fieldset"};
inline constexpr HTMLTagName kFloaterTag{"floater"};
inline constexpr HTMLTagName kHeadTag{"head"};
inline constexpr HTMLTagName kHeaderTag{"header"};
inline constexpr HTMLTagName kITag{"i"};
inline constexpr HTMLTagName kIconTag{"icon"};
inline constexpr HTMLTagName kInsTag{"ins"};
inline constexpr HTMLTagName kKbdTag{"kbd"};
inline constexpr HTMLTagName kLabelTag{"label"};
inline constexpr HTMLTagName kLegendTag{"legend"};
inline constexpr HTMLTagName kLinkTag{"link"};
inline constexpr HTMLTagName kMarkTag{"mark"};
inline constexpr HTMLTagName kMinimizeTag{"minimize"};
inline constexpr HTMLTagName kCloseTag{"close"};
inline constexpr HTMLTagName kPanelTag{"panel"};
inline constexpr HTMLTagName kParagraphTag{"p"};
inline constexpr HTMLTagName kQTag{"q"};
inline constexpr HTMLTagName kSTag{"s"};
inline constexpr HTMLTagName kSmallTag{"small"};
inline constexpr HTMLTagName kStrongTag{"strong"};
inline constexpr HTMLTagName kTitleTag{"title"};
inline constexpr HTMLTagName kUTag{"u"};
inline constexpr HTMLTagName kInputTag{"input"};
inline constexpr HTMLTagName kBodyTag{"body"};

std::string_view htmlTagName(HTMLTag tag);
HTMLTag lookupHTMLTag(std::string_view name);
bool isVoidHTMLTag(HTMLTag tag);
bool isHTMLNameCharacter(char character);
bool isHTMLWhitespace(char character);
bool containsHTMLWhitespace(std::string_view value);
std::string canonicalizeHTMLName(std::string_view name);
} // namespace radia::ui
