/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <string_view>
#include <vector>
#include "elements/element.h"

namespace radia::ui {
class Document final : public Node {
public:
    explicit Document(ElementPtr documentElement);
    ~Document();

    Document(const Document&) = delete;
    Document& operator=(const Document&) = delete;
    Document(Document&&) = delete;
    Document& operator=(Document&&) = delete;

    Document* asDocument() noexcept override { return this; }
    const Document* asDocument() const noexcept override { return this; }

    Node* firstChild() noexcept override;
    const Node* firstChild() const noexcept override;
    NodeList childNodes() override;
    ConstNodeList childNodes() const override;

    ElementPtr createElement(std::string_view elementName) const;
    Element* documentElement() noexcept;
    const Element* documentElement() const noexcept;
    Element* getElementById(std::string_view id) noexcept;
    const Element* getElementById(std::string_view id) const noexcept;

private:
    friend class Element;

    ElementPtr releaseDocumentElement();

    std::shared_ptr<detail::DocumentIdentity> mIdentity;
    std::vector<std::unique_ptr<Node>> mChildren;
};
} // namespace radia::ui
