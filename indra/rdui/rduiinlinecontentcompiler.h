#ifndef LL_RDUI_INLINE_CONTENT_COMPILER_H
#define LL_RDUI_INLINE_CONTENT_COMPILER_H

#include "rduiinlinecontent.h"
#include "rduilayoutdocument.h"
#include "rduiviewresult.h"
#include <string>
#include <vector>

namespace rdui
{
    class ViewBuildContext;

    InlineContent compileInlineContent(const std::vector<LayoutContent>& content,
                                       const std::string& host,
                                       const std::vector<InlineContentKind>& accepted,
                                       ViewBuildResult& result,
                                       const std::string& source,
                                       const ViewBuildContext* context);
}

#endif // LL_RDUI_INLINE_CONTENT_COMPILER_H
