/**
 * @file componentidentity.h
 * @brief Defines the shared identity and persistence-key rules for UI components.
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

#ifndef RD_COMPONENTIDENTITY_H
#define RD_COMPONENTIDENTITY_H

#include <optional>
#include <string>
#include <string_view>
#include <tuple>
#include <utility>

namespace rdui::viewer {
struct ComponentKey {
    std::string definitionId;
    std::string instanceKey;

    bool operator==(const ComponentKey&) const = default;
    bool operator<(const ComponentKey& other) const { return std::tie(definitionId, instanceKey) < std::tie(other.definitionId, other.instanceKey); }

    bool valid() const { return !definitionId.empty() && validPart(definitionId) && validInstanceKey(instanceKey); }

    std::string persistenceKey() const { return instanceKey.empty() ? definitionId : instanceKey + "@" + definitionId; }

    static std::optional<ComponentKey> fromPersistenceKey(std::string_view value) {
        if (value.empty()) return std::nullopt;

        const std::size_t separator = value.find('@');
        if (separator == std::string_view::npos) {
            ComponentKey result{std::string(value), {}};
            return result.valid() ? std::optional<ComponentKey>(std::move(result)) : std::nullopt;
        }
        if (separator == 0 || separator + 1 >= value.size() || value.find('@', separator + 1) != std::string_view::npos) return std::nullopt;

        ComponentKey result{std::string(value.substr(separator + 1)), std::string(value.substr(0, separator))};
        return result.valid() ? std::optional<ComponentKey>(std::move(result)) : std::nullopt;
    }

private:
    static bool validPart(std::string_view value) {
        return !value.empty() && value.find('/') == std::string_view::npos && value.find('@') == std::string_view::npos;
    }

    static bool validInstanceKey(std::string_view value) { return value.empty() || validPart(value); }
};
} // namespace rdui::viewer
#endif // RD_COMPONENTIDENTITY_H
