#ifndef LL_LLFONTGPURENDERER_H
#define LL_LLFONTGPURENDERER_H

#include "llfontgl.h"
#include "llfontshaping.h"
#include "llhbgpu.h"

#if LL_HAS_HB_GPU

class LLColor4U;
class LLFontFreetype;

// Owns the complete analytic rendering transaction: shader lifetime, one
// generation-pinned glyph arena batch, eligibility/build, glyph-buffer binding,
// and pass emission. LLFontGL only decides whether to accept this result or use
// its legacy atlas renderer.
class LLFontGpuRenderer final
{
public:
    struct Request
    {
        const LLFontFreetype& font;
        const LLWString& text;
        const LLFontShapeLayout& layout;
        const LLColor4& color;
        const LLColor4U& shadow_color;
        LLFontGL::pass_boundary_cb_t pass_boundary;
        S32 begin_offset;
        S32 length;
        F32 pen_x;
        F32 pen_y;
        F32 start_x;
        S32 scaled_max_pixels;
        F32 italic_slant;
        U8 style;
        LLFontGL::ShadowType shadow;
        bool subpixel_pen;
        bool use_color;
    };

    struct Result
    {
        bool rendered = false;
        S32 chars_drawn = 0;
        F32 pen_x = 0.f;
        bool emitted_fixed_color_glyph = false;
    };

    static Result tryRender(const Request& request);
    static void destroyGL();
};

#endif // LL_HAS_HB_GPU
#endif // LL_LLFONTGPURENDERER_H
