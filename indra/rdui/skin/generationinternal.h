/**
 * @file generationinternal.h
 * @brief Stores the private compiled data owned by an immutable SkinGeneration.
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

#ifndef RD_SKIN_GENERATIONINTERNAL_H
#define RD_SKIN_GENERATIONINTERNAL_H

#include <unordered_map>
#include <utility>
#include "layout/document.h"
#include "layout/resourcecompiler.h"
#include "render/svg.h"
#include "resourceprovider.h"
#include "skin/generation.h"

namespace rdui {
struct SkinGeneration::Impl {
    Impl(ResourceSnapshot resources_value, LocalizationCatalog localization_value, StyleSheet style_sheet_value,
         std::unordered_map<std::string, SvgIcon> icons_value, LayoutDocumentMap layout_documents_value)
        : resources(std::make_shared<const ResourceSnapshot>(std::move(resources_value))), localization(std::move(localization_value)),
          styleSheet(std::move(style_sheet_value)), icons(std::move(icons_value)), layout_documents(std::move(layout_documents_value)),
          layoutCompiler(&layout_documents) {}

    std::shared_ptr<const ResourceSnapshot> resources;
    LocalizationCatalog localization;
    StyleSheet styleSheet;
    std::unordered_map<std::string, SvgIcon> icons;
    LayoutDocumentMap layout_documents;
    LayoutResourceCompiler layoutCompiler;
};
} // namespace rdui
#endif // RD_SKIN_GENERATIONINTERNAL_H
