#ifndef LL_RDUI_LAYOUT_RESOURCE_COMPILER_H
#define LL_RDUI_LAYOUT_RESOURCE_COMPILER_H

#include "rduilayoutdocument.h"
#include "rduiviewcontract.h"
#include <string>
#include <unordered_map>

namespace rdui
{
    class LayoutResourceCompiler final
    {
        public:
            explicit LayoutResourceCompiler(const LayoutDocumentMap* documents = nullptr);

            ViewBuildResult createFromResource(const std::string& filename,
                                               const ViewBuildContext* context = nullptr) const;
            ViewBuildResult createFromString(const std::string& xml,
                                             const std::string& source_name = {},
                                             const ViewBuildContext* context = nullptr) const;
            DiagnosticResult validateWidgetDefaults(const std::string& element,
                                                     const ViewBuildContext* context = nullptr) const;

        private:
            struct BuildState;
            static std::string normalizeResource(std::string filename);
            std::unique_ptr<Widget> buildDocument(const LayoutDocument& document,
                                                  std::unique_ptr<Widget> root,
                                                  BuildState& state) const;
            std::unique_ptr<Widget> createResourceWidget(const std::string& filename,
                                                         BuildState& state) const;
            void loadWidgetDefaults(const std::string& element, BuildState& state) const;
            void validateViewScope(Widget& scope, BuildState& state,
                                   const std::string& source, bool count_root = true) const;

            const LayoutDocumentMap* mDocuments = nullptr;
            std::unordered_map<std::string, WidgetContract> mWidgetContracts;
    };
}

#endif // LL_RDUI_LAYOUT_RESOURCE_COMPILER_H
