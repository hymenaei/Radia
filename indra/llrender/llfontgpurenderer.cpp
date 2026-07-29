#include "linden_common.h"

#include "llfontgpurenderer.h"

#if LL_HAS_HB_GPU

#include "llfontface.h"
#include "llfontfreetype.h"
#include "llfontgpubatch.h"
#include "llfontgpuglyphcache.h"
#include "llfontgpushader.h"
#include "llglslshader.h"
#include "llrender.h"
#include "llshadermgr.h"
#include "llvertexbuffer.h"

namespace
{
    constexpr F32 BOLD_OFFSET = 1.f;

    class ScopedFontShader
    {
    public:
        ScopedFontShader()
        {
            if (LLGLSLShader::sCurBoundShaderPtr && LLGLSLShader::sCurBoundShaderPtr->mHasFontGpu)
            {
                mReady = true;
                return;
            }

            gGL.flush();
            mRestore = LLGLSLShader::sCurBoundShaderPtr;
            mScoped = LLFontGpuShader::getBatchedProgram();
            if (mScoped)
            {
                mScoped->bind();
                mReady = true;
            }
            else if (mRestore) mRestore->bind();
        }

        ~ScopedFontShader()
        {
            if (!mScoped) return;

            // Immediate vertices must draw under the analytic program before
            // its lifetime ends.
            gGL.flush();
            if (mRestore) mRestore->bind();
            else mScoped->unbind();
        }

        bool ready() const { return mReady; }

    private:
        LLGLSLShader* mRestore = nullptr;
        LLGLSLShader* mScoped = nullptr;
        bool mReady = false;
    };

    struct GlyphQuad
    {
        LLFontGpuGlyphCache::GlyphLoc loc;
        F32 pen_x;
        F32 pen_y;
        F32 scale;
        LLColor4U color;
        bool is_color;
        bool color_as_mask;
    };

    class GlyphBatchBuilder
    {
    public:
        GlyphBatchBuilder(const LLFontGpuRenderer::Request& request, const LLFontGpuGlyphCache::Batch& cache_batch)
            : mRequest(request),
              mCacheBatch(cache_batch),
              mPenX(request.pen_x),
              mPenY(request.pen_y),
              mWantColor(request.use_color && !LLFontGL::sForceMonochromeEmoji),
              mEmitBold((request.style & LLFontGL::BOLD) != 0),
              mEmitShadow(request.shadow != LLFontGL::NO_SHADOW && !mEmitBold),
              mForegroundColor(request.color)
        {
            mForeground.reserve(request.length);
            if (mEmitShadow) mShadow.reserve(request.length);
        }

        bool build()
        {
            if (!mRequest.layout.valid()) return false;

            size_t covered = 0;
            for (size_t run = 0; run < mRequest.layout.ranges.size() && !mOverflow; ++run)
            {
                const auto range = mRequest.layout.ranges[run];
                if (range.first != covered || range.second > size_t(mRequest.length)) return false;

                const auto& glyphs = *mRequest.layout.glyphs[run];
                if (glyphs.empty())
                {
                    if (!buildCodepoints(range.first, range.second)) return false;
                }
                else if (!buildShapedGlyphs(glyphs)) return false;

                if (mOverflow)
                {
                    mCharsDrawn += llmax(0, mOverflowCluster);
                    break;
                }
                mCharsDrawn += S32(range.second - range.first);
                covered = range.second;
            }

            return mOverflow || covered == size_t(mRequest.length);
        }

        void emit() const
        {
            emitPass(mShadow);
            if (mRequest.pass_boundary) mRequest.pass_boundary();
            emitPass(mForeground);
        }

        S32 charsDrawn() const { return mCharsDrawn; }
        F32 penX() const { return mPenX; }
        bool emittedFixedColorGlyph() const { return mEmittedFixedColorGlyph; }

    private:
        bool faceEligible(const LLFontFace* face) const
        {
            return face && face->unitsPerEm() != 0 && face->designToPixelScale() > 0.f && !(mWantColor && face->hasColor() && !face->hasColrV1());
        }

        bool wouldOverflow(F32 right, S32 cluster)
        {
            if ((mRequest.start_x + mRequest.scaled_max_pixels) >= right) return false;

            mOverflow = true;
            mOverflowCluster = cluster;
            return true;
        }

        bool buildCodepoints(size_t begin, size_t end)
        {
            const EFontGlyphType glyph_type = mWantColor ? EFontGlyphType::Color : EFontGlyphType::Grayscale;

            for (size_t cp = begin; cp < end; ++cp)
            {
                const LLFontGlyphInfo* glyph = mRequest.font.getGlyphInfo(mRequest.text[mRequest.begin_offset + cp], glyph_type);
                if (!glyph || !faceEligible(glyph->mSourceFace)) return false;

                const LLFontGlyphInfo* next = (cp + 1 < size_t(mRequest.length))
                                            ? mRequest.font.getGlyphInfo(mRequest.text[mRequest.begin_offset + cp + 1], glyph_type)
                                            : nullptr;

                const LLFontFace* face = glyph->mSourceFace;
                const bool color_glyph = mWantColor && face->hasColrV1();
                LLFontGpuGlyphCache* cache = color_glyph ? face->getGpuColorGlyphCache() : face->getGpuGlyphCache();
                if (!cache) return false;

                const LLFontGpuGlyphCache::GlyphLoc loc = cache->getGlyph(mCacheBatch, glyph->mGlyphIndex);
                F32 next_pen_x = mPenX + glyph->mXAdvance;
                if (next && cp + 1 < end) next_pen_x += mRequest.font.getXKerning(glyph, next);
                if (!mRequest.subpixel_pen) next_pen_x = F32(ll_round(next_pen_x));

                if (loc.drawable())
                {
                    const F32 scale = face->designToPixelScale();
                    const F32 right = mPenX + F32(loc.mXBearing + loc.mWidth) * scale;
                    if (wouldOverflow(right, S32(cp - begin))) return true;
                    placeGlyph(loc, scale, color_glyph, mPenX, mPenY);
                }
                else if (wouldOverflow(next_pen_x, S32(cp - begin))) return true;

                mPenX = next_pen_x;
            }
            return true;
        }

        bool buildShapedGlyphs(const std::vector<LLShapedGlyph>& glyphs)
        {
            for (const LLShapedGlyph& glyph : glyphs)
            {
                const LLFontFace* face = glyph.face ? glyph.face->getFontFace() : nullptr;
                if (!faceEligible(face)) return false;

                const bool color_glyph = mWantColor && face->hasColrV1();
                LLFontGpuGlyphCache* cache = color_glyph ? face->getGpuColorGlyphCache() : face->getGpuGlyphCache();
                if (!cache) return false;

                const F32 glyph_x = mPenX + glyph.x_offset;
                const F32 glyph_y = mPenY + glyph.y_offset;
                const LLFontGpuGlyphCache::GlyphLoc loc = cache->getGlyph(mCacheBatch, glyph.glyph_id);
                F32 next_pen_x = mPenX + glyph.x_advance;
                const F32 next_pen_y = mPenY + glyph.y_advance;
                if (!mRequest.subpixel_pen) next_pen_x = F32(ll_round(next_pen_x));

                if (loc.drawable())
                {
                    const F32 scale = face->designToPixelScale();
                    const F32 right = glyph_x + F32(loc.mXBearing + loc.mWidth) * scale;
                    if (wouldOverflow(right, glyph.cluster)) return true;
                    placeGlyph(loc, scale, color_glyph, glyph_x, glyph_y);
                }
                else if (wouldOverflow(next_pen_x, glyph.cluster)) return true;

                mPenX = next_pen_x;
                mPenY = next_pen_y;
            }
            return true;
        }

        void placeGlyph(const LLFontGpuGlyphCache::GlyphLoc& loc, F32 scale, bool color_glyph, F32 pen_x, F32 pen_y)
        {
            if (color_glyph)
            {
                if (mEmitShadow)
                {
                    auto add_shadow = [&](F32 dx, F32 dy)
                    {
                        mShadow.push_back({ loc, pen_x + dx, pen_y + dy, scale, mRequest.shadow_color, true, true });
                    };
                    if (mRequest.shadow == LLFontGL::DROP_SHADOW_SOFT)
                    {
                        add_shadow(-1.f, -1.f);
                        add_shadow( 1.f, -1.f);
                        add_shadow( 1.f,  1.f);
                        add_shadow(-1.f,  1.f);
                        add_shadow( 0.f, -2.f);
                    }
                    else add_shadow(1.f, -1.f);
                }

                mForeground.push_back({ loc, pen_x, pen_y, scale, mForegroundColor, true, false });
                if (mEmitBold) mForeground.push_back({ loc, pen_x + BOLD_OFFSET, pen_y, scale, mForegroundColor, true, false });
                mEmittedFixedColorGlyph = true;
                return;
            }

            if (mEmitShadow)
            {
                auto add_shadow = [&](F32 dx, F32 dy)
                {
                    mShadow.push_back({ loc, pen_x + dx, pen_y + dy, scale, mRequest.shadow_color, false, false });
                };
                if (mRequest.shadow == LLFontGL::DROP_SHADOW_SOFT)
                {
                    add_shadow(-1.f, -1.f);
                    add_shadow( 1.f, -1.f);
                    add_shadow( 1.f,  1.f);
                    add_shadow(-1.f,  1.f);
                    add_shadow( 0.f, -2.f);
                }
                else add_shadow(1.f, -1.f);
            }

            mForeground.push_back({ loc, pen_x, pen_y, scale, mForegroundColor, false, false });
            if (mEmitBold) mForeground.push_back({ loc, pen_x + BOLD_OFFSET, pen_y, scale, mForegroundColor, false, false });
        }

        void emitPass(const std::vector<GlyphQuad>& pass) const
        {
            for (const GlyphQuad& quad : pass)
            {
                const U32 glyph_loc =
                    (quad.loc.mTexelOffset & LLVertexBuffer::GLYPH_LOC_OFFSET_MASK) |
                    (quad.is_color ? LLVertexBuffer::GLYPH_LOC_COLOR : 0u) |
                    (quad.color_as_mask ? LLVertexBuffer::GLYPH_LOC_COLOR_AS_MASK : 0u);
                LLVector4a positions[6];
                LLVector2 texcoords[6];
                LLColor4U colors[6];
                U32 glyph_locs[6];
                LLFontGpuBatch::buildGlyphQuad(
                    positions, texcoords, colors, glyph_locs, quad.loc,
                    quad.pen_x, quad.pen_y, quad.scale,
                    mRequest.italic_slant, quad.color, glyph_loc);
                gGL.begin(LLRender::TRIANGLES);
                gGL.vertexBatchPreTransformed(positions, texcoords, colors, glyph_locs, 6);
                gGL.end();
            }
        }

        const LLFontGpuRenderer::Request& mRequest;
        const LLFontGpuGlyphCache::Batch& mCacheBatch;
        std::vector<GlyphQuad> mShadow;
        std::vector<GlyphQuad> mForeground;
        F32 mPenX;
        F32 mPenY;
        const bool mWantColor;
        const bool mEmitBold;
        const bool mEmitShadow;
        const LLColor4U mForegroundColor;
        S32 mCharsDrawn = 0;
        S32 mOverflowCluster = 0;
        bool mOverflow = false;
        bool mEmittedFixedColorGlyph = false;
    };
}

LLFontGpuRenderer::Result
LLFontGpuRenderer::tryRender(const Request& request)
{
    Result result;
    if (!request.layout.valid()) return result;

    ScopedFontShader shader;
    if (!shader.ready() || !LLGLSLShader::sCurBoundShaderPtr) return result;

    const S32 glyph_unit = LLGLSLShader::sCurBoundShaderPtr->getTextureChannel(LLShaderMgr::FONT_GLYPH_BUFFER);
    if (glyph_unit < 0) return result;

    LLFontGpuGlyphCache::Batch cache_batch = LLFontGpuGlyphCache::beginBatch();
    GlyphBatchBuilder builder(request, cache_batch);
    if (!builder.build()) return result;

    gGL.getTexUnit(glyph_unit)->activate();
    const bool bound = LLFontGpuGlyphCache::bindBufferTexture();
    gGL.getTexUnit(0)->activate();
    if (!bound) return result;

    builder.emit();
    result.rendered = true;
    result.chars_drawn = builder.charsDrawn();
    result.pen_x = builder.penX();
    result.emitted_fixed_color_glyph = builder.emittedFixedColorGlyph();
    return result;
}

void LLFontGpuRenderer::destroyGL()
{
    LLFontGpuShader::destroyBatchedProgram();
    LLFontGpuGlyphCache::destroyGL();
}

#endif // LL_HAS_HB_GPU
