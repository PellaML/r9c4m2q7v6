#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <dispatch/dispatch.h>
#include <stdint.h>
#include <string.h>

// The bridge only creates generic resources, executes, and copies completed
// bytes. Corpus construction, all numerical oracles, and comparisons are Rust.
enum { WIDTH = 8, MAX_VERTICES = 12, GUARD_WORDS = 64, OUTPUT_WORDS = 1360 };
typedef struct {
    uint32_t abi, completed, status, no_error, copied, variant, vertices, words;
    uint32_t no_depth_stencil, default_viewport_scissor, samples, input_unchanged;
    uint64_t registry_id, reserved;
} Receipt;
_Static_assert(sizeof(Receipt) == 64, "receipt ABI");
_Static_assert(MTLCommandBufferStatusCompleted == 4, "completed ABI");

static int finish(NSMutableDictionary *log, NSString *phase, NSError *error,
                  int code, char *json, size_t capacity) {
    log[@"phase"] = phase;
    log[@"exit_code"] = @(code);
    log[@"error"] = error ? @{@"domain": error.domain, @"code": @(error.code),
                              @"description": error.localizedDescription ?: @""}
                           : (id)[NSNull null];
    NSData *data = [NSJSONSerialization dataWithJSONObject:log
                         options:NSJSONWritingSortedKeys error:NULL];
    if (!data || data.length >= capacity) return 70;
    memcpy(json, data.bytes, data.length);
    json[data.length] = 0;
    return code;
}

int depth_native(const char *source, const uint32_t *input, uint32_t count,
                 uint32_t variant, uint32_t *output, size_t words,
                 Receipt *receipt, char *json, size_t capacity) {
    if (!receipt || !json || capacity < 2) return 64;
    memset(receipt, 0, sizeof(*receipt));
    json[0] = 0;
    receipt->abi = 1;
    receipt->variant = variant;
    receipt->vertices = count;
    receipt->words = (uint32_t)words;
    @autoreleasepool {
        NSMutableDictionary *log = [@{
            @"schema": @1, @"backend": @"native_metal", @"cpu_fallback": @NO,
            @"variant": @(variant), @"vertex_count": @(count),
            @"completed": @NO, @"copied": @NO,
            @"os_version_and_build": NSProcessInfo.processInfo.operatingSystemVersionString
        } mutableCopy];
        @try {
            if (!source || !input || !output || (count != 3 && count != 12) ||
                variant > 1 || words != OUTPUT_WORDS)
                return finish(log, @"invalid_arguments", nil, 64, json, capacity);
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            if (!device)
                return finish(log, @"no_device_no_fallback", nil, 23, json, capacity);
            receipt->registry_id = device.registryID;
            log[@"device"] = @{@"name": device.name ?: @"unknown",
                @"registry_id": @(device.registryID), @"unified_memory": @(device.hasUnifiedMemory)};

            MTLCompileOptions *options = [MTLCompileOptions new];
            log[@"default_compiler_options"] = @{
                @"fast_math": @(options.fastMathEnabled),
                @"language_version": @(options.languageVersion),
                @"preserve_invariance": @(options.preserveInvariance),
                @"optimization_level": @(options.optimizationLevel)};
            options.fastMathEnabled = NO;
            options.preprocessorMacros = @{@"DEPTH_SHIFT": @(variant)};
            log[@"effective_compiler_options"] = @{
                @"fast_math": @(options.fastMathEnabled),
                @"language_version": @(options.languageVersion),
                @"preserve_invariance": @(options.preserveInvariance),
                @"optimization_level": @(options.optimizationLevel),
                @"DEPTH_SHIFT": @(variant)};
            if (options.fastMathEnabled)
                return finish(log, @"compiler_option_failure", nil, 24, json, capacity);
            NSError *error = nil;
            id<MTLLibrary> library = [device newLibraryWithSource:@(source)
                                           options:options error:&error];
            if (!library || error)
                return finish(log, @"shader_compile_failure", error, 24, json, capacity);
            MTLRenderPipelineDescriptor *pipeline = [MTLRenderPipelineDescriptor new];
            pipeline.vertexFunction = [library newFunctionWithName:@"depth_vertex"];
            pipeline.fragmentFunction = [library newFunctionWithName:@"depth_fragment"];
            pipeline.rasterSampleCount = 1;
            pipeline.colorAttachments[0].pixelFormat = MTLPixelFormatRGBA32Float;
            pipeline.colorAttachments[0].blendingEnabled = NO;
            pipeline.colorAttachments[0].writeMask = MTLColorWriteMaskAll;
            pipeline.depthAttachmentPixelFormat = MTLPixelFormatInvalid;
            pipeline.stencilAttachmentPixelFormat = MTLPixelFormatInvalid;
            if (!pipeline.vertexFunction || !pipeline.fragmentFunction)
                return finish(log, @"missing_entry_point", nil, 24, json, capacity);
            id<MTLRenderPipelineState> state = [device newRenderPipelineStateWithDescriptor:pipeline error:&error];
            if (!state || error)
                return finish(log, @"pipeline_failure", error, 25, json, capacity);

            MTLTextureDescriptor *td = [MTLTextureDescriptor new];
            td.textureType = MTLTextureType2D;
            td.pixelFormat = MTLPixelFormatRGBA32Float;
            td.width = WIDTH; td.height = WIDTH; td.depth = 1;
            td.mipmapLevelCount = 1; td.arrayLength = 1; td.sampleCount = 1;
            td.storageMode = MTLStorageModePrivate;
            td.usage = MTLTextureUsageRenderTarget;
            id<MTLTexture> sink = [device newTextureWithDescriptor:td];
            id<MTLBuffer> vertices = [device newBufferWithBytes:input length:count * 32
                                        options:MTLResourceStorageModeShared];
            id<MTLBuffer> audit = [device newBufferWithBytes:output length:words * 4
                                     options:MTLResourceStorageModeShared];
            id<MTLCommandQueue> queue = [device newCommandQueue];
            if (!sink || !vertices || !audit || !queue ||
                vertices.storageMode != MTLStorageModeShared || audit.storageMode != MTLStorageModeShared ||
                !vertices.contents || !audit.contents || vertices.length != count * 32 || audit.length != words * 4 ||
                sink.pixelFormat != MTLPixelFormatRGBA32Float || sink.width != WIDTH || sink.height != WIDTH ||
                sink.sampleCount != 1 || sink.mipmapLevelCount != 1 || sink.arrayLength != 1 ||
                sink.storageMode != MTLStorageModePrivate || sink.usage != MTLTextureUsageRenderTarget)
                return finish(log, @"resource_failure", nil, 26, json, capacity);
            MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
            pass.colorAttachments[0].texture = sink;
            pass.colorAttachments[0].loadAction = MTLLoadActionDontCare;
            pass.colorAttachments[0].storeAction = MTLStoreActionDontCare;
            if (pass.depthAttachment.texture || pass.stencilAttachment.texture ||
                pipeline.depthAttachmentPixelFormat != MTLPixelFormatInvalid ||
                pipeline.stencilAttachmentPixelFormat != MTLPixelFormatInvalid)
                return finish(log, @"unexpected_depth_stencil", nil, 26, json, capacity);
            receipt->no_depth_stencil = 1;
            receipt->samples = 1;
            log[@"render_contract"] = @{
                @"width": @(WIDTH), @"height": @(WIDTH), @"samples": @1,
                @"color_format": @(td.pixelFormat), @"color_store": @"dont_care",
                @"depth_attachment": @NO, @"stencil_attachment": @NO,
                @"depth_stencil_state_set": @NO, @"viewport_set": @NO, @"scissor_set": @NO,
                @"default_znear": @0, @"default_zfar": @1,
                @"guard_bytes_each_end": @(GUARD_WORDS * 4), @"output_words": @(words)};
            id<MTLCommandBuffer> command = [queue commandBuffer];
            if (!command)
                return finish(log, @"command_failure", nil, 26, json, capacity);
            id<MTLRenderCommandEncoder> encoder = [command renderCommandEncoderWithDescriptor:pass];
            if (!encoder)
                return finish(log, @"encoder_failure", nil, 26, json, capacity);
            [encoder setRenderPipelineState:state];
            [encoder setCullMode:MTLCullModeNone];
            [encoder setVertexBuffer:vertices offset:0 atIndex:0];
            [encoder setVertexBuffer:audit offset:GUARD_WORDS * 4 atIndex:1];
            [encoder setVertexBytes:&count length:sizeof(count) atIndex:2];
            [encoder setFragmentBuffer:audit offset:GUARD_WORDS * 4 atIndex:1];
            // Intentionally no viewport, scissor, or depth/stencil state setter.
            receipt->default_viewport_scissor = 1;
            [encoder drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:count];
            [encoder endEncoding];
            dispatch_semaphore_t done = dispatch_semaphore_create(0);
            [command addCompletedHandler:^(id<MTLCommandBuffer> completed) {
                (void)completed;
                dispatch_semaphore_signal(done);
            }];
            [command commit];
            if (dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 20 * NSEC_PER_SEC)))
                return finish(log, @"completion_timeout_no_readback", nil, 27, json, capacity);
            // The completion callback already ran; this drains that callback,
            // not an unbounded first wait for GPU completion.
            [command waitUntilCompleted];
            receipt->status = (uint32_t)command.status;
            receipt->completed = command.status == MTLCommandBufferStatusCompleted;
            receipt->no_error = command.error == nil;
            log[@"command_status"] = @(command.status);
            log[@"completed"] = @(receipt->completed);
            if (!receipt->completed || !receipt->no_error)
                return finish(log, @"command_not_successful", command.error, 27, json, capacity);
            receipt->input_unchanged = memcmp(input, vertices.contents, count * 32) == 0;
            memcpy(output, audit.contents, words * 4);
            receipt->copied = 1;
            log[@"copied"] = @YES;
            log[@"input_buffer_unchanged"] = @(receipt->input_unchanged);
            return finish(log, @"completed_readback", nil, 0, json, capacity);
        } @catch (NSException *exception) {
            log[@"exception"] = @{@"name": exception.name, @"reason": exception.reason ?: @""};
            return finish(log, @"exception", nil, 28, json, capacity);
        }
    }
}
