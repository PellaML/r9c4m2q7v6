#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <float.h>
#include <string.h>
#include "metal_bridge.h"

_Static_assert(sizeof(ProbeConfig) == 32, "config ABI");
_Static_assert(sizeof(ProbeSample) == 32, "sample ABI");
_Static_assert(offsetof(ProbeSample, stamp) == 16, "stamp ABI");
_Static_assert(sizeof(ProbeRunInfo) == 24, "run-info ABI");

typedef struct {
    uint32_t count, width, sampler_index, token;
    float bias;
    uint32_t reserved[3];
} ShaderParameters;
_Static_assert(sizeof(ShaderParameters) == 32, "shader parameter ABI");

@interface ProbeContext : NSObject
@property(nonatomic, strong) id<MTLDevice> device;
@property(nonatomic, strong) id<MTLCommandQueue> queue;
@property(nonatomic, strong) id<MTLTexture> texture;
@property(nonatomic, strong) NSMutableDictionary<NSString *, id<MTLLibrary>> *libraries;
@property(nonatomic, strong) NSMutableDictionary<NSString *, id<MTLRenderPipelineState>> *pipelines;
@property(nonatomic, strong) NSMutableDictionary<NSNumber *, id<MTLSamplerState>> *samplers;
@end
@implementation ProbeContext
@end

static void emit_json(id object, char *destination, size_t capacity) {
    NSData *data = [NSJSONSerialization dataWithJSONObject:object options:NSJSONWritingSortedKeys error:NULL];
    const char *fallback = "{\"phase\":\"metadata_serialization_failed\"}";
    if (capacity == 0) return;
    if (!data || data.length >= capacity) {
        size_t length = MIN(strlen(fallback), capacity - 1);
        memcpy(destination, fallback, length);
        destination[length] = 0;
        return;
    }
    memcpy(destination, data.bytes, data.length);
    destination[data.length] = 0;
}

static void failure(NSString *phase, NSError *error, char *json, size_t capacity) {
    // Do not serialize paths, localized diagnostics, usernames, or environment data.
    emit_json(@{@"phase": phase, @"error": error ? @{
        @"domain": error.domain, @"code": @(error.code)
    } : (id)[NSNull null]}, json, capacity);
}

static BOOL texture_valid(id<MTLTexture> texture) {
    return texture && texture.textureType == MTLTextureType3D &&
        texture.pixelFormat == MTLPixelFormatBGRA8Unorm &&
        texture.width == 1 && texture.height == 1 && texture.depth == 1 &&
        texture.mipmapLevelCount == 1 && texture.sampleCount == 1 && texture.arrayLength == 1;
}

int32_t probe_open(void **context, char *json, size_t capacity) {
    *context = NULL;
    @autoreleasepool {
        @try {
            NSOperatingSystemVersion os = NSProcessInfo.processInfo.operatingSystemVersion;
            NSMutableDictionary *metadata = [@{
                @"api": @"Metal",
                @"os": @{@"major": @(os.majorVersion), @"minor": @(os.minorVersion),
                         @"patch": @(os.patchVersion),
                         @"version_and_build": NSProcessInfo.processInfo.operatingSystemVersionString}
            } mutableCopy];
            ProbeContext *p = [ProbeContext new];
            p.device = MTLCreateSystemDefaultDevice();
            if (!p.device) {
                metadata[@"availability"] = @"unavailable";
                metadata[@"reason"] = @"no_metal_device";
                emit_json(metadata, json, capacity);
                return 2;
            }
            metadata[@"device"] = @{
                @"name": p.device.name ?: @"unknown",
                @"has_unified_memory": @(p.device.hasUnifiedMemory),
                @"low_power": @(p.device.lowPower), @"headless": @(p.device.headless),
                @"removable": @(p.device.removable)
            };
            MTLTextureDescriptor *descriptor = [MTLTextureDescriptor new];
            descriptor.textureType = MTLTextureType3D;
            descriptor.pixelFormat = MTLPixelFormatBGRA8Unorm;
            descriptor.width = 1;
            descriptor.height = 1;
            descriptor.depth = 1;
            descriptor.mipmapLevelCount = 1;
            descriptor.arrayLength = 1;
            descriptor.sampleCount = 1;
            descriptor.usage = MTLTextureUsageShaderRead;
            descriptor.storageMode = MTLStorageModePrivate;
            p.texture = [p.device newTextureWithDescriptor:descriptor];
            if (!texture_valid(p.texture)) {
                metadata[@"availability"] = @"unavailable";
                metadata[@"reason"] = @"requested_3d_bgra8_texture_unavailable";
                emit_json(metadata, json, capacity);
                return 2;
            }
            metadata[@"actual_texture"] = @{
                @"type": @"MTLTextureType3D", @"type_value": @(p.texture.textureType),
                @"format": @"BGRA8Unorm", @"format_value": @(p.texture.pixelFormat),
                @"width": @(p.texture.width), @"height": @(p.texture.height),
                @"depth": @(p.texture.depth), @"mip_levels": @(p.texture.mipmapLevelCount),
                @"sample_count": @(p.texture.sampleCount), @"array_length": @(p.texture.arrayLength),
                @"storage_mode_value": @(p.texture.storageMode)
            };
            p.queue = [p.device newCommandQueue];
            if (!p.queue) {
                metadata[@"availability"] = @"failed";
                metadata[@"reason"] = @"command_queue_creation_failed";
                emit_json(metadata, json, capacity);
                return 1;
            }
            p.libraries = [NSMutableDictionary dictionary];
            p.pipelines = [NSMutableDictionary dictionary];
            p.samplers = [NSMutableDictionary dictionary];
            metadata[@"availability"] = @"resources_created_not_yet_executed";
            emit_json(metadata, json, capacity);
            *context = (__bridge_retained void *)p;
            return 0;
        } @catch (NSException *exception) {
            emit_json(@{@"phase": @"open", @"exception_name": exception.name}, json, capacity);
            return 1;
        }
    }
}

int32_t probe_sample(void *context, const char *library_path, const char *function_name,
                     const uint32_t *coordinates, const ProbeConfig *config,
                     ProbeSample *samples, ProbeRunInfo *info, char *json, size_t capacity) {
    memset(info, 0, sizeof(*info));
    @autoreleasepool {
        @try {
            ProbeContext *p = (__bridge ProbeContext *)context;
            if (!p || !config || config->count == 0 || config->count > 65536 ||
                config->zero_axes > 7 || config->linear > 1 || config->pixel_coords > 1 ||
                config->use_constexpr > 1) {
                failure(@"invalid_arguments", nil, json, capacity);
                return 1;
            }
            info->texture_verified = texture_valid(p.texture);
            if (!info->texture_verified) {
                failure(@"texture_descriptor_mismatch", nil, json, capacity);
                return 1;
            }
            NSString *path = [NSString stringWithUTF8String:library_path];
            NSString *entry = [NSString stringWithUTF8String:function_name];
            NSError *error = nil;
            id<MTLLibrary> library = p.libraries[path];
            if (!library) {
                library = [p.device newLibraryWithURL:[NSURL fileURLWithPath:path] error:&error];
                if (!library) { failure(@"load_metallib", error, json, capacity); return 1; }
                p.libraries[path] = library;
            }
            NSString *pipelineKey = [path stringByAppendingFormat:@":%@", entry];
            id<MTLRenderPipelineState> pipeline = p.pipelines[pipelineKey];
            if (!pipeline) {
                MTLRenderPipelineDescriptor *descriptor = [MTLRenderPipelineDescriptor new];
                descriptor.vertexFunction = [library newFunctionWithName:@"probe_vertex"];
                descriptor.fragmentFunction = [library newFunctionWithName:entry];
                descriptor.colorAttachments[0].pixelFormat = MTLPixelFormatRGBA32Float;
                descriptor.colorAttachments[0].blendingEnabled = NO;
                descriptor.rasterSampleCount = 1;
                if (!descriptor.vertexFunction || !descriptor.fragmentFunction) {
                    failure(@"entry_point_missing", nil, json, capacity); return 1;
                }
                pipeline = [p.device newRenderPipelineStateWithDescriptor:descriptor error:&error];
                if (!pipeline) { failure(@"render_pipeline", error, json, capacity); return 1; }
                p.pipelines[pipelineKey] = pipeline;
            }
            id<MTLSamplerState> sampler = nil;
            if (!config->use_constexpr) {
                NSNumber *key = @(config->zero_axes | (config->linear << 3) | (config->pixel_coords << 4));
                sampler = p.samplers[key];
                if (!sampler) {
                    MTLSamplerDescriptor *descriptor = [MTLSamplerDescriptor new];
                    descriptor.normalizedCoordinates = !config->pixel_coords;
                    descriptor.minFilter = config->linear ? MTLSamplerMinMagFilterLinear : MTLSamplerMinMagFilterNearest;
                    descriptor.magFilter = descriptor.minFilter;
                    descriptor.mipFilter = MTLSamplerMipFilterNotMipmapped;
                    descriptor.sAddressMode = (config->zero_axes & 1) ? MTLSamplerAddressModeClampToZero : MTLSamplerAddressModeClampToEdge;
                    descriptor.tAddressMode = (config->zero_axes & 2) ? MTLSamplerAddressModeClampToZero : MTLSamplerAddressModeClampToEdge;
                    descriptor.rAddressMode = (config->zero_axes & 4) ? MTLSamplerAddressModeClampToZero : MTLSamplerAddressModeClampToEdge;
                    descriptor.maxAnisotropy = 1;
                    descriptor.lodMinClamp = 0.0f;
                    descriptor.lodMaxClamp = FLT_MAX;
                    sampler = [p.device newSamplerStateWithDescriptor:descriptor];
                    if (!sampler) { failure(@"runtime_sampler_unavailable", nil, json, capacity); return 2; }
                    p.samplers[key] = sampler;
                }
            }

            const uint32_t width = 256;
            const uint32_t height = (config->count + width - 1) / width;
            const size_t byteCount = (size_t)config->count * sizeof(ProbeSample);
            id<MTLBuffer> input = [p.device newBufferWithBytes:coordinates length:(size_t)config->count * 12 options:MTLResourceStorageModeShared];
            id<MTLBuffer> output = [p.device newBufferWithBytes:samples length:byteCount options:MTLResourceStorageModeShared];
            id<MTLBuffer> upload = [p.device newBufferWithLength:256 options:MTLResourceStorageModeShared];
            MTLTextureDescriptor *targetDescriptor = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA32Float width:width height:height mipmapped:NO];
            targetDescriptor.usage = MTLTextureUsageRenderTarget;
            targetDescriptor.storageMode = MTLStorageModePrivate;
            id<MTLTexture> target = [p.device newTextureWithDescriptor:targetDescriptor];
            if (!input || !output || !upload || !target) {
                failure(@"working_resource_allocation", nil, json, capacity); return 1;
            }
            // The CPU only uploads BGRA bytes and poisoned result storage. It never filters.
            uint8_t *bytes = upload.contents;
            memset(bytes, 0, 256);
            bytes[0] = config->rgba[2]; bytes[1] = config->rgba[1];
            bytes[2] = config->rgba[0]; bytes[3] = config->rgba[3];

            id<MTLCommandBuffer> command = [p.queue commandBuffer];
            id<MTLBlitCommandEncoder> blit = [command blitCommandEncoder];
            if (!command || !blit) { failure(@"command_or_blit_encoder", nil, json, capacity); return 1; }
            [blit copyFromBuffer:upload sourceOffset:0 sourceBytesPerRow:256 sourceBytesPerImage:256
                     sourceSize:MTLSizeMake(1, 1, 1) toTexture:p.texture destinationSlice:0
               destinationLevel:0 destinationOrigin:MTLOriginMake(0, 0, 0)];
            [blit endEncoding];
            info->upload_encoded = 1;

            MTLRenderPassDescriptor *pass = [MTLRenderPassDescriptor renderPassDescriptor];
            pass.colorAttachments[0].texture = target;
            pass.colorAttachments[0].loadAction = MTLLoadActionClear;
            pass.colorAttachments[0].storeAction = MTLStoreActionStore;
            pass.colorAttachments[0].clearColor = MTLClearColorMake(0, 0, 0, 0);
            id<MTLRenderCommandEncoder> encoder = [command renderCommandEncoderWithDescriptor:pass];
            if (!encoder) { failure(@"render_encoder", nil, json, capacity); return 1; }
            ShaderParameters parameters = {config->count, width,
                config->zero_axes | (config->linear << 3), config->token, config->bias, {0, 0, 0}};
            [encoder setRenderPipelineState:pipeline];
            [encoder setCullMode:MTLCullModeNone];
            [encoder setViewport:(MTLViewport){0, 0, width, height, 0, 1}];
            [encoder setFragmentTexture:p.texture atIndex:0];
            if (sampler) [encoder setFragmentSamplerState:sampler atIndex:0];
            [encoder setFragmentBuffer:input offset:0 atIndex:0];
            [encoder setFragmentBuffer:output offset:0 atIndex:1];
            [encoder setFragmentBytes:&parameters length:sizeof(parameters) atIndex:2];
            [encoder drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
            [encoder endEncoding];
            info->render_encoded = 1;
            [command commit];
            info->wait_called = 1;
            [command waitUntilCompleted];
            info->command_status = (uint32_t)command.status;
            if (command.status != MTLCommandBufferStatusCompleted || command.error) {
                failure(@"command_completion", command.error, json, capacity); return 1;
            }
            memcpy(samples, output.contents, byteCount);
            info->readback_copied = 1;
            emit_json(@{@"phase": @"completed", @"command_error": [NSNull null]}, json, capacity);
            return 0;
        } @catch (NSException *exception) {
            emit_json(@{@"phase": @"sample", @"exception_name": exception.name}, json, capacity);
            return 1;
        }
    }
}

void probe_close(void *context) {
    if (context) {
        @autoreleasepool { id released = (__bridge_transfer id)context; (void)released; }
    }
}
