/**
 * @file compiler.h
 * @brief Compiles layered skin resources into immutable runtime generations.
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

#ifndef RD_SKIN_COMPILER_H
#define RD_SKIN_COMPILER_H

#include <memory>
#include "diagnostic.h"
#include "resourceprovider.h"
#include "skin/generation.h"

namespace radia::ui {
struct SkinGenerationPrepareResult : DiagnosticResult {
    bool ok() const { return !hasErrors() && generation != nullptr; }
    std::shared_ptr<const SkinGeneration> generation;
};

class SkinCompiler final {
public:
    SkinGenerationPrepareResult prepare(ResourceSnapshot resources) const;
};
} // namespace radia::ui
#endif // RD_SKIN_COMPILER_H
