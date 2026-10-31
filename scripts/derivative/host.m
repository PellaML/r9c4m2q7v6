#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

enum { WIDTH = 128, HEIGHT = 64, CASES = 2048, PASSES = 6, PIXELS = WIDTH * HEIGHT };
typedef struct { uint32_t f[8]; uint16_t scalar[4], vector[8]; uint32_t reserved[2]; } QuadInput;
typedef struct { uint32_t width, height, mode, reserved; } Params;
typedef struct {
    uint32_t stamp, f_input[2], f_dx[2], f_dy[2], f_width[2];
    uint16_t h_input, h_dx, h_dy, h_width;
    uint16_t hv_input[2], hv_dx[2], hv_dy[2], hv_width[2];
    uint32_t reserved;
} Trace;
_Static_assert(sizeof(QuadInput) == 64 && sizeof(Params) == 16, "input layout");
_Static_assert(sizeof(Trace) == 64 && offsetof(Trace, h_input) == 36, "trace layout");
_Static_assert(offsetof(Trace, reserved) == 60, "trace tail");

static int finish(NSMutableDictionary *m, NSString *out, NSString *phase, NSError *error, int status) {
    m[@"phase"] = phase;
    // No paths, usernames, environment or localized error text are serialized.
    m[@"error"] = error ? @{ @"domain": error.domain, @"code": @(error.code) } : (id)[NSNull null];
    NSData *data = [NSJSONSerialization dataWithJSONObject:m options:NSJSONWritingSortedKeys error:NULL];
    if (!data || ![data writeToFile:[out stringByAppendingPathComponent:@"native.json"] atomically:YES]) return 3;
    return status;
}
static BOOL save(NSString *out, NSString *name, const void *bytes, NSUInteger length) {
    return [[NSData dataWithBytes:bytes length:length]
        writeToFile:[out stringByAppendingPathComponent:name] atomically:YES];
}

int main(int argc, const char *argv[]) {
    if (argc != 4) return 64;
    @autoreleasepool {
        NSString *libraryPath = [NSString stringWithUTF8String:argv[1]];
        NSString *inputPath = [NSString stringWithUTF8String:argv[2]];
        NSString *out = [NSString stringWithUTF8String:argv[3]];
        NSMutableDictionary *m = [NSMutableDictionary dictionary];
        m[@"api"] = @"Metal render fragment stage";
        m[@"availability"] = @"not_executed";
        m[@"os_version_and_build"] = NSProcessInfo.processInfo.operatingSystemVersionString;
        @try {
            NSData *data = [NSData dataWithContentsOfFile:inputPath];
            if (!data || data.length != CASES * sizeof(QuadInput))
                return finish(m, out, @"input_layout", nil, 3);
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            if (!device) return finish(m, out, @"no_metal_device", nil, 2);
            m[@"device_name"] = device.name ?: @"unknown";
            m[@"unified_memory"] = @(device.hasUnifiedMemory);
            if (!device.hasUnifiedMemory) return finish(m, out, @"requires_unified_memory", nil, 2);
            NSError *error = nil;
            id<MTLLibrary> library = [device newLibraryWithURL:[NSURL fileURLWithPath:libraryPath] error:&error];
            if (!library) return finish(m, out, @"load_metallib", error, 3);
            id<MTLFunction> vertex = [library newFunctionWithName:@"quad_vertex"];
            id<MTLFunction> fragment = [library newFunctionWithName:@"derivative_probe"];
            if (!vertex || !fragment) return finish(m, out, @"missing_entry", nil, 3);
            MTLRenderPipelineDescriptor *desc = [MTLRenderPipelineDescriptor new];
            desc.vertexFunction = vertex;
            desc.fragmentFunction = fragment;
            desc.rasterSampleCount = 1;
            desc.alphaToCoverageEnabled = NO;
            desc.alphaToOneEnabled = NO;
            desc.colorAttachments[0].pixelFormat = MTLPixelFormatRGBA32Float;
            desc.colorAttachments[0].blendingEnabled = NO;
            desc.colorAttachments[0].writeMask = MTLColorWriteMaskAll;
            id<MTLRenderPipelineState> pipeline = [device newRenderPipelineStateWithDescriptor:desc error:&error];
            if (!pipeline) return finish(m, out, @"render_pipeline", error, 3);
            id<MTLCommandQueue> queue = [device newCommandQueue];
            id<MTLBuffer> input = [device newBufferWithBytes:data.bytes length:data.length options:MTLResourceStorageModeShared];
            id<MTLBuffer> coverage = [device newBufferWithLength:PASSES * PIXELS * sizeof(uint32_t) options:MTLResourceStorageModeShared];
            id<MTLBuffer> trace = [device newBufferWithLength:PIXELS * sizeof(Trace) options:MTLResourceStorageModeShared];
            if (!queue || !input || !coverage || !trace) return finish(m, out, @"buffer_allocation", nil, 3);
            memset(coverage.contents, 0xa5, coverage.length);
            memset(trace.contents, 0xa5, trace.length);
            NSMutableArray<id<MTLTexture>> *targets = [NSMutableArray arrayWithCapacity:PASSES];
            id<MTLCommandBuffer> command = [queue commandBuffer];
            if (!command) return finish(m, out, @"command_buffer", nil, 3);
            for (uint32_t mode = 0; mode < PASSES; ++mode) {
                MTLTextureDescriptor *td = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA32Float
                    width:WIDTH height:HEIGHT mipmapped:NO];
                td.storageMode = MTLStorageModeShared;
                td.usage = MTLTextureUsageRenderTarget;
                id<MTLTexture> target = [device newTextureWithDescriptor:td];
                if (!target) return finish(m, out, @"texture_allocation", nil, 3);
                [targets addObject:target];
                MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
                pass.colorAttachments[0].texture = target;
                pass.colorAttachments[0].loadAction = MTLLoadActionClear;
                pass.colorAttachments[0].storeAction = MTLStoreActionStore;
                pass.colorAttachments[0].clearColor = MTLClearColorMake(-1234.5, -1234.5, -1234.5, -1234.5);
                id<MTLRenderCommandEncoder> encoder = [command renderCommandEncoderWithDescriptor:pass];
                if (!encoder) return finish(m, out, @"render_encoder", nil, 3);
                [encoder setRenderPipelineState:pipeline];
                [encoder setCullMode:MTLCullModeNone];
                [encoder setViewport:(MTLViewport){0, 0, WIDTH, HEIGHT, 0, 1}];
                [encoder setScissorRect:(MTLScissorRect){0, 0, WIDTH, HEIGHT}];
                [encoder setFragmentBuffer:input offset:0 atIndex:0];
                Params params = {WIDTH, HEIGHT, mode, 0};
                [encoder setFragmentBytes:&params length:sizeof(params) atIndex:1];
                [encoder setFragmentBuffer:coverage offset:0 atIndex:2];
                [encoder setFragmentBuffer:trace offset:0 atIndex:3];
                [encoder drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
                [encoder endEncoding];
            }
            [command commit];
            [command waitUntilCompleted];
            m[@"pipeline_created"] = @YES;
            m[@"wait_until_completed_called"] = @YES;
            m[@"command_status"] = @(command.status);
            m[@"render_passes"] = @(PASSES);
            m[@"draw_calls"] = @(PASSES);
            m[@"raster_sample_count"] = @1;
            m[@"primitive"] = @"one fullscreen triangle per pass";
            m[@"width"] = @(WIDTH); m[@"height"] = @(HEIGHT);
            m[@"pixel_format"] = @"RGBA32Float";
            if (command.status != MTLCommandBufferStatusCompleted || command.error)
                return finish(m, out, @"command_completion", command.error, 3);
            const NSUInteger textureBytes = PIXELS * 4 * sizeof(uint32_t);
            NSMutableData *pixels = [NSMutableData dataWithLength:PASSES * textureBytes];
            for (NSUInteger i = 0; i < PASSES; ++i) {
                [targets[i] getBytes:(uint8_t *)pixels.mutableBytes + i * textureBytes
                    bytesPerRow:WIDTH * 4 * sizeof(uint32_t) fromRegion:MTLRegionMake2D(0, 0, WIDTH, HEIGHT)
                    mipmapLevel:0];
            }
            if (![pixels writeToFile:[out stringByAppendingPathComponent:@"textures.bin"] atomically:YES]
                || !save(out, @"trace.bin", trace.contents, trace.length)
                || !save(out, @"coverage.bin", coverage.contents, coverage.length))
                return finish(m, out, @"readback_write", nil, 3);
            m[@"readback_copied_after_successful_wait"] = @YES;
            m[@"availability"] = @"available_and_executed";
            return finish(m, out, @"completed", nil, 0);
        } @catch (NSException *exception) {
            (void)exception;
            return finish(m, out, @"objective_c_exception", nil, 3);
        }
    }
}
