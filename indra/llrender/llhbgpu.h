/**
 * @file llhbgpu.h
 * @brief Single source of truth for HarfBuzz-GPU availability.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Alchemy Viewer Source Code
 * Copyright (C) 2026, Rye <rye@alchemyviewer.org>
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

#ifndef LL_LLHBGPU_H
#define LL_LLHBGPU_H

#include <hb.h>
#if HB_VERSION_ATLEAST(14, 2, 0)
    #define LL_HAS_HB_GPU 1
    #include <hb-gpu.h>
#else
    #define LL_HAS_HB_GPU 0
#endif
#endif // LL_LLHBGPU_H
