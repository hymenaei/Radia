/**
 * @file stylesheet.h
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

#ifndef RD_STYLE_STYLESHEET_H
#define RD_STYLE_STYLESHEET_H

#include <cstdint>
#include <map>
#include <memory>
#include <set>
#include <string>
#include "diagnostic.h"
#include "resourceprovider.h"
#include "style/style.h"

namespace rdui {
class Widget;

struct StyleSheetLoadResult : DiagnosticResult {
    bool ok() const { return !hasErrors(); }
};

class StyleSheet {
public:
    using DependencyMap = ResourceDependencyMap;

    StyleSheet();
    ~StyleSheet();
    StyleSheet(const StyleSheet& other);
    StyleSheet& operator=(const StyleSheet& other);
    StyleSheet(StyleSheet&& other) noexcept;
    StyleSheet& operator=(StyleSheet&& other) noexcept;

    StyleSheetLoadResult loadRadia(const std::string& radia, const std::string& source_name = {});
    StyleSheetLoadResult loadRadiaLayers(const std::vector<ResourceLayer>& layers);
    std::uint64_t generation() const;
    const DependencyMap& dependencies() const;
    bool stateAffectsLayout(WidgetState state) const;
    bool stateAffectsLayout(const Widget& widget, WidgetState state) const;
    bool stateAffectsHitTesting(WidgetState state) const;
    bool stateAffectsHitTesting(const Widget& widget, WidgetState state) const;
    bool stateAffectsDescendants(const Widget& widget, WidgetState state) const;

    Style resolve(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint8_t states) const;
    Style resolvePart(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint8_t owner_states,
                      const std::string& part, uint8_t part_states = 0) const;
    Style resolveWidget(const Widget& widget) const;
    Style resolveWidgetPart(const Widget& owner, const Widget& part) const;
    Style resolveInline(const Widget& owner, const std::string& element, const std::vector<std::string>& inline_ancestors = {}) const;

private:
    struct Impl;
    std::shared_ptr<Impl> mImpl;
};
} // namespace rdui
#endif // RD_STYLE_STYLESHEET_H
