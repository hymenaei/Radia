#include "linden_common.h"
#include "rduiskingeneration.h"
#include "rduiskingenerationinternal.h"
#include "rdicon.h"

namespace rdui
{
    SkinGeneration::SkinGeneration(std::unique_ptr<Impl> implementation) : mImpl(std::move(implementation))
    {
    }

    SkinGeneration::~SkinGeneration() = default;

    std::vector<LocaleInfo> SkinGeneration::locales() const { return mImpl->localization.locales(); }
    const std::string& SkinGeneration::defaultLocale() const { return mImpl->localization.defaultLocaleId(); }
    const LocaleInfo* SkinGeneration::locale(const std::string& id) const { return mImpl->localization.locale(id); }
    bool SkinGeneration::containsLocale(const std::string& id) const { return mImpl->localization.containsLocale(id); }
    bool SkinGeneration::hasLocalizationKey(const std::string& id) const { return mImpl->localization.containsDefaultString(id); }
    InlineContent SkinGeneration::resolveContent(const std::string& locale, const LocalizationRequest& request) const
    {
        return mImpl->localization.resolve(locale, request);
    }
    std::string SkinGeneration::resolveText(const std::string& locale, const LocalizationRequest& request) const
    {
        return mImpl->localization.get(locale, request);
    }
    const StyleSheet& SkinGeneration::styleSheet() const { return mImpl->style_sheet; }

    std::shared_ptr<const SkinGeneration> SkinGeneration::empty()
    {
        static const std::shared_ptr<const SkinGeneration> generation(
            new SkinGeneration(std::make_unique<Impl>(
                ResourceSnapshot(), LocalizationCatalog(), StyleSheet(),
                std::unordered_map<std::string, SvgIcon>(), LayoutDocumentMap())));
        return generation;
    }

    ViewBuildResult SkinGeneration::createView(const std::string& resource_id, const std::string& locale) const
    {
        const std::string selected_locale = containsLocale(locale) ? locale : defaultLocale();
        const ViewBuildContext context(mImpl->localization, selected_locale);
        ViewBuildResult result = mImpl->layout_compiler.createFromResource(resource_id, &context);
        if (result.root) validateIconReferences(*result.root, result);
        if (result.hasErrors()) result.root.reset();
        return result;
    }

    void SkinGeneration::validateIconReferences(Widget& widget, ViewBuildResult& result) const
    {
        if (const auto* icon = dynamic_cast<const Icon*>(&widget); icon && !icon->name().empty() && !this->icon(icon->name()))
        {
            result.error("view.icon.missing", "Unknown icon resource: " + icon->name() + ".");
        }
        for (const auto& child : widget.children()) validateIconReferences(*child, result);
    }

    DiagnosticResult SkinGeneration::validateWidgetDefaults(const std::string& element) const
    {
        const ViewBuildContext context(mImpl->localization, defaultLocale());
        return mImpl->layout_compiler.validateWidgetDefaults(element, &context);
    }

    const SvgIcon* SkinGeneration::icon(const std::string& name) const
    {
        const auto found = mImpl->icons.find(name);
        return found == mImpl->icons.end() ? nullptr : &found->second;
    }
}
