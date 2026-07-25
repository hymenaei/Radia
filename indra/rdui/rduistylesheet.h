#ifndef LL_RDUI_STYLE_SHEET_H
#define LL_RDUI_STYLE_SHEET_H

#include "rduidiagnostic.h"
#include "rduiresourceprovider.h"
#include "rduistyle.h"
#include <cstdint>
#include <map>
#include <memory>
#include <set>
#include <string>

namespace rdui
{
    class Widget;

    struct StyleSheetLoadResult : DiagnosticResult
    {
        bool ok() const { return !hasErrors(); }
    };

    class StyleSheet
    {
        public:
            using DependencyMap = ResourceDependencyMap;

            StyleSheet();
            ~StyleSheet();
            StyleSheet(const StyleSheet& other);
            StyleSheet& operator=(const StyleSheet& other);
            StyleSheet(StyleSheet&& other) noexcept;
            StyleSheet& operator=(StyleSheet&& other) noexcept;

            StyleSheetLoadResult loadRadia(const std::string& radia, const std::string& source_name = {});
            StyleSheetLoadResult loadRadiaLayers(const std::vector<ResourceLayer>& layers);
            std::uint64_t generation() const;
            const DependencyMap& dependencies() const;

            Style resolve(const std::string& element,
                          const std::string& id,
                          const std::set<std::string>& classes,
                          uint8_t states) const;
            Style resolvePart(const std::string& element,
                              const std::string& id,
                              const std::set<std::string>& classes,
                              uint8_t owner_states,
                              const std::string& part,
                              uint8_t part_states = 0) const;
            Style resolveWidget(const Widget& widget) const;
            Style resolveWidgetPart(const Widget& owner, const Widget& part) const;
            Style resolveInline(const Widget& owner, const std::string& element,
                                const std::vector<std::string>& inline_ancestors = {}) const;

        private:
            struct Impl;
            std::unique_ptr<Impl> mImpl;
    };
}

#endif // LL_RDUI_STYLE_SHEET_H
