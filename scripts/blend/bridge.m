#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stdint.h>
#include <string.h>
#include <sys/sysctl.h>

// Public experiment's only native glue: resource/descriptor setup, encoding,
// completion, and exact byte copies. All generation and comparison is in Rust.
typedef struct {
    uint32_t abi, command_status, completed, no_error, copied, guards;
    uint32_t channels, layout, route, enabled, mask, variant, count, dst_factor;
    uint64_t device_id;
} Evidence;
_Static_assert(sizeof(Evidence) == 64, "Rust/C evidence ABI");
_Static_assert(MTLPixelFormatRG16Float == 65 && MTLPixelFormatRGBA16Float == 115, "format enums");
_Static_assert(MTLColorWriteMaskRed == 8 && MTLColorWriteMaskAll == 15, "mask enums");
_Static_assert(MTLBlendFactorZero == 0 && MTLBlendFactorOne == 1 && MTLBlendOperationAdd == 0, "blend enums");
_Static_assert(MTLCommandBufferStatusCompleted == 4, "completion enum");
enum { WIDTH = 16, ROW = 256, GUARD = 256 };

static int done(NSMutableDictionary *m, NSString *status, NSError *error,
                int code, char *json, size_t capacity) {
    m[@"status"] = status;
    m[@"error"] = error ? @{@"domain": error.domain, @"code": @(error.code),
        @"message": [error.localizedDescription substringToIndex:MIN((NSUInteger)2048, error.localizedDescription.length)]}
        : (id)[NSNull null];
    NSData *data = [NSJSONSerialization dataWithJSONObject:m options:NSJSONWritingSortedKeys error:NULL];
    if (!data || data.length >= capacity) return 70;
    memcpy(json, data.bytes, data.length);
    json[data.length] = 0;
    return code;
}
static NSDictionary *options(MTLCompileOptions *o) {
    NSMutableDictionary *d = [@{@"fast_math": @(o.fastMathEnabled),
        @"language_version": @(o.languageVersion), @"optimization_level": @(o.optimizationLevel),
        @"preserve_invariance": @(o.preserveInvariance)} mutableCopy];
    if (@available(macOS 15.0, *)) {
        d[@"math_mode"] = @(o.mathMode);
        d[@"floating_point_functions"] = @(o.mathFloatingPointFunctions);
    }
    return d;
}
static NSDictionary *attachment(MTLRenderPipelineColorAttachmentDescriptor *a) {
    return @{@"format": @(a.pixelFormat), @"enabled": @(a.blendingEnabled), @"mask": @(a.writeMask),
        @"source_rgb": @(a.sourceRGBBlendFactor), @"destination_rgb": @(a.destinationRGBBlendFactor),
        @"rgb_operation": @(a.rgbBlendOperation), @"source_alpha": @(a.sourceAlphaBlendFactor),
        @"destination_alpha": @(a.destinationAlphaBlendFactor), @"alpha_operation": @(a.alphaBlendOperation)};
}
static id<MTLBuffer> buffer(id<MTLDevice> device, NSUInteger bytes) {
    id<MTLBuffer> b = [device newBufferWithLength:bytes + 2 * GUARD options:MTLResourceStorageModeShared];
    if (b) memset(b.contents, 0xa5, b.length);
    return b;
}
static BOOL guards(id<MTLBuffer> b, NSUInteger payload, NSUInteger row_payload) {
    const uint8_t *p = b.contents;
    for (NSUInteger i = 0; i < b.length; ++i) {
        BOOL edge = i < GUARD || i >= GUARD + payload;
        BOOL pad = !edge && row_payload && (i - GUARD) % ROW >= row_payload;
        if ((edge || pad) && p[i] != 0xa5) return NO;
    }
    return YES;
}
static void texture_to_buffer(id<MTLBlitCommandEncoder> b, id<MTLTexture> t, id<MTLBuffer> out) {
    [b copyFromTexture:t sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0, 0, 0)
        sourceSize:MTLSizeMake(t.width, t.height, 1) toBuffer:out destinationOffset:GUARD
        destinationBytesPerRow:ROW destinationBytesPerImage:t.height * ROW];
}

int blend_native(uint32_t layout, uint32_t route, uint32_t enabled, uint32_t mask, uint32_t variant,
    const char *source, const uint32_t *src, const uint16_t *half_words, const uint16_t *destination,
    size_t count, uint16_t *before, uint16_t *after, uint16_t *first_before, uint16_t *first_after,
    uint32_t *seen, uint16_t *seen_half, uint32_t *export_float, uint16_t *export_half,
    Evidence *e, char *json, size_t capacity) {
    if (!e || !json || !capacity) return 64;
    memset(e, 0, sizeof(*e)); e->abi = 1;
    @autoreleasepool {
        NSMutableDictionary *m = [@{@"schema": @1, @"backend": @"native_metal",
            @"os_version_and_build": NSProcessInfo.processInfo.operatingSystemVersionString,
            @"completed": @NO, @"copied": @NO, @"variant": @(variant)} mutableCopy];
        @try {
            if (layout > 3 || route > 3 || enabled > 1 || (mask != 8 && mask != 15) || variant > 3 ||
                count == 0 || count > 4096 || count % WIDTH || !source || !src || !half_words || !destination ||
                !before || !after || !first_before || !first_after || !seen || !seen_half || !export_float || !export_half)
                return done(m, @"invalid_arguments", nil, 64, json, capacity);
            uint32_t channels = layout == 1 ? 4 : 2;
            BOOL mrt = layout >= 2;
            NSUInteger height = count / WIDTH, row_size = height * ROW;
            e->channels = channels; e->layout = layout; e->route = route;
            e->enabled = enabled; e->mask = variant == 2 ? 15 : mask;
            e->variant = variant; e->count = (uint32_t)count; e->dst_factor = variant == 3 ? 1 : 0;
            m[@"requested"] = @{@"layout": @(layout), @"route": @(route), @"enabled": @(enabled),
                @"mask": @(mask), @"pixels": @(count)};
            char build[128] = {0}; size_t build_size = sizeof(build);
            if (sysctlbyname("kern.osversion", build, &build_size, NULL, 0) != 0)
                return done(m, @"os_build_unavailable", nil, 23, json, capacity);
            m[@"os_build"] = @(build);
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            if (!device) return done(m, @"no_metal_device_no_fallback", nil, 23, json, capacity);
            e->device_id = device.registryID;
            m[@"device"] = @{@"name": device.name ?: @"unknown", @"registry_id": @(device.registryID),
                @"unified_memory": @(device.hasUnifiedMemory), @"low_power": @(device.isLowPower),
                @"removable": @(device.isRemovable), @"mac2": @([device supportsFamily:MTLGPUFamilyMac2]),
                @"apple7": @([device supportsFamily:MTLGPUFamilyApple7]), @"metal3": @([device supportsFamily:MTLGPUFamilyMetal3])};
            MTLCompileOptions *opts = [MTLCompileOptions new];
            m[@"default_compiler_options"] = options(opts);
            opts.fastMathEnabled = NO;
            opts.preprocessorMacros = @{@"WRONG_VALUE": @(variant == 1), @"MULTI_TARGET": @(mrt)};
            m[@"effective_compiler_options"] = options(opts);
            m[@"texture_rounding_override"] = @"none";
            if (opts.fastMathEnabled) return done(m, @"fast_math_option_not_retained", nil, 24, json, capacity);
            NSError *error = nil;
            id<MTLLibrary> library = [device newLibraryWithSource:@(source) options:opts error:&error];
            if (!library || error) return done(m, @"compile_failed", error, 24, json, capacity);
            NSArray *entries = @[@"emit_float", @"emit_half_words", @"emit_widened_words", @"emit_cast_half"];
            MTLRenderPipelineDescriptor *pd = [MTLRenderPipelineDescriptor new];
            pd.vertexFunction = [library newFunctionWithName:@"cover"];
            pd.fragmentFunction = [library newFunctionWithName:entries[route]];
            pd.rasterSampleCount = 1;
            pd.depthAttachmentPixelFormat = MTLPixelFormatInvalid;
            pd.stencilAttachmentPixelFormat = MTLPixelFormatInvalid;
            NSUInteger target_slot = mrt ? 1 : 0;
            MTLRenderPipelineColorAttachmentDescriptor *a = pd.colorAttachments[target_slot];
            a.pixelFormat = channels == 2 ? MTLPixelFormatRG16Float : MTLPixelFormatRGBA16Float;
            a.blendingEnabled = enabled != 0;
            a.writeMask = e->mask;
            a.sourceRGBBlendFactor = MTLBlendFactorOne;
            a.sourceAlphaBlendFactor = MTLBlendFactorOne;
            a.destinationRGBBlendFactor = e->dst_factor ? MTLBlendFactorOne : MTLBlendFactorZero;
            a.destinationAlphaBlendFactor = a.destinationRGBBlendFactor;
            a.rgbBlendOperation = MTLBlendOperationAdd; a.alphaBlendOperation = MTLBlendOperationAdd;
            if (mrt) {
                MTLRenderPipelineColorAttachmentDescriptor *c = pd.colorAttachments[0];
                c.pixelFormat = MTLPixelFormatRGBA16Float; c.blendingEnabled = NO;
                c.writeMask = layout == 3 ? MTLColorWriteMaskNone : MTLColorWriteMaskAll;
                c.sourceRGBBlendFactor = MTLBlendFactorOne; c.sourceAlphaBlendFactor = MTLBlendFactorOne;
                c.destinationRGBBlendFactor = MTLBlendFactorZero; c.destinationAlphaBlendFactor = MTLBlendFactorZero;
                c.rgbBlendOperation = MTLBlendOperationAdd; c.alphaBlendOperation = MTLBlendOperationAdd;
            }
            m[@"target_attachment"] = attachment(a);
            m[@"color0_attachment"] = attachment(pd.colorAttachments[0]);
            m[@"color1_attachment"] = attachment(pd.colorAttachments[1]);
            m[@"samples"] = @(pd.rasterSampleCount);
            if (!pd.vertexFunction || !pd.fragmentFunction || a.writeMask != e->mask || a.blendingEnabled != (enabled != 0) ||
                a.sourceRGBBlendFactor != MTLBlendFactorOne || a.sourceAlphaBlendFactor != MTLBlendFactorOne ||
                (uint32_t)a.destinationRGBBlendFactor != e->dst_factor || a.destinationAlphaBlendFactor != a.destinationRGBBlendFactor ||
                a.rgbBlendOperation != MTLBlendOperationAdd || a.alphaBlendOperation != MTLBlendOperationAdd)
                return done(m, @"pipeline_property_failure", nil, 25, json, capacity);
            id<MTLRenderPipelineState> pipeline = [device newRenderPipelineStateWithDescriptor:pd error:&error];
            if (!pipeline || error) return done(m, @"pipeline_failed", error, 25, json, capacity);

            NSMutableArray<id<MTLTexture>> *textures = [NSMutableArray array];
            NSMutableArray<id<MTLBuffer>> *uploads = [NSMutableArray array];
            NSMutableArray<id<MTLBuffer>> *pre = [NSMutableArray array];
            NSMutableArray<id<MTLBuffer>> *post = [NSMutableArray array];
            for (NSUInteger slot = 0; slot < (mrt ? 2u : 1u); ++slot) {
                uint32_t n = mrt && slot == 0 ? 4 : channels;
                MTLTextureDescriptor *td = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:
                    (n == 2 ? MTLPixelFormatRG16Float : MTLPixelFormatRGBA16Float) width:WIDTH height:height mipmapped:NO];
                td.storageMode = MTLStorageModePrivate; td.usage = MTLTextureUsageRenderTarget;
                id<MTLTexture> t = [device newTextureWithDescriptor:td];
                id<MTLBuffer> u = buffer(device, row_size), p = buffer(device, row_size), q = buffer(device, row_size);
                if (!t || !u || !p || !q) return done(m, @"texture_or_buffer_allocation_failed", nil, 26, json, capacity);
                if (t.pixelFormat != td.pixelFormat || t.width != WIDTH || t.height != height || t.sampleCount != 1 ||
                    t.mipmapLevelCount != 1 || t.arrayLength != 1 || t.storageMode != MTLStorageModePrivate ||
                    t.usage != MTLTextureUsageRenderTarget || t.textureType != MTLTextureType2D || t.framebufferOnly)
                    return done(m, @"texture_property_failure", nil, 26, json, capacity);
                for (NSUInteger i = 0; i < count; ++i)
                    memcpy((uint8_t *)u.contents + GUARD + (i / WIDTH) * ROW + (i % WIDTH) * n * 2,
                        destination + i * 4, n * 2);
                [textures addObject:t]; [uploads addObject:u]; [pre addObject:p]; [post addObject:q];
            }
            id<MTLBuffer> input = [device newBufferWithBytes:src length:count * 16 options:MTLResourceStorageModeShared];
            id<MTLBuffer> raw_half = [device newBufferWithBytes:half_words length:count * 8 options:MTLResourceStorageModeShared];
            id<MTLBuffer> echo = buffer(device, count * 16), echo_half = buffer(device, count * 8);
            id<MTLBuffer> exp_float = buffer(device, count * 16), exp_half = buffer(device, count * 8);
            id<MTLCommandQueue> queue = [device newCommandQueue];
            if (!input || !raw_half || !echo || !echo_half || !exp_float || !exp_half || !queue)
                return done(m, @"buffer_or_queue_allocation_failed", nil, 26, json, capacity);
            id<MTLCommandBuffer> command = [queue commandBuffer];
            if (!command) return done(m, @"command_unavailable", nil, 26, json, capacity);
            id<MTLBlitCommandEncoder> seed = [command blitCommandEncoder];
            if (!seed) return done(m, @"seed_encoder_unavailable", nil, 26, json, capacity);
            for (NSUInteger i = 0; i < textures.count; ++i) {
                [seed copyFromBuffer:uploads[i] sourceOffset:GUARD sourceBytesPerRow:ROW sourceBytesPerImage:row_size
                    sourceSize:MTLSizeMake(WIDTH, height, 1) toTexture:textures[i] destinationSlice:0 destinationLevel:0
                    destinationOrigin:MTLOriginMake(0, 0, 0)];
                texture_to_buffer(seed, textures[i], pre[i]);
            }
            [seed endEncoding];
            MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
            for (NSUInteger i = 0; i < textures.count; ++i) {
                pass.colorAttachments[i].texture = textures[i];
                pass.colorAttachments[i].loadAction = MTLLoadActionLoad;
                pass.colorAttachments[i].storeAction = MTLStoreActionStore;
            }
            id<MTLRenderCommandEncoder> draw = [command renderCommandEncoderWithDescriptor:pass];
            if (!draw) return done(m, @"render_encoder_unavailable", nil, 26, json, capacity);
            [draw setRenderPipelineState:pipeline];
            [draw setViewport:(MTLViewport){0, 0, WIDTH, (double)height, 0, 1}];
            [draw setFragmentBuffer:input offset:0 atIndex:0];
            [draw setFragmentBuffer:raw_half offset:0 atIndex:1];
            [draw setFragmentBuffer:echo offset:GUARD atIndex:2];
            [draw setFragmentBuffer:echo_half offset:GUARD atIndex:3];
            [draw setFragmentBuffer:exp_float offset:GUARD atIndex:4];
            [draw setFragmentBuffer:exp_half offset:GUARD atIndex:5];
            uint32_t shape[2] = {WIDTH, (uint32_t)count};
            [draw setFragmentBytes:shape length:sizeof(shape) atIndex:6];
            [draw drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
            [draw endEncoding];
            id<MTLBlitCommandEncoder> read = [command blitCommandEncoder];
            if (!read) return done(m, @"read_encoder_unavailable", nil, 26, json, capacity);
            for (NSUInteger i = 0; i < textures.count; ++i) texture_to_buffer(read, textures[i], post[i]);
            [read endEncoding]; [command commit]; [command waitUntilCompleted];
            e->command_status = (uint32_t)command.status;
            e->completed = command.status == MTLCommandBufferStatusCompleted;
            e->no_error = command.error == nil;
            m[@"command_status"] = @(command.status); m[@"completed"] = @(e->completed != 0);
            m[@"wait_until_completed_called"] = @YES;
            if (!e->completed || !e->no_error) return done(m, @"command_failed", command.error, 27, json, capacity);
            if (!guards(echo, count * 16, 0) || !guards(echo_half, count * 8, 0) ||
                !guards(exp_float, count * 16, 0) || !guards(exp_half, count * 8, 0))
                return done(m, @"sidecar_guard_changed", nil, 28, json, capacity);
            for (NSUInteger slot = 0; slot < textures.count; ++slot) {
                uint32_t n = mrt && slot == 0 ? 4 : channels;
                if (!guards(uploads[slot], row_size, WIDTH * n * 2) || !guards(pre[slot], row_size, WIDTH * n * 2) ||
                    !guards(post[slot], row_size, WIDTH * n * 2))
                    return done(m, @"readback_guard_changed", nil, 28, json, capacity);
                uint16_t *b = mrt && slot == 0 ? first_before : before;
                uint16_t *aout = mrt && slot == 0 ? first_after : after;
                for (NSUInteger y = 0; y < height; ++y) {
                    memcpy(b + y * WIDTH * n, (uint8_t *)pre[slot].contents + GUARD + y * ROW, WIDTH * n * 2);
                    memcpy(aout + y * WIDTH * n, (uint8_t *)post[slot].contents + GUARD + y * ROW, WIDTH * n * 2);
                }
            }
            memcpy(seen, (uint8_t *)echo.contents + GUARD, count * 16);
            memcpy(seen_half, (uint8_t *)echo_half.contents + GUARD, count * 8);
            memcpy(export_float, (uint8_t *)exp_float.contents + GUARD, count * 16);
            memcpy(export_half, (uint8_t *)exp_half.contents + GUARD, count * 8);
            e->guards = 1; e->copied = 1;
            m[@"copied"] = @YES; m[@"guards_intact"] = @YES;
            m[@"texture"] = @{@"storage": @(MTLStorageModePrivate), @"usage": @(MTLTextureUsageRenderTarget),
                @"width": @(WIDTH), @"height": @(height), @"row_bytes": @(ROW), @"load": @(MTLLoadActionLoad),
                @"store": @(MTLStoreActionStore), @"mrt": @(mrt), @"initialization": @"raw_blit_not_clear"};
            return done(m, @"gpu_readback_observed", nil, 0, json, capacity);
        } @catch (NSException *exception) {
            m[@"exception_name"] = exception.name ?: @"unknown";
            return done(m, @"objective_c_exception", nil, 29, json, capacity);
        }
    }
}
