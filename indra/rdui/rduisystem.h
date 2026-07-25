#ifndef LL_RDUI_SYSTEM_H
#define LL_RDUI_SYSTEM_H

#include "rduidiagnostic.h"
#include "rduiinlinecontent.h"
#include "rduilocalization.h"
#include "rduistylesheet.h"
#include "rduiviewresult.h"
#include <chrono>
#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <unordered_set>
#include <utility>

namespace rdui
{
    class SkinGeneration;
    class Surface;
    class TextMetrics;
    class OpenGLPaintContext;
    struct SvgIcon;

    class System
    {
        public:
            System();
            ~System();

            void publish(std::shared_ptr<const SkinGeneration> generation,
                         const std::function<void()>& commit_documents = {});
            ViewBuildResult createView(const std::string& resource_id) const;
            std::unique_ptr<Surface> createSurface(const TextMetrics& text_metrics) const;
            bool setLongClickDelay(std::chrono::milliseconds delay);
            bool setLocale(const std::string& id);
            void setLocaleChangedHandler(std::function<void(const std::string&)> handler)
            {
                mLocaleChangedHandler = std::move(handler);
            }
            void setKeybindingResolver(
                std::function<KeybindingPresentation(const std::string&)> resolver);
            void refreshKeybindings();

            const std::vector<LanguageInfo>& languages() const;
            const std::string& activeLocale() const { return mActiveLocale; }
            const std::string& defaultLocale() const;
            const LanguageInfo* activeLanguage() const;
            LayoutDirection layoutDirection() const
            {
                const LanguageInfo* language = activeLanguage();
                return language ? language->direction : LayoutDirection::LeftToRight;
            }
            bool hasLocalizationKey(const std::string& id) const;
            std::string resolveText(const std::string& id) const;
            KeybindingPresentation resolveKeybinding(const std::string& id) const;
            TextValue localized(std::string id) const;
            bool hasIcon(const std::string& name) const;
            bool sameReloadInputs(const ResourceSnapshot& left,
                                  const ResourceSnapshot& right) const;
            std::uint64_t generation() const { return mGenerationNumber; }
            std::uint64_t localeGeneration() const { return mLocaleGeneration; }
            std::chrono::milliseconds longClickDelay() const { return mLongClickDelay; }

        private:
            std::shared_ptr<const SkinGeneration> mSkinGeneration;
            std::string mActiveLocale;
            std::uint64_t mGenerationNumber = 0;
            std::uint64_t mLocaleGeneration = 0;
            std::chrono::milliseconds mLongClickDelay{500};
            mutable std::unordered_set<Surface*> mSurfaces;
            std::function<void(const std::string&)> mLocaleChangedHandler;
            std::function<KeybindingPresentation(const std::string&)> mKeybindingResolver;

            const SvgIcon* icon(const std::string& name) const;
            const StyleSheet& styleSheet() const;
            void registerSurface(Surface& surface) const;
            void unregisterSurface(Surface& surface) const;
            void notifyLocaleChanged();
            friend class Surface;
            friend class OpenGLPaintContext;
    };
}

#endif // LL_RDUI_SYSTEM_H
