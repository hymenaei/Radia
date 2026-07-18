#include "linden_common.h"
#include "../test/lltut.h"
#include "rduilocalization.h"
#include <algorithm>

namespace tut
{
    struct rduilocalization_data {};
    typedef test_group<rduilocalization_data> rduilocalization_test;
    typedef rduilocalization_test::object rduilocalization_object;
    rduilocalization_test rduilocalization_testcase("rduilocalization");

    template<> template<>
    void rduilocalization_object::test<1>()
    {
        rdui::LocalizationCatalog catalog;
        const auto result = catalog.loadXml(
            "<localizations default=\"en\">"
            "<localization id=\"en\" lang=\"English\" direction=\"ltr\"><string id=\"title\">Title</string><string id=\"fallback\">Fallback</string></localization>"
            "<localization id=\"pt\" lang=\"Português\" direction=\"ltr\"><string id=\"title\">Título</string></localization>"
            "<localization id=\"elvish\" lang=\"Quenya\" direction=\"rtl\"><string id=\"title\">Elda</string></localization>"
            "</localizations>", "localization.xml");

        ensure("valid localization loads", result.ok());
        ensure_equals("language order preserved", catalog.languages().size(), 3U);
        ensure_equals("arbitrary language id preserved", catalog.languages()[2].id, "elvish");
        ensure_equals("native UTF-8 name preserved", catalog.languages()[1].name, "Português");
        ensure_equals("ltr direction retained", static_cast<int>(catalog.languages()[0].direction),
                      static_cast<int>(rdui::LayoutDirection::LeftToRight));
        ensure_equals("rtl direction retained", static_cast<int>(catalog.languages()[2].direction),
                      static_cast<int>(rdui::LayoutDirection::RightToLeft));
        ensure_equals("declared default retained", catalog.defaultLanguageId(), "en");
        ensure_equals("explicit locale translation resolved", catalog.get("pt", "title"), "Título");
        ensure_equals("missing locale string falls back", catalog.get("pt", "fallback"), "Fallback");
        ensure_equals("globally missing string shows id", catalog.get("pt", "missing.id"), "missing.id");
        ensure("known language query succeeds", catalog.containsLanguage("pt"));
        ensure("unknown language query fails", !catalog.containsLanguage("unknown"));
    }

    template<> template<>
    void rduilocalization_object::test<2>()
    {
        rdui::LocalizationCatalog catalog;
        ensure("initial catalog loads", catalog.loadXml(
            "<localizations default=\"en\"><localization id=\"en\" lang=\"English\" direction=\"ltr\"><string id=\"value\">live</string></localization></localizations>").ok());

        const auto duplicate = catalog.loadXml(
            "<localizations default=\"custom\">"
            "<localization id=\"custom\" lang=\"First\" direction=\"ltr\"><string id=\"value\">first</string><string id=\"value\">second</string></localization>"
            "<localization id=\"custom\" lang=\"Last\" direction=\"ltr\"/>"
            "</localizations>");
        ensure("duplicate entries reject catalog", !duplicate.ok());
        ensure_equals("failed candidate preserves live catalog", catalog.get("en", "value"), "live");
    }

    template<> template<>
    void rduilocalization_object::test<3>()
    {
        rdui::LocalizationCatalog catalog;
        const auto malformed = catalog.loadXml("<localizations>", "broken.xml");
        ensure("malformed XML rejected", !malformed.ok());
        ensure_equals("diagnostic has stable code", malformed.errors.front().code, "localization.xml.invalid");
        ensure_equals("diagnostic identifies source", malformed.errors.front().source, "broken.xml");
        ensure("wrong root rejected", !catalog.loadXml("<strings/>").ok());
        ensure("missing default rejected", !catalog.loadXml(
            "<localizations><localization id=\"en\" lang=\"English\" direction=\"ltr\"/></localizations>").ok());
        ensure("empty catalog rejected", !catalog.loadXml("<localizations default=\"en\"/>").ok());
    }

    template<> template<>
    void rduilocalization_object::test<4>()
    {
        rdui::LocalizationCatalog catalog;
        const auto missing_default_key = catalog.loadXml(
            "<localizations default=\"en\">"
            "<localization id=\"en\" lang=\"English\" direction=\"ltr\"/>"
            "<localization id=\"pt\" lang=\"Português\" direction=\"ltr\"><string id=\"only.pt\">Só</string></localization>"
            "</localizations>", "localization.xml");
        ensure("translation absent from default language rejects catalog", !missing_default_key.ok());
        ensure_equals("missing default key diagnostic is stable",
                      missing_default_key.errors.back().code, "localization.default.string_missing");

        ensure("unknown attributes reject catalog", !catalog.loadXml(
            "<localizations default=\"en\" invented=\"true\"><localization id=\"en\" lang=\"English\" direction=\"ltr\"/></localizations>").ok());
        const auto missing_direction = catalog.loadXml(
            "<localizations default=\"en\"><localization id=\"en\" lang=\"English\"/></localizations>");
        ensure("missing direction rejects catalog", !missing_direction.ok());
        ensure("direction diagnostic is stable", std::any_of(
            missing_direction.errors.begin(), missing_direction.errors.end(),
            [](const rdui::Diagnostic& diagnostic)
            {
                return diagnostic.code == "localization.language.direction_invalid";
            }));
        ensure("direction values are case-sensitive", !catalog.loadXml(
            "<localizations default=\"en\"><localization id=\"en\" lang=\"English\" direction=\"RTL\"/></localizations>").ok());
    }

    template<> template<>
    void rduilocalization_object::test<5>()
    {
        rdui::LocalizationCatalog catalog;
        const auto result = catalog.loadXmlLayers({
            {"base/localization.xml",
             "<localizations default=\"en\">"
             "<localization id=\"en\" lang=\"English\" direction=\"ltr\"><string id=\"title\">Base</string></localization>"
             "</localizations>"},
            {"derived/localization.xml",
             "<localizations default=\"en\">"
             "<localization id=\"en\" lang=\"English\" direction=\"ltr\"><string id=\"title\">Derived</string></localization>"
             "</localizations>"},
            {"translation/localization.xml",
             "<localizations default=\"en\">"
             "<localization id=\"ar\" lang=\"العربية\" direction=\"rtl\"><string id=\"title\">مشتق</string></localization>"
             "</localizations>"},
        });

        ensure("localization layers merge", result.ok());
        ensure_equals("derived default string replaces base", catalog.get("en", "title"), "Derived");
        ensure_equals("derived language can rely on merged default key", catalog.get("ar", "title"), "مشتق");
    }
}
