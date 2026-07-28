#ifndef LL_RDUI_SKIN_GENERATION_H
#define LL_RDUI_SKIN_GENERATION_H

#include "rduilocalization.h"
#include "rduistylesheet.h"
#include "rduiviewresult.h"
#include <memory>
#include <string>
#include <vector>

namespace rdui
{
    class SkinCompiler;
    class System;
    struct SvgIcon;

    class SkinGeneration final
    {
        public:
            ~SkinGeneration();

            SkinGeneration(const SkinGeneration&) = delete;
            SkinGeneration& operator=(const SkinGeneration&) = delete;

            ViewBuildResult createView(const std::string& resource_id, const std::string& locale) const;

        private:
            struct Impl;
            explicit SkinGeneration(std::unique_ptr<Impl> implementation);

            static std::shared_ptr<const SkinGeneration> empty();
            DiagnosticResult validateWidgetDefaults(const std::string& element) const;
            void validateIconReferences(Widget& widget, ViewBuildResult& result) const;
            std::vector<LocaleInfo> locales() const;
            const std::string& defaultLocale() const;
            const LocaleInfo* locale(const std::string& id) const;
            bool containsLocale(const std::string& id) const;
            bool hasLocalizationKey(const std::string& id) const;
            InlineContent resolveContent(const std::string& locale, const LocalizationRequest& request) const;
            std::string resolveText(const std::string& locale, const LocalizationRequest& request) const;
            const StyleSheet& styleSheet() const;
            const SvgIcon* icon(const std::string& name) const;

            std::unique_ptr<Impl> mImpl;

            friend class SkinCompiler;
            friend class System;
    };
}

#endif // LL_RDUI_SKIN_GENERATION_H
