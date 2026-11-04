#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stdint.h>
#include <float.h>
#include <string.h>

// Generic RG16Float native observation bridge. It owns no CPU reference or
// comparator; it only creates synthetic resources, encodes GPU work and copies
// completed shared-buffer contents back to the Rust host.
typedef struct {
    uint32_t abi_version, command_status, completed, no_error, readback_copied;
    uint32_t pixel_format, usage, storage_mode, texture_count, pipelines_created;
    uint64_t device_registry_id;
} Rg16Evidence;
_Static_assert(sizeof(Rg16Evidence) == 48, "evidence ABI");

_Static_assert(MTLPixelFormatRG16Float == 65 && MTLPixelFormatRGBA16Float == 115 &&
    MTLPixelFormatStencil8 == 253 && MTLPixelFormatInvalid == 0, "pixel format SDK enums");
_Static_assert(MTLTextureUsageRenderTarget == 4 && MTLTextureUsageShaderRead == 1, "usage SDK enum");
_Static_assert(MTLStorageModePrivate == 2, "storage SDK enum");
enum { W = 16, H = 4, P = W * H, ROW_BYTES = 256, GUARD_BYTES = 256, MAX_TEXTURES = 16 };
enum { S_CLEAR = 0, S_RENDER = 1, S_MASK = 2, S_BLEND = 3, S_SAMPLE = 4, S_MIXED = 5, S_LOD = 6 };

static int finish(NSMutableDictionary *m, NSString *status, NSError *error,
                  int code, char *json, size_t capacity) {
    m[@"status"] = status;
    // Do not export descriptions/userInfo: they can contain local paths.
    m[@"error"] = error ? @{ @"domain": error.domain, @"code": @(error.code) }
                         : (id)[NSNull null];
    NSData *data = [NSJSONSerialization dataWithJSONObject:m
        options:NSJSONWritingSortedKeys error:NULL];
    if (!data || data.length >= capacity) return 70;
    memcpy(json, data.bytes, data.length);
    json[data.length] = 0;
    return code;
}

static NSUInteger texture_count(uint32_t scenario, size_t clear_pairs) {
    switch (scenario) {
        case S_CLEAR: return clear_pairs;
        case S_RENDER: return 1;
        case S_MASK: return 7;
        case S_BLEND: return 4;
        case S_SAMPLE: return 1;
        case S_LOD: return 1;
        case S_MIXED: return 2;
        default: return 0;
    }
}

static const MTLColorWriteMask kMasks[7] = {
    MTLColorWriteMaskRed, MTLColorWriteMaskGreen,
    MTLColorWriteMaskBlue, MTLColorWriteMaskAlpha,
    MTLColorWriteMaskRed | MTLColorWriteMaskGreen | MTLColorWriteMaskBlue,
    MTLColorWriteMaskNone,
    MTLColorWriteMaskAll,
};
static const MTLBlendFactor kBlendSrc[4] = {
    MTLBlendFactorSourceAlpha, MTLBlendFactorDestinationAlpha,
    MTLBlendFactorOneMinusDestinationAlpha, MTLBlendFactorZero,
};
static const MTLBlendFactor kBlendDst[4] = {
    MTLBlendFactorOneMinusSourceAlpha, MTLBlendFactorZero,
    MTLBlendFactorOne, MTLBlendFactorDestinationAlpha,
};

static BOOL observe_stencil_defaults(MTLStencilDescriptor *s,
        NSMutableDictionary *records, NSString *key) {
    if (!s) {
        records[key] = @{ @"nil": @YES };
        return YES;
    }
    records[key] = @{
        @"nil": @NO, @"compare": @(s.stencilCompareFunction),
        @"stencil_failure": @(s.stencilFailureOperation),
        @"depth_failure": @(s.depthFailureOperation),
        @"pass": @(s.depthStencilPassOperation),
        @"read_mask": @(s.readMask), @"write_mask": @(s.writeMask)
    };
    // A default descriptor may expose real front/back objects. Observe them;
    // pointer absence is not the semantic contract. This object is never bound.
    return s.stencilCompareFunction == MTLCompareFunctionAlways &&
        s.stencilFailureOperation == MTLStencilOperationKeep &&
        s.depthFailureOperation == MTLStencilOperationKeep &&
        s.depthStencilPassOperation == MTLStencilOperationKeep;
}

static id<MTLRenderPipelineState> make_pipeline(id<MTLDevice> device, id<MTLFunction> v,
        id<MTLFunction> f, MTLColorWriteMask mask, BOOL blend, int config, NSError **error) {
    MTLRenderPipelineDescriptor *d = [MTLRenderPipelineDescriptor new];
    d.vertexFunction = v;
    d.fragmentFunction = f;
    d.rasterSampleCount = 1;
    MTLRenderPipelineColorAttachmentDescriptor *c = d.colorAttachments[0];
    c.pixelFormat = MTLPixelFormatRG16Float;
    c.writeMask = mask;
    c.blendingEnabled = blend;
    if (config >= 0) {
        c.rgbBlendOperation = MTLBlendOperationAdd;
        c.alphaBlendOperation = MTLBlendOperationAdd;
        c.sourceRGBBlendFactor = kBlendSrc[config];
        c.destinationRGBBlendFactor = kBlendDst[config];
        // Irrelevant for a format without alpha storage; deliberately odd.
        c.sourceAlphaBlendFactor = MTLBlendFactorZero;
        c.destinationAlphaBlendFactor = MTLBlendFactorZero;
    }
    return [device newRenderPipelineStateWithDescriptor:d error:error];
}

static id<MTLRenderPipelineState> make_visible_pipeline(id<MTLDevice> device,
        id<MTLFunction> v, id<MTLFunction> f, BOOL mixed,
        NSMutableArray *records, NSError **error) {
    MTLRenderPipelineDescriptor *d = [MTLRenderPipelineDescriptor new];
    d.vertexFunction = v;
    d.fragmentFunction = f;
    d.rasterSampleCount = 1;
    d.colorAttachments[0].pixelFormat = MTLPixelFormatRGBA16Float;
    d.colorAttachments[0].writeMask = MTLColorWriteMaskAll;
    d.colorAttachments[0].blendingEnabled = NO;
    d.colorAttachments[1].pixelFormat = mixed ? MTLPixelFormatRG16Float : MTLPixelFormatInvalid;
    d.depthAttachmentPixelFormat = MTLPixelFormatInvalid;
    d.stencilAttachmentPixelFormat = mixed ? MTLPixelFormatStencil8 : MTLPixelFormatInvalid;
    [records addObject:@{
        @"mixed_attachments": @(mixed),
        @"color0": @(d.colorAttachments[0].pixelFormat),
        @"color1": @(d.colorAttachments[1].pixelFormat),
        @"depth": @(d.depthAttachmentPixelFormat),
        @"stencil": @(d.stencilAttachmentPixelFormat),
        @"sample_count": @(d.rasterSampleCount)
    }];
    return [device newRenderPipelineStateWithDescriptor:d error:error];
}

int rg16_native(uint32_t scenario, uint32_t variant, const char *source,
                const float *src, const float *dst, size_t color_floats,
                const double *clears, size_t clear_pairs,
                uint32_t *words, size_t word_count, float *aux, size_t aux_count,
                Rg16Evidence *e, char *json, size_t capacity) {
    if (!e || !json || !capacity) return 64;
    memset(e, 0, sizeof(*e));
    e->abi_version = 1;
    @autoreleasepool {
        NSMutableDictionary *m = [@{
            @"schema": @1, @"backend": @"native_metal", @"scenario": @(scenario),
            @"variant": @(variant), @"command_completed": @NO, @"readback_copied": @NO,
            @"wait_until_completed_called": @NO, @"width": @(W), @"height": @(H),
            @"os_version_and_build": NSProcessInfo.processInfo.operatingSystemVersionString
        } mutableCopy];
        @try {
            NSUInteger count = texture_count(scenario, clear_pairs);
            size_t want_aux = (scenario == S_SAMPLE || scenario == S_LOD) ? (size_t)P * 8 : 0;
            if (!source || !src || !dst || !words || color_floats != (size_t)P * 4 ||
                variant > 1 || count == 0 || count > MAX_TEXTURES ||
                word_count != count * P * (scenario == S_MIXED ? 2 : 1) || aux_count != want_aux ||
                (want_aux && !aux) || (scenario == S_CLEAR && !clears))
                return finish(m, @"invalid_arguments", nil, 64, json, capacity);
            e->texture_count = (uint32_t)count;
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            m[@"selection"] = @"system_default_only";
            if (!device) return finish(m, @"no_metal_device_no_fallback", nil, 23, json, capacity);
            e->device_registry_id = device.registryID;
            NSMutableDictionary *families = [NSMutableDictionary dictionary];
            // Families available at the macOS 13 deployment target only.
            const MTLGPUFamily fam[4] = {
                MTLGPUFamilyApple7, MTLGPUFamilyApple8, MTLGPUFamilyMac2, MTLGPUFamilyMetal3
            };
            NSArray *fam_names = @[@"apple7", @"apple8", @"mac2", @"metal3"];
            for (NSUInteger i = 0; i < 4; i++)
                families[fam_names[i]] = @([device supportsFamily:fam[i]]);
            m[@"device"] = @{
                @"name": device.name ?: @"unknown", @"registry_id": @(device.registryID),
                @"unified_memory": @(device.hasUnifiedMemory), @"low_power": @(device.isLowPower),
                @"removable": @(device.isRemovable), @"families": families
            };
            MTLCompileOptions *options = [MTLCompileOptions new];
            options.fastMathEnabled = NO;
            m[@"fast_math_enabled_getter"] = @(options.fastMathEnabled);
            m[@"language_version_getter"] = @(options.languageVersion);
            BOOL swap_fragment = (scenario == S_RENDER || scenario == S_MIXED) && variant == 1;
            BOOL swap_read = scenario == S_SAMPLE && variant == 1;
            options.preprocessorMacros = @{
                @"PROBE_SWAP_FRAGMENT": @(swap_fragment ? 1 : 0),
                @"PROBE_SWAP_READ": @(swap_read ? 1 : 0),
                @"PROBE_WRONG_LOD": @((scenario == S_LOD && variant == 1) ? 1 : 0)
            };
            NSError *error = nil;
            id<MTLLibrary> library = [device newLibraryWithSource:@(source) options:options error:&error];
            if (!library || error) return finish(m, @"compile_failed_or_reported_error", error, 24, json, capacity);
            id<MTLFunction> vfn = [library newFunctionWithName:@"rg16_vertex"];
            id<MTLFunction> ffn = [library newFunctionWithName:@"rg16_fragment"];
            id<MTLFunction> kfn = [library newFunctionWithName:@"rg16_read"];
            if (!vfn || !ffn || !kfn) return finish(m, @"entry_missing", nil, 24, json, capacity);

            // Seed pipeline: all channels, no blending.
            error = nil;
            NSMutableArray *visible_psos = [NSMutableArray array];
            id<MTLRenderPipelineState> seed = scenario == S_MIXED
                ? make_visible_pipeline(device, vfn, ffn, NO, visible_psos, &error)
                : make_pipeline(device, vfn, ffn, MTLColorWriteMaskAll, NO, -1, &error);
            if (!seed || error) return finish(m, @"seed_pipeline_failed", error, 25, json, capacity);
            NSMutableArray *tests = [NSMutableArray array];
            NSMutableArray *test_desc = [NSMutableArray array];
            if (scenario == S_MASK || scenario == S_BLEND) {
                for (NSUInteger k = 0; k < count; k++) {
                    MTLColorWriteMask mask = MTLColorWriteMaskAll;
                    BOOL blend = NO;
                    int config = -1;
                    if (scenario == S_MASK) {
                        mask = kMasks[k];
                        // Deliberately wrong mask for the negative control.
                        if (variant == 1 && k == 0) mask = MTLColorWriteMaskAll;
                    } else {
                        config = (int)k;
                        mask = MTLColorWriteMaskRed | MTLColorWriteMaskGreen | MTLColorWriteMaskBlue;
                        // Deliberately disabled blending for the negative control.
                        blend = !(variant == 1 && k == 0);
                    }
                    error = nil;
                    id<MTLRenderPipelineState> p = make_pipeline(device, vfn, ffn, mask, blend, config, &error);
                    if (!p || error) {
                        m[@"failed_test_pipeline"] = @(k);
                        return finish(m, @"test_pipeline_failed", error, 25, json, capacity);
                    }
                    [tests addObject:p];
                    [test_desc addObject:@{ @"index": @(k), @"write_mask": @((NSUInteger)mask),
                        @"blending_enabled": @(blend), @"config": @(config) }];
                }
            }
            if (scenario == S_MIXED) {
                error = nil;
                id<MTLRenderPipelineState> mixed = make_visible_pipeline(device, vfn, ffn, YES, visible_psos, &error);
                if (!mixed || error) return finish(m, @"mixed_pipeline_failed", error, 25, json, capacity);
                [tests addObject:mixed];
                m[@"visible_pso_declarations"] = visible_psos;
                m[@"depth_stencil_state_setter_called"] = @NO;
                MTLDepthStencilDescriptor *defaults = [MTLDepthStencilDescriptor new];
                m[@"unbound_default_depth_stencil_descriptor"] = @{
                    @"depth_compare": @(defaults.depthCompareFunction),
                    @"depth_write_enabled": @(defaults.depthWriteEnabled),
                    @"front_stencil_nil": @(defaults.frontFaceStencil == nil),
                    @"back_stencil_nil": @(defaults.backFaceStencil == nil)
                };
                BOOL front_inert = observe_stencil_defaults(defaults.frontFaceStencil, m, @"unbound_front_stencil");
                BOOL back_inert = observe_stencil_defaults(defaults.backFaceStencil, m, @"unbound_back_stencil");
                if (defaults.depthCompareFunction != MTLCompareFunctionAlways || defaults.depthWriteEnabled ||
                    !front_inert || !back_inert)
                    return finish(m, @"unexpected_api_defaults", nil, 25, json, capacity);
            }
            m[@"test_pipelines"] = test_desc;
            if (scenario == S_LOD) {
                id<MTLFunction> lf = [library newFunctionWithName:@"rg16_lod_fragment"];
                if (!lf) return finish(m, @"lod_entry_missing", nil, 24, json, capacity);
                error = nil;
                id<MTLRenderPipelineState> lp = make_pipeline(device, vfn, lf, MTLColorWriteMaskAll, NO, -1, &error);
                if (!lp || error) return finish(m, @"lod_pipeline_failed", error, 25, json, capacity);
                [tests addObject:lp];
            }
            e->pipelines_created = (uint32_t)(1 + tests.count);
            id<MTLComputePipelineState> reader = nil;
            if (scenario == S_SAMPLE) {
                error = nil;
                reader = [device newComputePipelineStateWithFunction:kfn error:&error];
                if (!reader || error) return finish(m, @"read_pipeline_failed", error, 25, json, capacity);
                if (reader.maxTotalThreadsPerThreadgroup < P)
                    return finish(m, @"threadgroup_too_small", nil, 25, json, capacity);
                e->pipelines_created += 1;
            }

            MTLTextureDescriptor *td = [MTLTextureDescriptor new];
            td.textureType = MTLTextureType2D;
            td.pixelFormat = scenario == S_MIXED ? MTLPixelFormatRGBA16Float : MTLPixelFormatRG16Float;
            td.width = W;
            td.height = H;
            td.depth = 1;
            td.mipmapLevelCount = 1;
            td.sampleCount = 1;
            td.arrayLength = 1;
            td.storageMode = MTLStorageModePrivate;
            td.usage = MTLTextureUsageRenderTarget |
                ((scenario == S_SAMPLE || scenario == S_LOD) ? MTLTextureUsageShaderRead : 0);
            NSMutableArray *textures = [NSMutableArray array];
            for (NSUInteger k = 0; k < count; k++) {
                id<MTLTexture> t = [device newTextureWithDescriptor:td];
                if (!t) return finish(m, @"texture_allocation_failed", nil, 26, json, capacity);
                if (t.width != W || t.height != H || t.depth != 1 || t.mipmapLevelCount != 1 ||
                    t.sampleCount != 1 || t.arrayLength != 1 || t.textureType != MTLTextureType2D || t.framebufferOnly)
                    return finish(m, @"visible_texture_property_mismatch", nil, 26, json, capacity);
                if (k == 0) {
                    e->pixel_format = (uint32_t)t.pixelFormat;
                    e->usage = (uint32_t)t.usage;
                    e->storage_mode = (uint32_t)t.storageMode;
                } else if (e->pixel_format != (uint32_t)t.pixelFormat ||
                           e->usage != (uint32_t)t.usage ||
                           e->storage_mode != (uint32_t)t.storageMode) {
                    return finish(m, @"texture_properties_inconsistent", nil, 26, json, capacity);
                }
                [textures addObject:t];
            }
            id<MTLTexture> discarded_rg = nil, discarded_stencil = nil;
            if (scenario == S_MIXED) {
                td.pixelFormat = MTLPixelFormatRG16Float;
                discarded_rg = [device newTextureWithDescriptor:td];
                td.pixelFormat = MTLPixelFormatStencil8;
                discarded_stencil = [device newTextureWithDescriptor:td];
                if (!discarded_rg || !discarded_stencil)
                    return finish(m, @"discard_attachment_allocation_failed", nil, 26, json, capacity);
                NSMutableArray *attachments = [NSMutableArray array];
                for (id<MTLTexture> t in @[discarded_rg, discarded_stencil]) {
                    if (t.width != W || t.height != H || t.depth != 1 || t.mipmapLevelCount != 1 ||
                        t.sampleCount != 1 || t.arrayLength != 1 || t.textureType != MTLTextureType2D ||
                        t.usage != MTLTextureUsageRenderTarget || t.storageMode != MTLStorageModePrivate ||
                        t.framebufferOnly ||
                        t.pixelFormat != (t == discarded_rg ? MTLPixelFormatRG16Float : MTLPixelFormatStencil8))
                        return finish(m, @"discard_attachment_property_mismatch", nil, 26, json, capacity);
                    [attachments addObject:@{ @"pixel_format": @(t.pixelFormat), @"usage": @(t.usage),
                        @"storage_mode": @(t.storageMode), @"width": @(t.width), @"height": @(t.height),
                        @"mips": @(t.mipmapLevelCount), @"samples": @(t.sampleCount),
                        @"array_length": @(t.arrayLength), @"load_action": @(MTLLoadActionDontCare),
                        @"store_action": @(MTLStoreActionDontCare), @"resolve_texture_nil": @YES }];
                }
                m[@"discarded_attachments_validated"] = attachments;
                m[@"discarded_attachment_bytes_read"] = @NO;
                m[@"actual_texture_resource_count"] = @4;
            }
            m[@"texture_observed"] = @{ @"pixel_format": @(e->pixel_format), @"usage": @(e->usage),
                @"storage_mode": @(e->storage_mode), @"count": @(count) };

            NSUInteger color_bytes = color_floats * sizeof(float);
            id<MTLBuffer> src_buf = [device newBufferWithBytes:src length:color_bytes
                options:MTLResourceStorageModeShared];
            id<MTLBuffer> dst_buf = [device newBufferWithBytes:dst length:color_bytes
                options:MTLResourceStorageModeShared];
            id<MTLBuffer> readback = [device newBufferWithLength:count * H * ROW_BYTES + 2 * GUARD_BYTES
                options:MTLResourceStorageModeShared];
            id<MTLBuffer> aux_buf = want_aux ? [device newBufferWithLength:want_aux * sizeof(float)
                options:MTLResourceStorageModeShared] : nil;
            id<MTLCommandQueue> queue = [device newCommandQueue];
            if (!src_buf || !dst_buf || !readback || (want_aux && !aux_buf) || !queue)
                return finish(m, @"allocation_failed", nil, 26, json, capacity);
            memset(readback.contents, 0xa5, readback.length);
            if (aux_buf) memset(aux_buf.contents, 0xa5, want_aux * sizeof(float));
            id<MTLCommandBuffer> command = [queue commandBuffer];
            if (!command) return finish(m, @"command_unavailable", nil, 26, json, capacity);
            uint32_t width = W;
            NSUInteger pixel_bytes = scenario == S_MIXED ? 8 : 4;
            NSMutableArray *clear_desc = [NSMutableArray array];
            for (NSUInteger k = 0; k < count; k++) {
                id<MTLTexture> t = textures[k];
                MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
                pass.colorAttachments[0].texture = t;
                pass.colorAttachments[0].loadAction = MTLLoadActionClear;
                pass.colorAttachments[0].storeAction = MTLStoreActionStore;
                MTLClearColor cc = MTLClearColorMake(-3.0, 7.0, 0.5, 0.5);
                if (scenario == S_CLEAR) {
                    double r = clears[2 * k], g = clears[2 * k + 1];
                    // Deliberately swapped clear channels for the negative control.
                    if (variant == 1) { double x = r; r = g; g = x; }
                    // B and A are deliberately non-default and must be discarded.
                    cc = MTLClearColorMake(r, g, 0.75, 0.25);
                    [clear_desc addObject:@[@(r), @(g)]];
                }
                pass.colorAttachments[0].clearColor = cc;
                if (scenario == S_MIXED && k == 1) {
                    pass.colorAttachments[1].texture = discarded_rg;
                    pass.colorAttachments[1].loadAction = MTLLoadActionDontCare;
                    pass.colorAttachments[1].storeAction = MTLStoreActionDontCare;
                    pass.stencilAttachment.texture = discarded_stencil;
                    pass.stencilAttachment.loadAction = MTLLoadActionDontCare;
                    pass.stencilAttachment.storeAction = MTLStoreActionDontCare;
                    // No depth attachment and no depth/stencil state setter.
                    m[@"mixed_pass_encoded"] = @{
                        @"color0_load": @(pass.colorAttachments[0].loadAction),
                        @"color0_store": @(pass.colorAttachments[0].storeAction),
                        @"color1_load": @(pass.colorAttachments[1].loadAction),
                        @"color1_store": @(pass.colorAttachments[1].storeAction),
                        @"stencil_load": @(pass.stencilAttachment.loadAction),
                        @"stencil_store": @(pass.stencilAttachment.storeAction),
                        @"depth_texture_nil": @(pass.depthAttachment.texture == nil),
                        @"color1_store_options": @(pass.colorAttachments[1].storeActionOptions),
                        @"stencil_store_options": @(pass.stencilAttachment.storeActionOptions)
                    };
                    if (pass.depthAttachment.texture != nil ||
                        pass.colorAttachments[1].resolveTexture != nil || pass.stencilAttachment.resolveTexture != nil)
                        return finish(m, @"unexpected_depth_or_resolve_attachment", nil, 26, json, capacity);
                }
                id<MTLRenderCommandEncoder> enc = [command renderCommandEncoderWithDescriptor:pass];
                if (!enc) return finish(m, @"render_encoder_unavailable", nil, 26, json, capacity);
                if (scenario != S_CLEAR) {
                    BOOL two_pass = scenario == S_MASK || scenario == S_BLEND;
                    [enc setRenderPipelineState:(scenario == S_MIXED && k == 1 ? tests[0] : seed)];
                    [enc setFragmentBuffer:(two_pass ? dst_buf : src_buf) offset:0 atIndex:0];
                    [enc setFragmentBytes:&width length:sizeof(width) atIndex:1];
                    [enc drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
                }
                [enc endEncoding];
                if (scenario == S_MASK || scenario == S_BLEND) {
                    MTLRenderPassDescriptor *p2 = [MTLRenderPassDescriptor renderPassDescriptor];
                    p2.colorAttachments[0].texture = t;
                    p2.colorAttachments[0].loadAction = MTLLoadActionLoad;
                    p2.colorAttachments[0].storeAction = MTLStoreActionStore;
                    id<MTLRenderCommandEncoder> e2 = [command renderCommandEncoderWithDescriptor:p2];
                    if (!e2) return finish(m, @"render_encoder_unavailable", nil, 26, json, capacity);
                    [e2 setRenderPipelineState:tests[k]];
                    [e2 setFragmentBuffer:src_buf offset:0 atIndex:0];
                    [e2 setFragmentBytes:&width length:sizeof(width) atIndex:1];
                    [e2 drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
                    [e2 endEncoding];
                }
            }
            m[@"clear_colors_encoded"] = clear_desc;
            if (scenario == S_SAMPLE) {
                id<MTLComputeCommandEncoder> ce = [command computeCommandEncoder];
                if (!ce) return finish(m, @"compute_encoder_unavailable", nil, 26, json, capacity);
                [ce setComputePipelineState:reader];
                [ce setTexture:textures[0] atIndex:0];
                [ce setTexture:textures[0] atIndex:1];
                [ce setBuffer:aux_buf offset:0 atIndex:0];
                [ce dispatchThreadgroups:MTLSizeMake(1, 1, 1)
                    threadsPerThreadgroup:MTLSizeMake(W, H, 1)];
                [ce endEncoding];
            }
            if (scenario == S_LOD) {
                NSMutableArray *samplers = [NSMutableArray array];
                NSMutableArray *states = [NSMutableArray array];
                for (NSUInteger k = 0; k < 2; k++) {
                    MTLSamplerDescriptor *sd = [MTLSamplerDescriptor new];
                    sd.minFilter = MTLSamplerMinMagFilterLinear;
                    sd.magFilter = MTLSamplerMinMagFilterLinear;
                    sd.mipFilter = MTLSamplerMipFilterNotMipmapped;
                    sd.normalizedCoordinates = YES;
                    sd.maxAnisotropy = 1;
                    sd.lodMinClamp = 0.0f;
                    sd.lodMaxClamp = FLT_MAX;
                    sd.sAddressMode = MTLSamplerAddressModeClampToEdge;
                    sd.tAddressMode = MTLSamplerAddressModeClampToEdge;
                    sd.rAddressMode = MTLSamplerAddressModeClampToEdge;
                    sd.compareFunction = MTLCompareFunctionNever;
                    sd.lodAverage = k == 1;
                    if (sd.lodAverage != (k == 1) || sd.minFilter != MTLSamplerMinMagFilterLinear ||
                        sd.magFilter != sd.minFilter || sd.mipFilter != MTLSamplerMipFilterNotMipmapped ||
                        sd.maxAnisotropy != 1 || !sd.normalizedCoordinates)
                        return finish(m, @"sampler_property_mismatch", nil, 26, json, capacity);
                    id<MTLSamplerState> ss = [device newSamplerStateWithDescriptor:sd];
                    if (!ss) return finish(m, @"sampler_allocation_failed", nil, 26, json, capacity);
                    [samplers addObject:ss];
                    [states addObject:@{ @"lod_average": @(sd.lodAverage), @"min": @(sd.minFilter),
                        @"mag": @(sd.magFilter), @"mip": @(sd.mipFilter), @"max_anisotropy": @(sd.maxAnisotropy),
                        @"normalized": @(sd.normalizedCoordinates), @"lod_min": @(sd.lodMinClamp),
                        @"lod_max": @(sd.lodMaxClamp), @"s_address": @(sd.sAddressMode),
                        @"t_address": @(sd.tAddressMode), @"r_address": @(sd.rAddressMode),
                        @"compare": @(sd.compareFunction) }];
                }
                m[@"sampler_descriptors_observed"] = states;
                m[@"lod_sample_methods"] = @"top two rows implicit derivatives; bottom two explicit varying gradients; readback pairs individual,averaged";
                td.usage = MTLTextureUsageRenderTarget;
                id<MTLTexture> draw_target = [device newTextureWithDescriptor:td];
                if (!draw_target) return finish(m, @"sample_draw_target_failed", nil, 26, json, capacity);
                MTLRenderPassDescriptor *sp = [MTLRenderPassDescriptor renderPassDescriptor];
                sp.colorAttachments[0].texture = draw_target;
                sp.colorAttachments[0].loadAction = MTLLoadActionDontCare;
                sp.colorAttachments[0].storeAction = MTLStoreActionDontCare;
                id<MTLRenderCommandEncoder> se = [command renderCommandEncoderWithDescriptor:sp];
                if (!se) return finish(m, @"sample_encoder_failed", nil, 26, json, capacity);
                [se setRenderPipelineState:tests[0]];
                [se setFragmentTexture:textures[0] atIndex:0];
                [se setFragmentSamplerState:samplers[0] atIndex:0];
                [se setFragmentSamplerState:samplers[1] atIndex:1];
                [se setFragmentBuffer:aux_buf offset:0 atIndex:0];
                [se setFragmentBytes:&width length:sizeof(width) atIndex:1];
                [se setFragmentBuffer:dst_buf offset:0 atIndex:2];
                [se drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
                [se endEncoding];
            }
            id<MTLBlitCommandEncoder> blit = [command blitCommandEncoder];
            if (!blit) return finish(m, @"blit_encoder_unavailable", nil, 26, json, capacity);
            for (NSUInteger k = 0; k < count; k++) {
                [blit copyFromTexture:textures[k] sourceSlice:0 sourceLevel:0
                    sourceOrigin:MTLOriginMake(0, 0, 0) sourceSize:MTLSizeMake(W, H, 1)
                    toBuffer:readback destinationOffset:GUARD_BYTES + k * H * ROW_BYTES
                    destinationBytesPerRow:ROW_BYTES destinationBytesPerImage:H * ROW_BYTES];
            }
            [blit endEncoding];
            [command commit];
            [command waitUntilCompleted];
            e->command_status = (uint32_t)command.status;
            e->completed = command.status == MTLCommandBufferStatusCompleted;
            e->no_error = command.error == nil;
            m[@"command_status"] = @(command.status);
            m[@"command_completed"] = @(e->completed != 0);
            m[@"wait_until_completed_called"] = @YES;
            m[@"command_error"] = command.error ? @{
                @"domain": command.error.domain, @"code": @(command.error.code)
            } : (id)[NSNull null];
            if (!e->completed || !e->no_error)
                return finish(m, @"command_failed", command.error, 27, json, capacity);
            // Shared storage is read only after successful GPU completion.
            const uint8_t *raw = readback.contents;
            for (NSUInteger i = 0; i < readback.length; i++) {
                BOOL guard = i < GUARD_BYTES || i >= GUARD_BYTES + count * H * ROW_BYTES;
                BOOL padding = !guard && (i - GUARD_BYTES) % ROW_BYTES >= W * pixel_bytes;
                if ((guard || padding) && raw[i] != 0xa5)
                    return finish(m, @"readback_guard_changed", nil, 28, json, capacity);
            }
            for (NSUInteger k = 0; k < count; k++)
                for (NSUInteger y = 0; y < H; y++)
                    memcpy((uint8_t *)words + (k * P + y * W) * pixel_bytes,
                        raw + GUARD_BYTES + (k * H + y) * ROW_BYTES, W * pixel_bytes);
            m[@"readback_guards_intact"] = @YES;
            m[@"blit_row_bytes"] = @(ROW_BYTES);
            if (aux_buf) memcpy(aux, aux_buf.contents, want_aux * sizeof(float));
            e->readback_copied = 1;
            m[@"readback_copied"] = @YES;
            m[@"layout"] = scenario == S_MIXED ? @"two visible RGBA16Float surfaces: baseline then mixed; row-major little-endian R,G,B,A uint16; discarded attachments never read" : @"little-endian uint32 per texel: low16=R binary16, high16=G binary16; row-major 16x4 per texture";
            return finish(m, @"gpu_readback_observed", nil, 0, json, capacity);
        } @catch (NSException *exception) {
            m[@"exception_name"] = exception.name ?: @"unknown";
            return finish(m, @"objective_c_exception", nil, 29, json, capacity);
        }
    }
}
