#ifndef LL_RDUI_LOCALIZATION_INTERNAL_H
#define LL_RDUI_LOCALIZATION_INTERNAL_H

#include "rduilocalization.h"

#include <unicode/locid.h>
#include <unicode/numfmt.h>
#include <unicode/plurrule.h>

#include <cstddef>
#include <cstdint>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <type_traits>
#include <unordered_map>
#include <utility>
#include <variant>
#include <vector>

namespace rdui::localization_detail
{
    enum class TemplateKind : std::uint8_t
    {
        Text,
        Argument,
        B,
        I,
        S,
        Kbd,
        Br,
    };

    struct TemplateNode
    {
        TemplateKind kind = TemplateKind::Text;
        std::string value;
        std::vector<TemplateNode> children;
    };

    struct StringTemplate
    {
        std::vector<TemplateNode> nodes;
        std::set<std::string> arguments;
        std::multiset<std::string> bindings;
    };

    using PluralTemplates =
        std::unordered_map<std::string, StringTemplate>;

    class StringValue
    {
        public:
            using Templates =
                std::variant<StringTemplate, PluralTemplates>;

            StringValue(
                StringTemplate value, std::string source_name,
                std::size_t source_line)
                : mTemplates(std::move(value)),
                  source(std::move(source_name)),
                  line(source_line)
            {
            }

            StringValue(
                PluralTemplates value, std::string source_name,
                std::size_t source_line)
                : mTemplates(std::move(value)),
                  source(std::move(source_name)),
                  line(source_line)
            {
            }

            bool plural() const
            {
                return std::holds_alternative<PluralTemplates>(mTemplates);
            }
            const Templates& templates() const { return mTemplates; }
            const StringTemplate* scalar() const
            {
                return std::get_if<StringTemplate>(&mTemplates);
            }
            const PluralTemplates* plurals() const
            {
                return std::get_if<PluralTemplates>(&mTemplates);
            }

            std::string source;
            std::size_t line = 0;

        private:
            Templates mTemplates;
    };

    template<typename Visitor>
    void forEachTemplate(const StringValue& value, Visitor&& visitor)
    {
        std::visit([&](const auto& templates)
            {
                using Templates = std::decay_t<decltype(templates)>;
                if constexpr (std::is_same_v<Templates, StringTemplate>)
                {
                    visitor(templates);
                }
                else
                {
                    for (const auto& [category, value] : templates)
                    {
                        (void)category;
                        visitor(value);
                    }
                }
            },
            value.templates());
    }

    using StringMap = std::unordered_map<std::string, StringValue>;

    struct ParsedLocale
    {
        std::string id;
        std::string source;
        std::size_t line = 0;
        std::optional<std::string> name;
        std::optional<LayoutDirection> direction;
        std::optional<std::string> fallback;
        bool strings_present = false;
        StringMap strings;
    };

    struct ParsedCatalog
    {
        std::optional<std::string> default_locale;
        std::vector<ParsedLocale> locales;
    };

    struct StringContract
    {
        std::set<std::string> arguments;
        std::multiset<std::string> bindings;
        bool plural = false;
        std::string required_plural_argument;
    };

    struct LocaleRecord
    {
        LocaleInfo info;
        StringMap strings;
        std::vector<std::size_t> fallback_chain;
        icu::Locale locale;
        std::unique_ptr<icu::PluralRules> plural_rules;
        std::unique_ptr<icu::NumberFormat> number_format;
    };

    std::string localeIdentity(const std::string& id);
    bool parseRichString(const std::string& source, StringTemplate& parsed,
                         LocalizationLoadResult& result,
                         const std::string& source_name, std::size_t line);
    LocalizationLoadResult parseYamlCatalog(const std::string& yaml,
                                            const std::string& source_name,
                                            bool base,
                                            ParsedCatalog& catalog);
}

namespace rdui
{
    struct LocalizationCatalog::Impl
    {
        LocalizationLoadResult load(const std::vector<ResourceLayer>& layers);
        void compile(LocalizationLoadResult& result);

        const localization_detail::LocaleRecord* locale(const std::string& id) const;
        localization_detail::LocaleRecord* locale(const std::string& id);
        const localization_detail::StringValue* find(const localization_detail::LocaleRecord& locale, const std::string& key) const;

        std::vector<localization_detail::LocaleRecord> locales;
        std::unordered_map<std::string, std::size_t> locale_indices;
        std::unordered_map<std::string, localization_detail::StringContract> contracts;
        std::string default_locale;
    };
}

#endif // LL_RDUI_LOCALIZATION_INTERNAL_H
