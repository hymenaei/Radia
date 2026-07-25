#include "linden_common.h"
#include "rduisystem.h"
#include "rduiskingeneration.h"
#include "rduisurface.h"

#include <set>

namespace rdui
{
    namespace
    {
        bool equalReloadInputs(const ResourceSnapshot& left, const ResourceSnapshot& right,
                               const ResourceDependencyMap& dependencies)
        {
            if (left.resources() != right.resources()) return false;
            const auto& left_resources = left.layeredResources();
            const auto& right_resources = right.layeredResources();
            if (left_resources.size() != right_resources.size()) return false;

            std::set<std::string> relevant_sources;
            for (const auto& [source, imports] : dependencies)
            {
                relevant_sources.insert(source);
                relevant_sources.insert(imports.begin(), imports.end());
            }

            for (const auto& [resource_id, left_layers] : left_resources)
            {
                const auto right_resource = right_resources.find(resource_id);
                if (right_resource == right_resources.end()) return false;
                const auto& right_layers = right_resource->second;
                if (resource_id != "skin.radia")
                {
                    if (left_layers != right_layers) return false;
                    continue;
                }
                if (left_layers.size() != right_layers.size()) return false;
                for (std::size_t index = 0; index < left_layers.size(); ++index)
                {
                    const ResourceLayer& left_layer = left_layers[index];
                    const ResourceLayer& right_layer = right_layers[index];
                    if (left_layer.source_name != right_layer.source_name
                        || left_layer.source != right_layer.source
                        || left_layer.entrypoint != right_layer.entrypoint) return false;

                    for (const auto& [module_id, source] : left_layer.modules)
                    {
                        if (!relevant_sources.contains(left_layer.sourceNameFor(module_id))) continue;
                        const auto right_module = right_layer.modules.find(module_id);
                        if (right_module == right_layer.modules.end() || right_module->second != source) return false;
                    }
                    for (const auto& [module_id, source] : right_layer.modules)
                    {
                        if (!relevant_sources.contains(right_layer.sourceNameFor(module_id))) continue;
                        const auto left_module = left_layer.modules.find(module_id);
                        if (left_module == left_layer.modules.end() || left_module->second != source) return false;
                    }
                }
            }
            return true;
        }
    }

    System::System() : mSkinGeneration(SkinGeneration::empty())
    {
    }

    System::~System() = default;

    void System::publish(std::shared_ptr<const SkinGeneration> generation,
                         const std::function<void()>& commit_documents)
    {
        if (!generation) return;

        const std::string previous_locale = mActiveLocale;
        mSkinGeneration = std::move(generation);
        mActiveLocale = mSkinGeneration->containsLocale(previous_locale)
            ? previous_locale : mSkinGeneration->defaultLocale();
        ++mGenerationNumber;
        ++mLocaleGeneration;

        for (Surface* surface : mSurfaces)
            if (surface) surface->generationChanged(styleSheet());

        if (commit_documents) commit_documents();
        notifyLocaleChanged();
    }

    const std::vector<LanguageInfo>& System::languages() const
    {
        return mSkinGeneration->languages();
    }

    const std::string& System::defaultLocale() const
    {
        return mSkinGeneration->defaultLocale();
    }

    const LanguageInfo* System::activeLanguage() const
    {
        return mSkinGeneration->language(mActiveLocale);
    }

    bool System::hasLocalizationKey(const std::string& id) const
    {
        return mSkinGeneration->hasLocalizationKey(id);
    }

    std::string System::resolveText(const std::string& id) const
    {
        return mSkinGeneration->resolveText(mActiveLocale, id);
    }

    TextValue System::localized(std::string id) const
    {
        std::string value = resolveText(id);
        return TextValue::fromLocalization(std::move(id), std::move(value));
    }

    const StyleSheet& System::styleSheet() const
    {
        return mSkinGeneration->styleSheet();
    }

    bool System::sameReloadInputs(const ResourceSnapshot& left,
                                  const ResourceSnapshot& right) const
    {
        return equalReloadInputs(left, right, styleSheet().dependencies());
    }

    const SvgIcon* System::icon(const std::string& name) const
    {
        return mSkinGeneration->icon(name);
    }

    bool System::hasIcon(const std::string& name) const
    {
        return icon(name) != nullptr;
    }

    bool System::setLocale(const std::string& id)
    {
        if (!mSkinGeneration->containsLocale(id)) return false;
        if (id == mActiveLocale) return true;
        mActiveLocale = id;
        ++mLocaleGeneration;
        notifyLocaleChanged();
        return true;
    }

    void System::setKeybindingResolver(
        std::function<KeybindingPresentation(const std::string&)> resolver)
    {
        mKeybindingResolver = std::move(resolver);
        refreshKeybindings();
    }

    KeybindingPresentation System::resolveKeybinding(const std::string& id) const
    {
        return mKeybindingResolver ? mKeybindingResolver(id) : KeybindingPresentation{};
    }

    void System::refreshKeybindings()
    {
        for (Surface* surface : mSurfaces)
            if (surface) surface->keybindingsChanged();
    }

    void System::registerSurface(Surface& surface) const
    {
        mSurfaces.insert(&surface);
    }

    void System::unregisterSurface(Surface& surface) const
    {
        mSurfaces.erase(&surface);
    }

    void System::notifyLocaleChanged()
    {
        for (Surface* surface : mSurfaces)
            if (surface) surface->localeChanged();
        if (mLocaleChangedHandler) mLocaleChangedHandler(mActiveLocale);
    }

    ViewBuildResult System::createView(const std::string& resource_id) const
    {
        return mSkinGeneration->createView(resource_id, mActiveLocale);
    }

    std::unique_ptr<Surface> System::createSurface(const TextMetrics& text_metrics) const
    {
        return std::unique_ptr<Surface>(new Surface(*this, text_metrics));
    }

    bool System::setLongClickDelay(std::chrono::milliseconds delay)
    {
        if (delay.count() <= 0) return false;
        mLongClickDelay = delay;
        return true;
    }
}
