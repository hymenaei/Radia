#ifndef LL_RDUI_SKIN_GENERATION_INTERNAL_H
#define LL_RDUI_SKIN_GENERATION_INTERNAL_H

#include "rduilayoutdocument.h"
#include "rduilayoutresourcecompiler.h"
#include "rduiresourceprovider.h"
#include "rduiskingeneration.h"
#include "rduisvg.h"
#include <unordered_map>
#include <utility>

namespace rdui
{
    struct SkinGeneration::Impl
    {
        Impl(ResourceSnapshot resources_value,
             LocalizationCatalog localization_value,
             StyleSheet style_sheet_value,
             std::unordered_map<std::string, SvgIcon> icons_value,
             LayoutDocumentMap layout_documents_value)
            : resources(std::make_shared<const ResourceSnapshot>(std::move(resources_value))),
              localization(std::move(localization_value)),
              style_sheet(std::move(style_sheet_value)),
              icons(std::move(icons_value)),
              layout_documents(std::move(layout_documents_value)),
              layout_compiler(&layout_documents)
        {
        }

        std::shared_ptr<const ResourceSnapshot> resources;
        LocalizationCatalog localization;
        StyleSheet style_sheet;
        std::unordered_map<std::string, SvgIcon> icons;
        LayoutDocumentMap layout_documents;
        LayoutResourceCompiler layout_compiler;
    };
}

#endif // LL_RDUI_SKIN_GENERATION_INTERNAL_H
