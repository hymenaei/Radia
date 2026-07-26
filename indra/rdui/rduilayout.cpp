#include "linden_common.h"
#include "rduilayout.h"
#include "rduiwidget.h"
#include "rduistylesheet.h"
#include "rduitextmetrics.h"
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
            Vec2 automatic_minimum;
        };

        float& mainSize(ChildLayout& child, Flow flow)
        {
            return flow == Flow::Row ? child.measured.x : child.measured.y;
        }

        float mainSize(const ChildLayout& child, Flow flow)
        {
            return flow == Flow::Row ? child.measured.x : child.measured.y;
        }

        float mainMinimum(const ChildLayout& child, Flow flow, float available_main, float flex_base)
        {
            const std::optional<Length>& minimum = flow == Flow::Row ? child.style.min_width : child.style.min_height;
            if (minimum) return minimum->resolve(available_main);
            const float automatic = flow == Flow::Row ? child.automatic_minimum.x : child.automatic_minimum.y;
            return std::min(automatic, flex_base);
        }

        void applyFlexBasis(ChildLayout& child, Flow flow, float available_main)
        {
            if (child.style.flex_basis.isAuto()) return;
            const float basis = child.style.flex_basis.resolve(0.f, available_main);
            mainSize(child, flow) = std::max(basis, mainMinimum(child, flow, available_main, basis));
        }

        void distributeFlexSpace(std::vector<ChildLayout>& children,
                                 std::size_t begin,
                                 std::size_t end,
                                 Flow flow,
                                 float available_main,
                                 bool allow_growth,
                                 float& total)
        {
            const float free_space = available_main - total;
            if (free_space > 0.f && allow_growth)
            {
                float total_grow = 0.f;
                for (std::size_t index = begin; index < end; ++index)
                    total_grow += children[index].style.flex_grow;
                if (total_grow <= 0.f) return;
                for (std::size_t index = begin; index < end; ++index)
                    mainSize(children[index], flow) += free_space * children[index].style.flex_grow / total_grow;
                total += free_space;
                return;
            }
            if (free_space >= 0.f) return;

            float deficit = -free_space;
            std::vector<float> base_sizes;
            std::vector<bool> active;
            base_sizes.reserve(end - begin);
            active.reserve(end - begin);
            for (std::size_t index = begin; index < end; ++index)
            {
                const ChildLayout& child = children[index];
                const float base = mainSize(child, flow);
                base_sizes.push_back(base);
                active.push_back(child.style.flex_shrink > 0.f
                              && base > mainMinimum(child, flow, available_main, base));
            }

            constexpr float epsilon = 1.0e-4f;
            while (deficit > epsilon)
            {
                float total_weight = 0.f;
                for (std::size_t offset = 0; offset < active.size(); ++offset)
                    if (active[offset])
                        total_weight += children[begin + offset].style.flex_shrink * base_sizes[offset];
                if (total_weight <= epsilon) break;

                const float round_deficit = deficit;
                float reduced = 0.f;
                for (std::size_t offset = 0; offset < active.size(); ++offset)
                {
                    if (!active[offset]) continue;
                    ChildLayout& child = children[begin + offset];
                    float& size = mainSize(child, flow);
                    const float minimum = mainMinimum(child, flow, available_main, base_sizes[offset]);
                    const float share = round_deficit * child.style.flex_shrink * base_sizes[offset] / total_weight;
                    const float reduction = std::min(share, size - minimum);
                    size -= reduction;
                    reduced += reduction;
                    if (size <= minimum + epsilon) active[offset] = false;
                }
                if (reduced <= epsilon) break;
                deficit -= reduced;
                total -= reduced;
            }
        }

        enum class CrossAlignment { Start, Center, End, Stretch };

        float verticalAlignmentOffset(VerticalAlign alignment, float free_space)
        {
            if (alignment == VerticalAlign::Middle) return free_space * .5f;
            if (alignment == VerticalAlign::Bottom) return free_space;
            return 0.f;
        }

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
            if (flow == Flow::Column) return CrossAlignment::Stretch;
            if (parent.vertical_align == VerticalAlign::Middle) return CrossAlignment::Center;
            if (parent.vertical_align == VerticalAlign::Bottom) return CrossAlignment::End;
            return CrossAlignment::Start;
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
                const Vec2 intrinsic = node.intrinsicSize(theme, style, text_metrics);
                Vec2 content = flow == Flow::Row ? Vec2{} : intrinsic;
                float row_width = intrinsic.x;
                float row_height = intrinsic.y;
                std::size_t row_children = 0;
                std::size_t row_lines = 0;
                Widget* previous_child = nullptr;
                const float fixed_gap = style.gap.fixedPixels();
                const auto finishRow = [&]
                {
                    if (!row_children && intrinsic.x == 0.f && intrinsic.y == 0.f) return;
                    content.x = std::max(content.x, row_width);
                    if (row_lines) content.y += fixed_gap;
                    content.y += row_height;
                    ++row_lines;
                    row_width = 0.f;
                    row_height = 0.f;
                    row_children = 0;
                    previous_child = nullptr;
                };

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
                    if ((flow == Flow::Row || flow == Flow::Column) && !child_style.flex_basis.isAuto())
                    {
                        const Dimension& parent_dimension = flow == Flow::Row ? style.width : style.height;
                        const float rect_size = flow == Flow::Row ? node.mRect.w : node.mRect.h;
                        const float padding = flow == Flow::Row ? style.padding.horizontal() : style.padding.vertical();
                        const bool definite_parent = !parent_dimension.isAuto() || node.mRectExplicit;
                        const bool percentage_is_auto = child_style.flex_basis.isPercentage() && !definite_parent;
                        if (!percentage_is_auto)
                        {
                            const float parent_size = parent_dimension.isAuto()
                                ? rect_size : parent_dimension.resolve(0.f, rect_size);
                            const float reference = std::max(0.f, parent_size - padding);
                            const float basis = child_style.flex_basis.resolve(0.f, reference);
                            const std::optional<Length>& authored_minimum = flow == Flow::Row
                                ? child_style.min_width : child_style.min_height;
                            const float automatic_minimum = flow == Flow::Row ? child_size.x : child_size.y;
                            const float minimum = authored_minimum
                                ? authored_minimum->resolve(reference) : std::min(automatic_minimum, basis);
                            if (flow == Flow::Row) child_size.x = std::max(basis, minimum);
                            else child_size.y = std::max(basis, minimum);
                        }
                    }
                    const float outer_width = child_size.x + child_style.margin.horizontal();
                    const float outer_height = child_size.y + child_style.margin.vertical();

                    if (flow == Flow::Row)
                    {
                        if (child.flowBreakBefore() && row_children) finishRow();
                        if (previous_child && node.hasLayoutGapBetween(*previous_child, child))
                            row_width += fixed_gap;
                        if (previous_child)
                            row_width -= node.layoutOverlapBetween(*previous_child, child, style);
                        row_width += outer_width;
                        row_height = std::max(row_height, outer_height);
                        ++row_children;
                    }
                    else if (flow == Flow::Column)
                    {
                        if (previous_child && node.hasLayoutGapBetween(*previous_child, child))
                            content.y += fixed_gap;
                        if (previous_child)
                            content.y -= node.layoutOverlapBetween(*previous_child, child, style);
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
                    if (flow == Flow::Row || flow == Flow::Column) previous_child = &child;
                }

                if (flow == Flow::Row)
                {
                    if (row_children || !row_lines) finishRow();
                }

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
                    const Vec2 measured = measure(*child, theme, text_metrics);
                    result.push_back({child.get(), std::move(style), measured, measured});
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

            static Rect positionedRect(const ChildLayout& child, const Rect& parent,
                                       VerticalAlign vertical_alignment)
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

                const float vertical_space = std::max(0.f, parent.h - height - margin.vertical());
                float y = explicit_rect ? child.node->mRect.y
                        : parent.top() - margin.top.fixedPixels() - height
                          - verticalAlignmentOffset(vertical_alignment, vertical_space);
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
                        applyFlexBasis(child, flow, available_main);
                    }

                    std::vector<std::pair<std::size_t, std::size_t>> lines;
                    std::size_t line_start = 0;
                    for (std::size_t index = 0; index < children.size(); ++index)
                    {
                        if (index > line_start && children[index].node->flowBreakBefore())
                        {
                            lines.emplace_back(line_start, index);
                            line_start = index;
                        }
                    }
                    if (line_start < children.size()) lines.emplace_back(line_start, children.size());

                    const float line_gap = parent_style.gap.fixedPixels();
                    std::vector<float> line_heights;
                    line_heights.reserve(lines.size());
                    for (const auto& [begin, end] : lines)
                    {
                        float height = 0.f;
                        for (std::size_t index = begin; index < end; ++index)
                            height = std::max(height, children[index].measured.y + children[index].style.margin.vertical());
                        line_heights.push_back(
                            lines.size() == 1 && available_cross >= 0.f ? available_cross : height);
                    }
                    for (std::size_t line = 0; line < lines.size(); ++line)
                    {
                        const auto [begin, end] = lines[line];
                        for (std::size_t index = begin; index < end; ++index)
                            applyCrossAxisSizing(children[index].measured, children[index].style, flow,
                                                 line_heights[line],
                                                 crossAlignment(parent_style, children[index].style, flow));
                    }

                    float block_height = 0.f;
                    for (float height : line_heights) block_height += height;
                    if (line_heights.size() > 1)
                        block_height += line_gap * static_cast<float>(line_heights.size() - 1);
                    float line_top = content.top() - verticalAlignmentOffset(
                        parent_style.vertical_align, std::max(0.f, available_cross - block_height));

                    for (std::size_t line = 0; line < lines.size(); ++line)
                    {
                        const auto [begin, end] = lines[line];
                        const float line_height = line_heights[line];
                        const float line_bottom = line_top - line_height;
                        float total = 0.f;
                        int auto_margins = 0;
                        for (std::size_t index = begin; index < end; ++index)
                        {
                            const ChildLayout& child = children[index];
                            total += child.measured.x + child.style.margin.horizontal();
                            auto_margins += child.style.margin.horizontalAutoCount();
                        }
                        std::size_t gap_count = 0;
                        for (std::size_t index = begin + 1; index < end; ++index)
                            if (node.hasLayoutGapBetween(*children[index - 1].node, *children[index].node))
                                ++gap_count;
                        float overlap = 0.f;
                        for (std::size_t index = begin + 1; index < end; ++index)
                            overlap += node.layoutOverlapBetween(*children[index - 1].node,
                                                                 *children[index].node, parent_style);
                        float gap = parent_style.gap.fixedPixels();
                        total += gap * static_cast<float>(gap_count) - overlap;
                        distributeFlexSpace(children, begin, end, flow, available_main,
                                            !auto_margins && !parent_style.gap.isAuto(), total);
                        float free_space = available_main - total;
                        if (!auto_margins && parent_style.gap.isAuto() && gap_count)
                        {
                            gap = std::max(0.f, free_space) / static_cast<float>(gap_count);
                            total += gap * static_cast<float>(gap_count);
                            free_space = available_main - total;
                        }
                        const float auto_margin = auto_margins
                            ? std::max(0.f, free_space) / static_cast<float>(auto_margins) : 0.f;
                        const bool rtl = direction == LayoutDirection::RightToLeft;
                        float x = rtl ? content.right() : content.left();
                        if (!auto_margins)
                        {
                            const float offset = rowAlignmentOffset(parent_style.justify_content, direction, free_space);
                            x += rtl ? -offset : offset;
                        }

                        for (std::size_t index = begin; index < end; ++index)
                        {
                            ChildLayout& child = children[index];
                            const MarginInsets& margin = child.style.margin;
                            const float available_cross_space = line_height - child.measured.y - margin.vertical();
                            const float cross_space = std::max(0.f, available_cross_space);
                            const int cross_auto_count = margin.verticalAutoCount();
                            const float cross_auto = cross_auto_count
                                ? cross_space / static_cast<float>(cross_auto_count) : 0.f;
                            float y = line_bottom + margin.bottom.fixedPixels();
                            if (cross_auto_count) y += margin.bottom.isAuto() ? cross_auto : 0.f;
                            else
                            {
                                const CrossAlignment alignment = crossAlignment(parent_style, child.style, flow);
                                if (alignment == CrossAlignment::Start || alignment == CrossAlignment::Stretch)
                                    y = line_top - margin.top.fixedPixels() - child.measured.y;
                                else if (alignment == CrossAlignment::Center)
                                    y += available_cross_space * .5f;
                            }
                            if (rtl)
                            {
                                x -= margin.right.fixedPixels() + (margin.right.isAuto() ? auto_margin : 0.f);
                                setArrangedRect(*child.node, {x - child.measured.x, y,
                                                             child.measured.x, child.measured.y});
                                x -= child.measured.x + margin.left.fixedPixels()
                                   + (margin.left.isAuto() ? auto_margin : 0.f);
                                if (index + 1 < end)
                                {
                                    if (node.hasLayoutGapBetween(*child.node, *children[index + 1].node))
                                        x -= gap;
                                    x += node.layoutOverlapBetween(*child.node, *children[index + 1].node,
                                                                   parent_style);
                                }
                            }
                            else
                            {
                                x += margin.left.fixedPixels() + (margin.left.isAuto() ? auto_margin : 0.f);
                                setArrangedRect(*child.node, {x, y, child.measured.x, child.measured.y});
                                x += child.measured.x + margin.right.fixedPixels()
                                   + (margin.right.isAuto() ? auto_margin : 0.f);
                                if (index + 1 < end)
                                {
                                    if (node.hasLayoutGapBetween(*child.node, *children[index + 1].node))
                                        x += gap;
                                    x -= node.layoutOverlapBetween(*child.node, *children[index + 1].node,
                                                                   parent_style);
                                }
                            }
                            arrangeNode(*child.node, theme, text_metrics, direction);
                        }
                        line_top = line_bottom - line_gap;
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
                        applyFlexBasis(child, flow, available_main);
                        applyCrossAxisSizing(child.measured, child.style, flow, available_cross,
                                             crossAlignment(parent_style, child.style, flow));
                    }
                    float total = 0.f;
                    int auto_margins = 0;
                    for (const ChildLayout& child : children)
                    {
                        total += child.measured.y + child.style.margin.vertical();
                        auto_margins += child.style.margin.verticalAutoCount();
                    }
                    std::size_t gap_count = 0;
                    for (std::size_t index = 1; index < children.size(); ++index)
                        if (node.hasLayoutGapBetween(*children[index - 1].node, *children[index].node))
                            ++gap_count;
                    float overlap = 0.f;
                    for (std::size_t index = 1; index < children.size(); ++index)
                        overlap += node.layoutOverlapBetween(*children[index - 1].node,
                                                             *children[index].node, parent_style);
                    float gap = parent_style.gap.fixedPixels();
                    total += gap * static_cast<float>(gap_count) - overlap;
                    distributeFlexSpace(children, 0, children.size(), flow, available_main,
                                        !auto_margins && !parent_style.gap.isAuto(), total);
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
                        else if (alignment == JustifyContent::Start)
                            y -= verticalAlignmentOffset(parent_style.vertical_align, free_space);
                    }
                    for (std::size_t i = 0; i < children.size(); ++i)
                    {
                        ChildLayout& child = children[i];
                        const MarginInsets& margin = child.style.margin;
                        if (i)
                        {
                            if (node.hasLayoutGapBetween(*children[i - 1].node, *child.node)) y -= gap;
                            y += node.layoutOverlapBetween(*children[i - 1].node, *child.node,
                                                           parent_style);
                        }
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
                        setArrangedRect(*child.node, positionedRect(child, content, parent_style.vertical_align));
                        arrangeNode(*child.node, theme, text_metrics, direction);
                    }
                }

                node.onArranged(parent_style);
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
