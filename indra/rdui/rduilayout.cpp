#include "linden_common.h"
#include "rduilayout.h"
#include "rduiwidget.h"
#include "rduistylesheet.h"
#include "rduitext.h"
#include <algorithm>
#include <vector>

namespace rdui
{
    Style resolveWidgetStyle(const StyleSheet& theme, const Widget& node)
    {
        const Widget* owner = &node;
        Style style;
        if (node.part().empty()) style = theme.resolveWidget(node);
        else
        {
            for (const Widget* candidate = node.parent(); candidate; candidate = candidate->parent())
            {
                if (candidate->styleElement() != node.styleElement()) continue;
                owner = candidate;
                if (candidate->part().empty()) break;
            }
            style = theme.resolveWidgetPart(*owner, node);
        }
        node.constrainResolvedStyle(style);
        if (const Widget* parent = node.parent()) inheritStyle(style, resolveWidgetStyle(theme, *parent));
        return style;
    }

    namespace
    {
        float styledDimension(const Dimension& value, const std::optional<Length>& minimum, float fallback, float reference = 0.f)
        {
            const float resolved = value.resolve(fallback, reference);
            return minimum ? std::max(resolved, minimum->resolve(reference)) : resolved;
        }

        void warnIgnoredPosition(const Widget& child, const Style& style, Flow flow)
        {
            if (flow == Flow::Free) return;
            const char* property = style.left ? "left"
                                 : style.right ? "right"
                                 : style.top ? "top"
                                 : style.bottom ? "bottom"
                                 : nullptr;
            if (!property) return;
            LL_WARNS("rdui") << "Ignoring '" << property << "' on <" << child.element()
                             << (child.id().empty() ? "" : " id=\"" + child.id() + "\"")
                             << ">: parent flow is '" << (flow == Flow::Row ? "row" : "column") << "'." << LL_ENDL;
        }

        struct ChildLayout
        {
            Widget* node;
            Style style;
            Vec2 measured;
        };

        enum class CrossAlignment { Start, Center, End, Stretch };

        CrossAlignment crossAlignment(const Style& parent, const Style& child, Flow flow)
        {
            if (child.align_self != AlignSelf::Auto)
            {
                if (child.align_self == AlignSelf::Start) return CrossAlignment::Start;
                if (child.align_self == AlignSelf::Center) return CrossAlignment::Center;
                if (child.align_self == AlignSelf::End) return CrossAlignment::End;
                return CrossAlignment::Stretch;
            }
            if (parent.align_items == AlignItems::Start) return CrossAlignment::Start;
            if (parent.align_items == AlignItems::Center) return CrossAlignment::Center;
            if (parent.align_items == AlignItems::End) return CrossAlignment::End;
            if (parent.align_items == AlignItems::Stretch) return CrossAlignment::Stretch;
            return flow == Flow::Column ? CrossAlignment::Stretch : CrossAlignment::Center;
        }

        void applyCrossAxisSizing(Vec2& size, const Style& style, Flow flow, float available_cross,
                                  CrossAlignment alignment)
        {
            if (alignment != CrossAlignment::Stretch) return;
            if (flow == Flow::Row)
            {
                if (!style.height.isAuto() || style.margin.verticalAutoCount()) return;
                const float height = std::max(0.f, available_cross - style.margin.vertical());
                size.y = styledDimension(style.height, style.min_height, height, available_cross);
                if (style.aspect_ratio && style.width.isAuto() && *style.aspect_ratio > 0.f)
                    size.x = size.y * *style.aspect_ratio;
            }
            else if (flow == Flow::Column)
            {
                if (!style.width.isAuto() || style.margin.horizontalAutoCount()) return;
                const float width = std::max(0.f, available_cross - style.margin.horizontal());
                size.x = styledDimension(style.width, style.min_width, width, available_cross);
                if (style.aspect_ratio && style.height.isAuto() && *style.aspect_ratio > 0.f)
                    size.y = size.x / *style.aspect_ratio;
            }
        }

        float rowAlignmentOffset(JustifyContent alignment, LayoutDirection direction, float free_space)
        {
            if (alignment == JustifyContent::Center) return free_space * .5f;
            if (alignment == JustifyContent::End) return free_space;
            if (alignment == JustifyContent::Left) return direction == LayoutDirection::RightToLeft ? free_space : 0.f;
            if (alignment == JustifyContent::Right) return direction == LayoutDirection::RightToLeft ? 0.f : free_space;
            return 0.f;
        }
    }

    class LayoutEngine
    {
        public:
            static Vec2 measure(Widget& node, const StyleSheet& theme, const TextMetrics& text_metrics)
            {
                if (node.mVisibility == Visibility::Collapsed)
                {
                    node.mDesiredSize = {};
                    node.mMeasureDirty = false;
                    return node.mDesiredSize;
                }
                if (!node.mMeasureDirty) return node.mDesiredSize;

                const Style style = resolveWidgetStyle(theme, node);
                const Flow flow = style.flow;
                Vec2 content = node.intrinsicSize(theme, style, text_metrics);
                std::size_t participating_count = 0;

                for (const auto& child_ptr : node.mChildren)
                {
                    Widget& child = *child_ptr;
                    if (child.visibility() == Visibility::Collapsed) continue;
                    const Style child_style = resolveWidgetStyle(theme, child);
                    Vec2 child_size = measure(child, theme, text_metrics);
                    if ((flow == Flow::Row || flow == Flow::Column) && child_style.aspect_ratio)
                    {
                        std::optional<float> cross_size;
                        if (flow == Flow::Row)
                        {
                            if (!style.height.isAuto()) cross_size = style.height.resolve(0.f, node.mRect.h);
                            else if (node.mRectExplicit) cross_size = node.mRect.h;
                        }
                        else
                        {
                            if (!style.width.isAuto()) cross_size = style.width.resolve(0.f, node.mRect.w);
                            else if (node.mRectExplicit) cross_size = node.mRect.w;
                        }
                        if (cross_size)
                        {
                            const float padding = flow == Flow::Row ? style.padding.vertical() : style.padding.horizontal();
                            applyCrossAxisSizing(child_size, child_style, flow, std::max(0.f, *cross_size - padding),
                                                 crossAlignment(style, child_style, flow));
                        }
                    }
                    const float outer_width = child_size.x + child_style.margin.horizontal();
                    const float outer_height = child_size.y + child_style.margin.vertical();

                    if (flow == Flow::Row)
                    {
                        content.x += outer_width;
                        content.y = std::max(content.y, outer_height);
                    }
                    else if (flow == Flow::Column)
                    {
                        content.x = std::max(content.x, outer_width);
                        content.y += outer_height;
                    }
                    else
                    {
                        const float horizontal_offset = child_style.left ? child_style.left->resolve(0.f) : child_style.margin.left.fixedPixels();
                        const float vertical_offset = child_style.top ? child_style.top->resolve(0.f) : child_style.margin.top.fixedPixels();
                        content.x = std::max(content.x, horizontal_offset + outer_width);
                        content.y = std::max(content.y, vertical_offset + outer_height);
                    }
                    ++participating_count;
                }

                const float fixed_gap = style.gap.fixedPixels();
                if (participating_count > 1 && flow == Flow::Row) content.x += fixed_gap * static_cast<float>(participating_count - 1);
                else if (participating_count > 1 && flow == Flow::Column) content.y += fixed_gap * static_cast<float>(participating_count - 1);

                const Vec2 natural(content.x + style.padding.horizontal(), content.y + style.padding.vertical());
                node.mDesiredSize = {
                    styledDimension(style.width, style.min_width, natural.x),
                    styledDimension(style.height, style.min_height, natural.y)
                };
                node.mMeasureDirty = false;
                return node.mDesiredSize;
            }

            static void arrange(Widget& node, const StyleSheet& theme, const TextMetrics& text_metrics, LayoutDirection direction)
            {
                measure(node, theme, text_metrics);
                if (node.mVisibility == Visibility::Collapsed)
                {
                    node.mArrangeDirty = false;
                    return;
                }
                arrangeNode(node, theme, text_metrics, direction);
            }

        private:
            static std::vector<ChildLayout> flowChildren(Widget& parent, const StyleSheet& theme, const TextMetrics& text_metrics, Flow flow)
            {
                std::vector<ChildLayout> result;
                result.reserve(parent.mChildren.size());
                for (auto& child : parent.mChildren)
                {
                    if (child->visibility() == Visibility::Collapsed) continue;
                    Style style = resolveWidgetStyle(theme, *child);
                    warnIgnoredPosition(*child, style, flow);
                    result.push_back({child.get(), std::move(style), measure(*child, theme, text_metrics)});
                }
                if (flow == Flow::Row || flow == Flow::Column)
                {
                    std::stable_sort(result.begin(), result.end(), [](const ChildLayout& left, const ChildLayout& right)
                    {
                        return left.style.order < right.style.order;
                    });
                }
                return result;
            }

            static Rect positionedRect(const ChildLayout& child, const Rect& parent)
            {
                const bool explicit_rect = child.node->mRectExplicit;
                const float width = explicit_rect && child.style.width.isAuto()
                                  ? child.style.min_width ? std::max(child.node->mRect.w, child.style.min_width->resolve(parent.w))
                                                          : child.node->mRect.w
                                  : styledDimension(child.style.width, child.style.min_width,
                                                    child.measured.x > 0.f ? child.measured.x : parent.w, parent.w);
                const float height = explicit_rect && child.style.height.isAuto()
                                   ? child.style.min_height ? std::max(child.node->mRect.h, child.style.min_height->resolve(parent.h))
                                                            : child.node->mRect.h
                                   : styledDimension(child.style.height, child.style.min_height, child.measured.y, parent.h);
                const MarginInsets& margin = child.style.margin;
                const float horizontal_space = std::max(0.f, parent.w - width - margin.horizontal());
                float x = explicit_rect ? child.node->mRect.x
                        : margin.left.isAuto() ? parent.left() + horizontal_space
                        : parent.left() + margin.left.fixedPixels();
                if (margin.left.isAuto() && margin.right.isAuto()) x = parent.left() + horizontal_space * .5f;

                float y = explicit_rect ? child.node->mRect.y : parent.top() - margin.top.fixedPixels() - height;
                if (child.style.left) x = parent.left() + child.style.left->resolve(parent.w) + margin.left.fixedPixels();
                else if (child.style.right) x = parent.right() - child.style.right->resolve(parent.w) - margin.right.fixedPixels() - width;
                if (child.style.top) y = parent.top() - child.style.top->resolve(parent.h) - margin.top.fixedPixels() - height;
                else if (child.style.bottom) y = parent.bottom() + child.style.bottom->resolve(parent.h) + margin.bottom.fixedPixels();
                return {x, y, width, height};
            }

            static void setArrangedRect(Widget& node, const Rect& rect)
            {
                node.mRect = rect;
                node.mArrangeDirty = false;
                node.invalidatePaint();
            }

            static void arrangeNode(Widget& node, const StyleSheet& theme, const TextMetrics& text_metrics, LayoutDirection direction)
            {
                const Style parent_style = resolveWidgetStyle(theme, node);
                const Rect panel = node.mRect;
                const Rect content(panel.x + parent_style.padding.left,
                                   panel.y + parent_style.padding.bottom,
                                   std::max(0.f, panel.w - parent_style.padding.horizontal()),
                                   std::max(0.f, panel.h - parent_style.padding.vertical()));
                const Flow flow = parent_style.flow;
                std::vector<ChildLayout> children = flowChildren(node, theme, text_metrics, flow);

                if (flow == Flow::Row)
                {
                    const float available_main = panel.w - parent_style.padding.horizontal();
                    const float available_cross = panel.h - parent_style.padding.vertical();
                    for (ChildLayout& child : children)
                    {
                        child.measured.x = styledDimension(child.style.width, child.style.min_width, child.measured.x, available_main);
                        child.measured.y = styledDimension(child.style.height, child.style.min_height, child.measured.y, available_cross);
                        applyCrossAxisSizing(child.measured, child.style, flow, available_cross,
                                             crossAlignment(parent_style, child.style, flow));
                    }
                    float total = 0.f;
                    float total_grow = 0.f;
                    int auto_margins = 0;
                    for (const ChildLayout& child : children)
                    {
                        total += child.measured.x + child.style.margin.horizontal();
                        total_grow += child.style.grow;
                        auto_margins += child.style.margin.horizontalAutoCount();
                    }
                    const std::size_t gap_count = children.empty() ? 0 : children.size() - 1;
                    float gap = parent_style.gap.fixedPixels();
                    total += gap * static_cast<float>(gap_count);
                    const float available_growth = available_main - total;
                    if (!auto_margins && !parent_style.gap.isAuto() && available_growth > 0.f && total_grow > 0.f)
                    {
                        for (ChildLayout& child : children) child.measured.x += available_growth * child.style.grow / total_grow;
                        total += available_growth;
                    }
                    float free_space = available_main - total;
                    if (!auto_margins && parent_style.gap.isAuto() && gap_count)
                    {
                        gap = std::max(0.f, free_space) / static_cast<float>(gap_count);
                        total += gap * static_cast<float>(gap_count);
                        free_space = available_main - total;
                    }
                    const float auto_margin = auto_margins ? std::max(0.f, free_space) / static_cast<float>(auto_margins) : 0.f;
                    const bool rtl = direction == LayoutDirection::RightToLeft;
                    float x = rtl ? content.right() : content.left();
                    if (!auto_margins)
                    {
                        const float offset = rowAlignmentOffset(parent_style.justify_content, direction, free_space);
                        x += rtl ? -offset : offset;
                    }
                    for (ChildLayout& child : children)
                    {
                        const MarginInsets& margin = child.style.margin;
                        const float available_cross_space = available_cross - child.measured.y - margin.vertical();
                        const float cross_space = std::max(0.f, available_cross_space);
                        const int cross_auto_count = margin.verticalAutoCount();
                        const float cross_auto = cross_auto_count ? cross_space / static_cast<float>(cross_auto_count) : 0.f;
                        float y = content.bottom() + margin.bottom.fixedPixels();
                        if (cross_auto_count) y += margin.bottom.isAuto() ? cross_auto : 0.f;
                        else
                        {
                            const CrossAlignment alignment = crossAlignment(parent_style, child.style, flow);
                            if (alignment == CrossAlignment::Start || alignment == CrossAlignment::Stretch)
                                y = content.top() - margin.top.fixedPixels() - child.measured.y;
                            else if (alignment == CrossAlignment::Center)
                                y += available_cross_space * .5f;
                        }
                        if (rtl)
                        {
                            x -= margin.right.fixedPixels() + (margin.right.isAuto() ? auto_margin : 0.f);
                            setArrangedRect(*child.node, {x - child.measured.x, y, child.measured.x, child.measured.y});
                            x -= child.measured.x + margin.left.fixedPixels() + (margin.left.isAuto() ? auto_margin : 0.f) + gap;
                        }
                        else
                        {
                            x += margin.left.fixedPixels() + (margin.left.isAuto() ? auto_margin : 0.f);
                            setArrangedRect(*child.node, {x, y, child.measured.x, child.measured.y});
                            x += child.measured.x + margin.right.fixedPixels() + (margin.right.isAuto() ? auto_margin : 0.f) + gap;
                        }
                        arrangeNode(*child.node, theme, text_metrics, direction);
                    }
                }
                else if (flow == Flow::Column)
                {
                    const float available_main = panel.h - parent_style.padding.vertical();
                    const float available_cross = panel.w - parent_style.padding.horizontal();
                    for (ChildLayout& child : children)
                    {
                        child.measured.x = styledDimension(child.style.width, child.style.min_width, child.measured.x, available_cross);
                        child.measured.y = styledDimension(child.style.height, child.style.min_height, child.measured.y, available_main);
                        applyCrossAxisSizing(child.measured, child.style, flow, available_cross,
                                             crossAlignment(parent_style, child.style, flow));
                    }
                    float total = 0.f;
                    float total_grow = 0.f;
                    int auto_margins = 0;
                    for (const ChildLayout& child : children)
                    {
                        total += child.measured.y + child.style.margin.vertical();
                        total_grow += child.style.grow;
                        auto_margins += child.style.margin.verticalAutoCount();
                    }
                    const std::size_t gap_count = children.empty() ? 0 : children.size() - 1;
                    float gap = parent_style.gap.fixedPixels();
                    total += gap * static_cast<float>(gap_count);
                    const float available_growth = available_main - total;
                    if (!auto_margins && !parent_style.gap.isAuto() && available_growth > 0.f && total_grow > 0.f)
                    {
                        for (ChildLayout& child : children) child.measured.y += available_growth * child.style.grow / total_grow;
                        total += available_growth;
                    }
                    float free_space = available_main - total;
                    if (!auto_margins && parent_style.gap.isAuto() && gap_count)
                    {
                        gap = std::max(0.f, free_space) / static_cast<float>(gap_count);
                        total += gap * static_cast<float>(gap_count);
                        free_space = available_main - total;
                    }
                    const float auto_margin = auto_margins ? std::max(0.f, free_space) / static_cast<float>(auto_margins) : 0.f;
                    float y = content.top();
                    if (!auto_margins)
                    {
                        const JustifyContent alignment = parent_style.justify_content;
                        if (alignment == JustifyContent::Center) y -= free_space * .5f;
                        else if (alignment == JustifyContent::End || alignment == JustifyContent::Right) y -= free_space;
                    }
                    for (std::size_t i = 0; i < children.size(); ++i)
                    {
                        ChildLayout& child = children[i];
                        const MarginInsets& margin = child.style.margin;
                        if (i) y -= gap;
                        y -= margin.top.fixedPixels() + (margin.top.isAuto() ? auto_margin : 0.f);

                        const int horizontal_auto_count = margin.horizontalAutoCount();
                        const float width = child.measured.x;
                        const float horizontal_space = std::max(0.f, content.w - width - margin.horizontal());
                        const float horizontal_auto = horizontal_auto_count ? horizontal_space / static_cast<float>(horizontal_auto_count) : 0.f;
                        float x = content.left() + margin.left.fixedPixels();
                        if (horizontal_auto_count) x += margin.left.isAuto() ? horizontal_auto : 0.f;
                        else
                        {
                            const CrossAlignment alignment = crossAlignment(parent_style, child.style, flow);
                            if (alignment == CrossAlignment::Center) x += horizontal_space * .5f;
                            else
                            {
                                const bool logical_start = alignment == CrossAlignment::Start || alignment == CrossAlignment::Stretch;
                                const bool align_right = logical_start == (direction == LayoutDirection::RightToLeft);
                                if (align_right) x = content.right() - margin.right.fixedPixels() - width;
                            }
                        }

                        y -= child.measured.y;
                        setArrangedRect(*child.node, {x, y, width, child.measured.y});
                        arrangeNode(*child.node, theme, text_metrics, direction);
                        y -= margin.bottom.fixedPixels() + (margin.bottom.isAuto() ? auto_margin : 0.f);
                    }
                }
                else
                {
                    for (ChildLayout& child : children)
                    {
                        setArrangedRect(*child.node, positionedRect(child, content));
                        arrangeNode(*child.node, theme, text_metrics, direction);
                    }
                }

                node.mArrangeDirty = false;
            }
    };

    Vec2 measureWidget(const Widget& node, const StyleSheet& theme, const TextMetrics& text_metrics)
    {
        return LayoutEngine::measure(const_cast<Widget&>(node), theme, text_metrics);
    }

    void measureTree(Widget& root, const StyleSheet& theme, const TextMetrics& text_metrics)
    {
        LayoutEngine::measure(root, theme, text_metrics);
    }

    void arrangeTree(Widget& root, const StyleSheet& theme, const TextMetrics& text_metrics, LayoutDirection direction)
    {
        LayoutEngine::arrange(root, theme, text_metrics, direction);
    }

    void layoutTree(Widget& root, const StyleSheet& theme, const TextMetrics& text_metrics, LayoutDirection direction)
    {
        measureTree(root, theme, text_metrics);
        arrangeTree(root, theme, text_metrics, direction);
    }
}
