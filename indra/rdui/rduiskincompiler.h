#ifndef LL_RDUI_SKIN_COMPILER_H
#define LL_RDUI_SKIN_COMPILER_H

#include "rduidiagnostic.h"
#include "rduiresourceprovider.h"
#include "rduiskingeneration.h"
#include <memory>

namespace rdui
{
    struct SkinGenerationPrepareResult : DiagnosticResult
    {
        bool ok() const { return !hasErrors() && generation != nullptr; }
        std::shared_ptr<const SkinGeneration> generation;
    };

    class SkinCompiler final
    {
        public:
            SkinGenerationPrepareResult prepare(ResourceSnapshot resources) const;
    };
}

#endif // LL_RDUI_SKIN_COMPILER_H
