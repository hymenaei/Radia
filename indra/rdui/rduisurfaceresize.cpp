#include "linden_common.h"
#include "rduisurface.h"
#include "rdfloater.h"
#include "rdpanel.h"
#include "rduifloaterresize.h"
#include "rduilayout.h"
#include <algorithm>

namespace rdui
{
    Vec2 Surface::minimumFloaterSize(const Floater& floater) const
    {
        const Vec2 original = floater.originalSize();
        const Style floater_style = resolveWidgetStyle(*mStyleSheet, floater);
        Vec2 minimum{
            floater_style.min_width ? floater_style.min_width->resolve(original.x) : 0.f,
            floater_style.min_height ? floater_style.min_height->resolve(original.y) : 0.f};

        if (const Panel* header = floater.header())
        {
            const Vec2 measured = measureWidget(*header, *mStyleSheet, mTextMetrics);
            const Style header_style = resolveWidgetStyle(*mStyleSheet, *header);
            minimum.x = std::max(minimum.x, measured.x + header_style.margin.horizontal()
                                           + floater_style.padding.horizontal());
            minimum.y = std::max(minimum.y, measured.y + header_style.margin.vertical()
                                           + floater_style.padding.vertical());
        }
        return minimum;
    }

    Floater* Surface::resizeFloaterAt(const Vec2& point, std::uint8_t& edges) const
    {
        edges = 0;
        if (!mViewport.contains(point)) return nullptr;
        const auto find_in_layer = [&](SurfaceLayer layer) -> Floater*
        {
            const auto& children = layerRoot(layer).children();
            for (auto child = children.rbegin(); child != children.rend(); ++child)
            {
                auto* floater = dynamic_cast<Floater*>(child->get());
                if (!floater || floater->visibility() != Visibility::Visible || floater->closed()
                    || !floater->canResize() || floater->minimized()) continue;
                const detail::ResizeEdges hit = detail::resizeEdgesAt(floater->rect(), point);
                if (hit == detail::ResizeEdges::None) continue;
                edges = static_cast<std::uint8_t>(hit);
                return floater;
            }
            return nullptr;
        };

        if (hasActiveModal()) return find_in_layer(SurfaceLayer::Modal);
        return find_in_layer(SurfaceLayer::Floater);
    }

    void Surface::updateResizeCursor(const Vec2& point)
    {
        if (Widget* captured = mCaptured.get())
        {
            if (auto* floater = dynamic_cast<Floater*>(captured);
                floater && floater->mInteraction == Floater::FloaterInteraction::Resize) return;
        }
        std::uint8_t edges = 0;
        resizeFloaterAt(point, edges);
        mResizeCursor = detail::resizeCursor(static_cast<detail::ResizeEdges>(edges));
    }
}
