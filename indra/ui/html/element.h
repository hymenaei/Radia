/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string>
#include <string_view>
#include "dom/element.h"

namespace radia::ui {
namespace detail {
class ElementDefinitions;
class HTMLElementFactory;
}

namespace html_detail { class FragmentParser; }

class HTMLElement : public Element {
    friend class Surface;
    friend class detail::HTMLElementFactory;
    friend class html_detail::FragmentParser;

protected:
    explicit HTMLElement(std::string_view localName);

public:
    void setKeybinding(std::string keybindingId);

protected:
    void onLocaleChanged(const System& system) override;
    virtual void onKeybindingsChanged(const System& system);

public:
    using Element::textContent;
    std::string textContent() const override;

private:
    void rebuildKeybindingContent(const System& system);

    std::string mKeybindingId;
};
} // namespace radia::ui
