/**
 * @file inlinecontentcompiler.h
 * @brief Compiles authored Layout Resource content into typed inline text sources.
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

#ifndef RD_TEXT_INLINECONTENTCOMPILER_H
#define RD_TEXT_INLINECONTENTCOMPILER_H

#include <string>
#include <vector>
#include "layout/buildresult.h"
#include "layout/document.h"
#include "text/source.h"

namespace radia::ui {
class LayoutBuildContext;

TextSource compileInlineContent(const std::vector<LayoutContent>& contentItems, const std::string& hostElementName,
                                const std::vector<InlineContentKind>& accepted, LayoutBuildResult& result, const std::string& source,
                                const LayoutBuildContext* context);
} // namespace radia::ui
#endif // RD_TEXT_INLINECONTENTCOMPILER_H
