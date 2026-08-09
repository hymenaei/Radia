/**
 * @file floaterplacementstore.h
 * @brief Persists attached and detached Floater placements keyed by instance identity.
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

#ifndef RD_FLOATERPLACEMENTSTORE_H
#define RD_FLOATERPLACEMENTSTORE_H

#include <optional>
#include <string>
#include <unordered_set>
#include <utility>
#include <variant>
#include "llsd.h"

namespace rdui::viewer {
class FloaterInstanceId final {
public:
    explicit FloaterInstanceId(std::string value) : mValue(std::move(value)) {}

    const std::string& value() const { return mValue; }
    bool empty() const { return mValue.empty(); }

private:
    std::string mValue;
};

struct FloaterPlacementSize {
    float width = 0.f;
    float height = 0.f;
};

struct AttachedFloaterPlacement {
    float x = 0.f;
    float y = 0.f;
    std::optional<FloaterPlacementSize> size;
};

struct DetachedFloaterPlacement {
    int x = 0;
    int y = 0;
    int width = 0;
    int height = 0;
    std::string monitor;
    std::optional<FloaterPlacementSize> logicalSize;
};

using FloaterPlacement = std::variant<AttachedFloaterPlacement, DetachedFloaterPlacement>;

class FloaterPlacementStore final {
public:
    class Persistence {
    public:
        virtual ~Persistence() = default;
        virtual LLSD read() const = 0;
        virtual void write(const LLSD& placements) = 0;
    };

    explicit FloaterPlacementStore(Persistence& persistence);

    std::optional<FloaterPlacement> restore(const FloaterInstanceId& identity);
    void save(const FloaterInstanceId& identity, FloaterPlacement placement);

private:
    Persistence& mPersistence;
    std::unordered_set<std::string> mRestoredIdentities;
};
} // namespace rdui::viewer
#endif // RD_FLOATERPLACEMENTSTORE_H
