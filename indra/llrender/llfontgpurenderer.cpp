/**
 * @file llfontgpurenderer.cpp
 * @brief Coordinates shaped text, glyph caching, and GPU batches for analytic font rendering.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include "llfontgpurenderer.h"
#if LL_HAS_HB_GPU
#include "alfontface.h"
#include "llfontfreetype.h"
#include "llfontgpubatch.h"
#include "llfontgpuglyphcache.h"
#include "llfontgpushader.h"
#include "llglslshader.h"
#include "llrender.h"
#include "llshadermgr.h"
#include "llvertexbuffer.h"

namespace {
constexpr F32 kBoldOffset = 1.f;

class ScopedFontShader {
public:
    ScopedFontShader() {
        if (LLGLSLShader::sCurBoundShaderPtr && LLGLSLShader::sCurBoundShaderPtr->mHasFontGpu) {
            mReady = true;
            return;
        }

        gGL.flush();
        mRestore = LLGLSLShader::sCurBoundShaderPtr;
        mScoped = LLFontGpuShader::getBatchedProgram();
        if (mScoped) {
            mScoped->bind();
            mReady = true;
        } else if (mRestore) mRestore->bind();
    }

    ~ScopedFontShader() {
        if (!mScoped) return;

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

struct GlyphQuad {
    LLFontGpuGlyphCache::GlyphLoc loc;
    F32 penX;
    F32 penY;
    F32 scale;
    LLColor4U color;
    bool isColor;
    bool colorAsMask;
};

class GlyphBatchBuilder {
public:
    GlyphBatchBuilder(const LLFontGpuRenderer::Request& request, const LLFontGpuGlyphCache::Batch& cacheBatch)
        : mRequest(request), mCacheBatch(cacheBatch), mPenX(request.penX), mPenY(request.penY),
          mWantColor(request.useColor && !LLFontGL::sForceMonochromeEmoji), mEmitBold((request.style & LLFontGL::BOLD) != 0),
          mEmitShadow(request.shadow != LLFontGL::NO_SHADOW && !mEmitBold), mForegroundColor(request.color) {
        mForeground.reserve(request.textLength);
        if (mEmitShadow) mShadow.reserve(request.textLength);
    }

    bool build() {
        if (!mRequest.layout.valid()) return false;

        size_t covered = 0;
        for (size_t run = 0; run < mRequest.layout.ranges.size() && !mOverflow; ++run) {
            const auto range = mRequest.layout.ranges[run];
            if (range.first != covered || range.second > size_t(mRequest.textLength)) return false;

            const auto& glyphs = *mRequest.layout.glyphs[run];
            if (glyphs.empty()) {
                if (!buildCodepoints(range.first, range.second)) return false;
            } else if (!buildShapedGlyphs(glyphs)) return false;

            if (mOverflow) {
                mCharsDrawn += llmax(0, mOverflowCluster);
                break;
            }
            mCharsDrawn += S32(range.second - range.first);
            covered = range.second;
        }

        return mOverflow || covered == size_t(mRequest.textLength);
    }

    void emit() const {
        emitPass(mShadow);
        if (mRequest.passBoundary) mRequest.passBoundary();
        emitPass(mForeground);
    }

    S32 charsDrawn() const { return mCharsDrawn; }
    F32 penX() const { return mPenX; }
    bool emittedFixedColorGlyph() const { return mEmittedFixedColorGlyph; }

private:
    bool faceEligible(const ALFontFace* face) const {
        return face && face->unitsPerEm() != 0 && face->designToPixelScale() > 0.f && !(mWantColor && face->hasColor() && !face->hasColrV1());
    }

    bool wouldOverflow(F32 right, S32 cluster) {
        if ((mRequest.startX + mRequest.scaledMaxPixels) >= right) return false;

        mOverflow = true;
        mOverflowCluster = cluster;
        return true;
    }

    bool buildCodepoints(size_t begin, size_t end) {
        const EFontGlyphType glyphType = mWantColor ? EFontGlyphType::Color : EFontGlyphType::Grayscale;

        for (size_t cp = begin; cp < end; ++cp) {
            const LLFontGlyphInfo* glyph = mRequest.font.getGlyphInfo(mRequest.text[mRequest.beginOffset + cp], glyphType);
            if (!glyph || !faceEligible(glyph->mSourceFace)) return false;

            const LLFontGlyphInfo* next = (cp + 1 < size_t(mRequest.textLength))
                ? mRequest.font.getGlyphInfo(mRequest.text[mRequest.beginOffset + cp + 1], glyphType)
                : nullptr;

            const ALFontFace* face = glyph->mSourceFace;
            const bool colorGlyph = mWantColor && face->hasColrV1();
            LLFontGpuGlyphCache* cache = colorGlyph ? face->getGpuColorGlyphCache() : face->getGpuGlyphCache();
            if (!cache) return false;

            const LLFontGpuGlyphCache::GlyphLoc loc = cache->getOrEncodeGlyph(mCacheBatch, glyph->mGlyphIndex);
            F32 nextPenX = mPenX + glyph->mXAdvance;
            if (next && cp + 1 < end) nextPenX += mRequest.font.getXKerning(glyph, next);
            if (!mRequest.useSubpixelPen) nextPenX = F32(ll_round(nextPenX));

            if (loc.drawable()) {
                const F32 scale = face->designToPixelScale();
                const F32 right = mPenX + F32(loc.mXBearing + loc.mWidth) * scale;
                if (wouldOverflow(right, S32(cp - begin))) return true;
                placeGlyph(loc, scale, colorGlyph, mPenX, mPenY);
            } else if (wouldOverflow(nextPenX, S32(cp - begin))) return true;

            mPenX = nextPenX;
        }
        return true;
    }

    bool buildShapedGlyphs(const std::vector<ALShapedGlyph>& glyphs) {
        for (const ALShapedGlyph& glyph : glyphs) {
            const ALFontFace* face = glyph.face ? glyph.face->getFontFace() : nullptr;
            if (!faceEligible(face)) return false;

            const bool colorGlyph = mWantColor && face->hasColrV1();
            LLFontGpuGlyphCache* cache = colorGlyph ? face->getGpuColorGlyphCache() : face->getGpuGlyphCache();
            if (!cache) return false;

            const F32 glyphX = mPenX + glyph.x_offset;
            const F32 glyphY = mPenY + glyph.y_offset;
            const LLFontGpuGlyphCache::GlyphLoc loc = cache->getOrEncodeGlyph(mCacheBatch, glyph.glyph_id);
            F32 nextPenX = mPenX + glyph.x_advance;
            const F32 nextPenY = mPenY + glyph.y_advance;
            if (!mRequest.useSubpixelPen) nextPenX = F32(ll_round(nextPenX));

            if (loc.drawable()) {
                const F32 scale = face->designToPixelScale();
                const F32 right = glyphX + F32(loc.mXBearing + loc.mWidth) * scale;
                if (wouldOverflow(right, glyph.cluster)) return true;
                placeGlyph(loc, scale, colorGlyph, glyphX, glyphY);
            } else if (wouldOverflow(nextPenX, glyph.cluster)) return true;

            mPenX = nextPenX;
            mPenY = nextPenY;
        }
        return true;
    }

    void emitShadow(const LLFontGpuGlyphCache::GlyphLoc& loc, F32 scale, bool colorGlyph, F32 penX, F32 penY) {
        if (!mEmitShadow) return;

        auto addShadow = [&](F32 dx, F32 dy) { mShadow.push_back({loc, penX + dx, penY + dy, scale, mRequest.shadowColor, colorGlyph, colorGlyph}); };
        if (mRequest.shadow == LLFontGL::DROP_SHADOW_SOFT) {
            addShadow(-1.f, -1.f);
            addShadow(1.f, -1.f);
            addShadow(1.f, 1.f);
            addShadow(-1.f, 1.f);
            addShadow(0.f, -2.f);
        } else {
            addShadow(1.f, -1.f);
        }
    }

    void placeGlyph(const LLFontGpuGlyphCache::GlyphLoc& loc, F32 scale, bool colorGlyph, F32 penX, F32 penY) {
        if (colorGlyph) {
            emitShadow(loc, scale, colorGlyph, penX, penY);

            mForeground.push_back({loc, penX, penY, scale, mForegroundColor, true, false});
            if (mEmitBold) mForeground.push_back({loc, penX + kBoldOffset, penY, scale, mForegroundColor, true, false});
            mEmittedFixedColorGlyph = true;
            return;
        }

        emitShadow(loc, scale, colorGlyph, penX, penY);

        mForeground.push_back({loc, penX, penY, scale, mForegroundColor, false, false});
        if (mEmitBold) mForeground.push_back({loc, penX + kBoldOffset, penY, scale, mForegroundColor, false, false});
    }

    void emitPass(const std::vector<GlyphQuad>& pass) const {
        for (const GlyphQuad& quad : pass) {
            const U32 glyphLocation = (quad.loc.mTexelOffset & LLVertexBuffer::GLYPH_LOC_OFFSET_MASK)
                | (quad.isColor ? LLVertexBuffer::GLYPH_LOC_COLOR : 0u)
                | (quad.colorAsMask ? LLVertexBuffer::GLYPH_LOC_COLOR_AS_MASK : 0u);
            LLVector4a positions[6];
            LLVector2 texcoords[6];
            LLColor4U colors[6];
            U32 glyphLocations[6];
            LLFontGpuBatch::buildGlyphQuad(positions, texcoords, colors, glyphLocations, quad.loc, quad.penX, quad.penY, quad.scale,
                                           mRequest.italicSlant, quad.color, glyphLocation);
            gGL.begin(LLRender::TRIANGLES);
            gGL.vertexBatchPreTransformed(positions, texcoords, colors, glyphLocations, 6);
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
} // namespace

LLFontGpuRenderer::Result LLFontGpuRenderer::tryRender(const Request& request) {
    Result result;
    if (!request.layout.valid()) return result;

    ScopedFontShader shader;
    if (!shader.ready() || !LLGLSLShader::sCurBoundShaderPtr) return result;

    const S32 glyphUnit = LLGLSLShader::sCurBoundShaderPtr->getTextureChannel(LLShaderMgr::FONT_GLYPH_BUFFER);
    if (glyphUnit < 0) return result;

    LLFontGpuGlyphCache::Batch cacheBatch = LLFontGpuGlyphCache::beginBatch();
    GlyphBatchBuilder builder(request, cacheBatch);
    if (!builder.build()) return result;

    const bool bound = LLFontGpuGlyphCache::bindBufferTexture(glyphUnit);
    if (!bound) return result;

    builder.emit();
    result.rendered = true;
    result.charsDrawn = builder.charsDrawn();
    result.penX = builder.penX();
    result.emittedFixedColorGlyph = builder.emittedFixedColorGlyph();
    return result;
}

void LLFontGpuRenderer::destroyGL() {
    LLFontGpuShader::destroyBatchedProgram();
    LLFontGpuGlyphCache::destroyGL();
}
#endif // LL_HAS_HB_GPU
