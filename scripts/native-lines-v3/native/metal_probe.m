#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <dispatch/dispatch.h>
#include <AvailabilityMacros.h>
#include <math.h>
#include <stdio.h>
#include <string.h>
#include <sys/sysctl.h>
#include <sys/utsname.h>
#include "metal_probe.h"
#include "metal_source.h"

// Native API plumbing only. No line-coverage implementation or expected bitmap.
@interface GPContext : NSObject {
@public
    id<MTLDevice> device;
    id<MTLCommandQueue> queue;
    id<MTLLibrary> library;
    id<MTLRenderPipelineState> attached, stencilOnly, clipAttached, clipStencilOnly, query;
    id<MTLDepthStencilState> replace, keep, equal;
}
@end
@implementation GPContext
@end

static void text(char *dst, size_t n, const char *src) {
    if (n) snprintf(dst, n, "%s", src ? src : "");
}
static int failure(GPReceipt *r, const char *reason, NSError *err) {
    text(r->reason, sizeof(r->reason), reason);
    if (err) {
        r->error_present = 1;
        r->error_code = (int64_t)err.code;
        // Never serialize localizedDescription, userInfo, stack traces or paths.
        const char *s = err.domain.UTF8String;
        size_t j = 0;
        for (size_t i = 0; s && s[i] && j + 1 < sizeof(r->error_domain); ++i) {
            char c = s[i];
            r->error_domain[j++] = ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
                (c >= '0' && c <= '9') || c == '.' || c == '_' || c == '-') ? c : '_';
        }
        r->error_domain[j] = 0;
    }
    return 1;
}
static BOOL isParavirtual(id<MTLDevice> d) {
    return [d.name rangeOfString:@"Apple" options:NSCaseInsensitiveSearch].location != NSNotFound &&
        [d.name rangeOfString:@"Paravirtual" options:NSCaseInsensitiveSearch].location != NSNotFound;
}
static id<MTLTexture> texture(GPContext *c, MTLPixelFormat format) {
    MTLTextureDescriptor *d = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:format
        width:GP_SIDE height:GP_SIDE mipmapped:NO];
    d.storageMode = MTLStorageModePrivate;
    d.usage = MTLTextureUsageRenderTarget;
    d.sampleCount = 1;
    d.hazardTrackingMode = MTLHazardTrackingModeTracked;
    return [c->device newTextureWithDescriptor:d];
}
static id<MTLRenderPipelineState> pipeline(GPContext *c, uint32_t mode, BOOL queryPass, BOOL directClip,
                                         GPReceipt *r) {
    r->mode = mode;
    MTLRenderPipelineDescriptor *d = [MTLRenderPipelineDescriptor new];
    d.vertexFunction = [c->library newFunctionWithName:queryPass ? @"query_position" :
        (directClip ? @"direct_clip_position" : @"line_position")];
    d.fragmentFunction = queryPass ? [c->library newFunctionWithName:@"query_white"] : nil;
    d.rasterizationEnabled = YES;
    d.rasterSampleCount = 1;
    d.alphaToCoverageEnabled = NO;
    d.alphaToOneEnabled = NO;
    d.inputPrimitiveTopology = MTLPrimitiveTopologyClassUnspecified;
    d.depthAttachmentPixelFormat = MTLPixelFormatInvalid;
    d.stencilAttachmentPixelFormat = MTLPixelFormatStencil8;
    if (queryPass) {
        d.colorAttachments[0].pixelFormat = MTLPixelFormatRGBA8Unorm;
        d.colorAttachments[0].writeMask = MTLColorWriteMaskAll;
        d.colorAttachments[0].blendingEnabled = NO;
    } else if (mode == 0) {
        d.colorAttachments[0].pixelFormat = MTLPixelFormatRGBA16Float;
        d.colorAttachments[1].pixelFormat = MTLPixelFormatRG16Float;
        for (NSUInteger i = 0; i < 2; ++i) {
            d.colorAttachments[i].writeMask = MTLColorWriteMaskNone;
            d.colorAttachments[i].blendingEnabled = NO;
        }
    }
    if (!d.vertexFunction || (queryPass && !d.fragmentFunction)) {
        failure(r, "authored_function_missing", nil); return nil;
    }
    NSError *error = nil;
    id<MTLRenderPipelineState> result = [c->device newRenderPipelineStateWithDescriptor:d error:&error];
    if (!result || error) { failure(r, "pipeline_unsupported_or_error", error); return nil; }
    text(r->reason, sizeof(r->reason), "pipeline_created_not_measured");
    return result;
}
static id<MTLDepthStencilState> stencilState(GPContext *c, BOOL queryPass, BOOL keepPass) {
    MTLStencilDescriptor *s = [MTLStencilDescriptor new];
    s.stencilCompareFunction = queryPass ? MTLCompareFunctionEqual : MTLCompareFunctionAlways;
    s.stencilFailureOperation = MTLStencilOperationKeep;
    s.depthFailureOperation = MTLStencilOperationKeep;
    s.depthStencilPassOperation = (queryPass || keepPass) ? MTLStencilOperationKeep : MTLStencilOperationReplace;
    s.readMask = 255;
    s.writeMask = queryPass ? 0 : 255;
    MTLDepthStencilDescriptor *d = [MTLDepthStencilDescriptor new];
    d.depthCompareFunction = MTLCompareFunctionAlways;
    d.depthWriteEnabled = NO;
    d.frontFaceStencil = s;
    d.backFaceStencil = s;
    return [c->device newDepthStencilStateWithDescriptor:d];
}

int gp_check_abi(uint32_t abi, size_t a, size_t b, size_t c) {
    return abi == GP_ABI && a == sizeof(GPCase) && b == sizeof(GPReceipt) && c == sizeof(GPMetadata);
}
int gp_open(uint64_t requested_registry, void **handle, GPMetadata *m, GPReceipt *r) {
    if (!handle || !m || !r) return 1;
    *handle = NULL;
    memset(m, 0, sizeof(*m)); memset(r, 0, sizeof(*r));
    @autoreleasepool { @try {
        NSOperatingSystemVersion v = NSProcessInfo.processInfo.operatingSystemVersion;
        snprintf(m->os_version, sizeof(m->os_version), "%ld.%ld.%ld",
                 (long)v.majorVersion, (long)v.minorVersion, (long)v.patchVersion);
        size_t n = sizeof(m->os_build);
        if (sysctlbyname("kern.osversion", m->os_build, &n, NULL, 0) != 0)
            return failure(r, "os_build_unavailable", nil);
        m->os_build[sizeof(m->os_build) - 1] = 0;
        struct utsname u;
        if (uname(&u) != 0) return failure(r, "architecture_unavailable", nil);
        text(m->architecture, sizeof(m->architecture), u.machine);
        text(m->clang_version, sizeof(m->clang_version), __clang_version__);
        m->sdk_max = __MAC_OS_X_VERSION_MAX_ALLOWED;
        NSArray<id<MTLDevice>> *all = MTLCopyAllDevices();
        m->device_count = (uint32_t)all.count;
        id<MTLDevice> selected = nil;
        for (id<MTLDevice> d in all) {
            if (requested_registry ? (d.registryID == requested_registry) : isParavirtual(d)) {
                selected = d; m->selected_by = requested_registry ? 1 : 2; break;
            }
        }
        if (!selected && !requested_registry) { selected = MTLCreateSystemDefaultDevice(); m->selected_by = 3; }
        if (!selected) return failure(r, requested_registry ? "requested_device_unavailable" : "no_metal_device", nil);
        m->registry_id = selected.registryID;
        m->paravirtual_name_match = isParavirtual(selected) ? 1 : 0;
        m->unified_memory = selected.hasUnifiedMemory;
        m->low_power = selected.lowPower;
        m->removable = selected.removable;
        if (!selected.name.UTF8String || strlen(selected.name.UTF8String) >= sizeof(m->device_name))
            return failure(r, "device_name_missing_or_unrepresentable", nil);
        text(m->device_name, sizeof(m->device_name), selected.name.UTF8String);
        GPContext *c = [GPContext new];
        c->device = selected;
        c->queue = [selected newCommandQueue];
        if (!c->queue) return failure(r, "command_queue_unavailable", nil);
        NSError *error = nil;
        MTLCompileOptions *options = [MTLCompileOptions new];
        options.languageVersion = MTLLanguageVersion2_4;
        options.fastMathEnabled = NO;
        c->library = [selected newLibraryWithSource:[NSString stringWithUTF8String:generic_msl]
                                            options:options error:&error];
        if (!c->library || error) return failure(r, "authored_msl_compile_failed", error);
        c->query = pipeline(c, 0, YES, NO, r);
        if (!c->query) return 1;
        c->attached = pipeline(c, 0, NO, NO, &m->mode_errors[0]);
        c->stencilOnly = pipeline(c, 1, NO, NO, &m->mode_errors[1]);
        m->available_modes = (c->attached ? 1u : 0u) | (c->stencilOnly ? 2u : 0u);
        if (!m->available_modes) return failure(r, "no_supported_attachment_mode", nil);
        c->clipAttached = pipeline(c, 0, NO, YES, r);
        if (!c->clipAttached) return failure(r, "direct_clip_attached_pipeline_unavailable", nil);
        c->clipStencilOnly = pipeline(c, 1, NO, YES, r);
        if (!c->clipStencilOnly) return failure(r, "direct_clip_stencil_pipeline_unavailable", nil);
        c->replace = stencilState(c, NO, NO);
        c->keep = stencilState(c, NO, YES);
        c->equal = stencilState(c, YES, YES);
        if (!c->replace || !c->keep || !c->equal) return failure(r, "depth_stencil_state_unavailable", nil);
        *handle = (__bridge_retained void *)c;
        text(r->reason, sizeof(r->reason), "initialized_not_measured");
        return 0;
    } @catch (NSException *exception) {
        (void)exception;
        return failure(r, "native_initialization_exception", nil);
    } }
}

static MTLRenderPassDescriptor *pass(id<MTLTexture> stencil, id<MTLTexture> a,
                                    id<MTLTexture> b, BOOL clear, uint32_t value) {
    MTLRenderPassDescriptor *p = [MTLRenderPassDescriptor renderPassDescriptor];
    p.renderTargetWidth = GP_SIDE; p.renderTargetHeight = GP_SIDE;
    p.defaultRasterSampleCount = 1;
    p.stencilAttachment.texture = stencil;
    p.stencilAttachment.loadAction = clear ? MTLLoadActionClear : MTLLoadActionLoad;
    p.stencilAttachment.storeAction = MTLStoreActionStore;
    p.stencilAttachment.clearStencil = value;
    id<MTLTexture> colors[2] = {a, b};
    for (NSUInteger i = 0; i < 2; ++i) if (colors[i]) {
        p.colorAttachments[i].texture = colors[i];
        p.colorAttachments[i].loadAction = clear ? MTLLoadActionClear : MTLLoadActionLoad;
        p.colorAttachments[i].storeAction = MTLStoreActionStore;
        p.colorAttachments[i].clearColor = i == 0 ? MTLClearColorMake(0.25, 0.5, 0.75, 1.0)
                                                : MTLClearColorMake(0.5, 0.25, 0.0, 1.0);
    }
    return p;
}
static BOOL copyTexture(id<MTLCommandBuffer> cb, id<MTLTexture> t, id<MTLBuffer> b, NSUInteger slab) {
    id<MTLBlitCommandEncoder> e = [cb blitCommandEncoder];
    if (!e) return NO;
    [e copyFromTexture:t sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0, 0, 0)
           sourceSize:MTLSizeMake(GP_SIDE, GP_SIDE, 1) toBuffer:b
    destinationOffset:slab * GP_SLAB + GP_GUARD destinationBytesPerRow:GP_ROW
    destinationBytesPerImage:GP_ROW * GP_SIDE];
    [e endEncoding];
    return YES;
}
static int complete(id<MTLCommandBuffer> cb, GPReceipt *r) {
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    [cb addCompletedHandler:^(id<MTLCommandBuffer> finished) {
        (void)finished;
        dispatch_semaphore_signal(done);
    }];
    [cb commit]; r->submitted = 1;
    if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) != 0) {
        r->timed_out = 1; r->status = (uint32_t)cb.status;
        return failure(r, "command_buffer_timeout", cb.error);
    }
    r->completion_seen = 1;
    r->status = (uint32_t)cb.status;
    r->gpu_start = cb.GPUStartTime; r->gpu_end = cb.GPUEndTime;
    if (cb.status != MTLCommandBufferStatusCompleted || cb.error)
        return failure(r, "command_buffer_not_successfully_completed", cb.error);
    text(r->reason, sizeof(r->reason), "completed");
    return 0;
}
static BOOL valid(const GPCase *p) {
    if (p->indexed > 1 || p->keep > 1 || p->primitive > 1 || p->no_draw > 1 || p->position_mode > 1 ||
        p->clear_stencil > 1 || p->instances < 1 || p->instances > 3 || p->draw_count > 8) return NO;
    if (p->position_mode) {
        if (p->instances != 1) return NO;
        for (uint32_t i = 0; i < 3; ++i) for (uint32_t j = 0; j < 2; ++j)
            if (p->offsets[i][j] != 0.0f) return NO;
    }
    if (!p->no_draw && (p->draw_count == 0 || p->draw_count % (p->primitive ? 3 : 2))) return NO;
    if (p->vertex_start > 8 || p->index_first > 10) return NO;
    if (p->indexed) {
        if (p->index_first + p->draw_count > 10) return NO;
        for (uint32_t i = 0; i < p->draw_count; ++i) if (p->indices[p->index_first + i] >= 8) return NO;
    } else if (p->vertex_start + p->draw_count > 8) return NO;
    for (uint32_t i = 0; i < 8; ++i) for (uint32_t j = 0; j < 2; ++j)
        if (!isfinite(p->vertices[i][j]) || fabsf(p->vertices[i][j]) > 128) return NO;
    for (uint32_t i = 0; i < 3; ++i) for (uint32_t j = 0; j < 2; ++j)
        if (!isfinite(p->offsets[i][j]) || fabsf(p->offsets[i][j]) > 32) return NO;
    for (uint32_t i = 0; i < 4; ++i) if (!isfinite(p->viewport[i])) return NO;
    if (p->viewport[0] < 0 || p->viewport[1] < 0 || p->viewport[2] <= 0 || p->viewport[3] <= 0 ||
        p->viewport[0] > GP_SIDE || p->viewport[1] > GP_SIDE ||
        p->viewport[2] > 512 || p->viewport[3] > 512) return NO;
    if (p->scissor[0] >= GP_SIDE || p->scissor[1] >= GP_SIDE ||
        !p->scissor[2] || !p->scissor[3] || p->scissor[2] > GP_SIDE || p->scissor[3] > GP_SIDE ||
        p->scissor[0] + p->scissor[2] > GP_SIDE || p->scissor[1] + p->scissor[3] > GP_SIDE) return NO;
    return YES;
}

int gp_run(void *handle, const GPCase *p, uint32_t mode, uint8_t *out, size_t capacity, GPReceipt *r) {
    if (!r) return 1;
    memset(r, 0, sizeof(*r)); r->mode = mode;
    if (!handle || !p || !out || mode > 1 || !valid(p)) return failure(r, "invalid_bounded_input", nil);
    const NSUInteger size = (mode == 0 ? 6 : 2) * GP_SLAB;
    if (capacity < size) return failure(r, "output_capacity", nil);
    @autoreleasepool { @try {
        GPContext *c = (__bridge GPContext *)handle;
        id<MTLRenderPipelineState> pso = p->position_mode
            ? (mode == 0 ? c->clipAttached : c->clipStencilOnly)
            : (mode == 0 ? c->attached : c->stencilOnly);
        if (!pso) return failure(r, "attachment_mode_unsupported", nil);
        id<MTLTexture> s = texture(c, MTLPixelFormatStencil8);
        id<MTLTexture> q = texture(c, MTLPixelFormatRGBA8Unorm);
        id<MTLTexture> a = mode == 0 ? texture(c, MTLPixelFormatRGBA16Float) : nil;
        id<MTLTexture> b = mode == 0 ? texture(c, MTLPixelFormatRG16Float) : nil;
        if (!s || !q || (mode == 0 && (!a || !b))) return failure(r, "texture_or_format_unavailable", nil);
        if (s.pixelFormat != MTLPixelFormatStencil8 || s.sampleCount != 1 || q.sampleCount != 1 ||
            s.storageMode != MTLStorageModePrivate || q.storageMode != MTLStorageModePrivate)
            return failure(r, "texture_configuration_mismatch", nil);
        id<MTLBuffer> rb = [c->device newBufferWithLength:size options:MTLResourceStorageModeShared];
        id<MTLBuffer> points = [c->device newBufferWithBytes:p->vertices length:sizeof(p->vertices)
                                                   options:MTLResourceStorageModeShared];
        id<MTLBuffer> indices = p->indexed ? [c->device newBufferWithBytes:p->indices length:sizeof(p->indices)
                                                                   options:MTLResourceStorageModeShared] : nil;
        if (!rb || !rb.contents || !points || (p->indexed && !indices) || rb.storageMode != MTLStorageModeShared)
            return failure(r, "shared_buffer_unavailable", nil);
        memset(rb.contents, 0xa5, size);
        id<MTLCommandBuffer> cb = [c->queue commandBuffer];
        if (!cb) return failure(r, "command_buffer_unavailable", nil);
        cb.label = [NSString stringWithFormat:@"generic-line-%u-mode-%u", p->id, mode];

        // Establish fresh, known nonzero color sentinels and stencil 0 (or query calibration 1).
        id<MTLRenderCommandEncoder> e = [cb renderCommandEncoderWithDescriptor:pass(s, a, b, YES, p->clear_stencil)];
        if (!e) return failure(r, "clear_encoder_unavailable", nil);
        [e endEncoding];
        if (mode == 0 && (!copyTexture(cb, a, rb, 2) || !copyTexture(cb, b, rb, 3)))
            return failure(r, "baseline_blit_unavailable", nil);

        e = [cb renderCommandEncoderWithDescriptor:pass(s, a, b, NO, 0)];
        if (!e) return failure(r, "line_encoder_unavailable", nil);
        [e setRenderPipelineState:pso];
        [e setDepthStencilState:p->keep ? c->keep : c->replace];
        [e setStencilFrontReferenceValue:1 backReferenceValue:1];
        [e setViewport:(MTLViewport){p->viewport[0], p->viewport[1], p->viewport[2], p->viewport[3], 0.0, 1.0}];
        [e setScissorRect:(MTLScissorRect){p->scissor[0], p->scissor[1], p->scissor[2], p->scissor[3]}];
        [e setCullMode:MTLCullModeNone]; [e setDepthClipMode:MTLDepthClipModeClip];
        [e setFrontFacingWinding:MTLWindingCounterClockwise];
        [e setTriangleFillMode:MTLTriangleFillModeFill];
        [e setVertexBuffer:points offset:0 atIndex:0];
        [e setVertexBytes:p->viewport length:sizeof(p->viewport) atIndex:1];
        [e setVertexBytes:p->offsets length:sizeof(p->offsets) atIndex:2];
        if (!p->no_draw) {
            MTLPrimitiveType type = p->primitive ? MTLPrimitiveTypeTriangle : MTLPrimitiveTypeLine;
            if (p->indexed) [e drawIndexedPrimitives:type indexCount:p->draw_count indexType:MTLIndexTypeUInt16
                indexBuffer:indices indexBufferOffset:p->index_first * sizeof(uint16_t) instanceCount:p->instances];
            else [e drawPrimitives:type vertexStart:p->vertex_start vertexCount:p->draw_count instanceCount:p->instances];
        }
        [e endEncoding];
        if (mode == 0 && (!copyTexture(cb, a, rb, 4) || !copyTexture(cb, b, rb, 5)))
            return failure(r, "post_draw_blit_unavailable", nil);

        // Equal(1) is the requested mask; Equal(0) independently checks its complement.
        for (uint32_t i = 0; i < 2; ++i) {
            MTLRenderPassDescriptor *qp = pass(s, nil, nil, NO, 0);
            qp.colorAttachments[0].texture = q;
            qp.colorAttachments[0].loadAction = MTLLoadActionClear;
            qp.colorAttachments[0].storeAction = MTLStoreActionStore;
            qp.colorAttachments[0].clearColor = MTLClearColorMake(0, 0, 0, 0);
            e = [cb renderCommandEncoderWithDescriptor:qp];
            if (!e) return failure(r, "query_encoder_unavailable", nil);
            [e setRenderPipelineState:c->query]; [e setDepthStencilState:c->equal];
            [e setStencilFrontReferenceValue:(i == 0 ? 1 : 0) backReferenceValue:(i == 0 ? 1 : 0)];
            [e setViewport:(MTLViewport){0, 0, GP_SIDE, GP_SIDE, 0, 1}];
            [e setScissorRect:(MTLScissorRect){0, 0, GP_SIDE, GP_SIDE}];
            [e setCullMode:MTLCullModeNone];
            [e drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
            [e endEncoding];
            if (!copyTexture(cb, q, rb, i)) return failure(r, "query_blit_unavailable", nil);
        }
        if (complete(cb, r)) return 1;
        // Shared memory is accessed ONLY after the completion handler and status/error checks.
        memcpy(out, rb.contents, size); r->bytes = (uint32_t)size;
        return 0;
    } @catch (NSException *exception) {
        (void)exception;
        return failure(r, "native_measurement_exception", nil);
    } }
}
void gp_close(void *handle) {
    if (handle) { @autoreleasepool { GPContext *c = (__bridge_transfer GPContext *)handle; (void)c; } }
}
