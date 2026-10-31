#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

typedef struct { uint32_t normalized_bits[2], pixel_bits[2], offset_case, tag, reserved[2]; } Input;
typedef struct { uint32_t count, normalized, run, reserved; } Params;
typedef struct { uint32_t stamp, tag, shape, mips, value[4]; } Output;
_Static_assert(sizeof(Input) == 32 && offsetof(Input, tag) == 20, "input layout");
_Static_assert(sizeof(Params) == 16 && sizeof(Output) == 32 && offsetof(Output, value) == 16, "output layout");

static int finish(NSMutableDictionary *m, NSString *out, NSString *phase, NSError *error, int status) {
    m[@"phase"] = phase;
    // Serialize no environment, local paths, usernames or localized error text.
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
    if (argc != 3) return 64;
    @autoreleasepool {
        NSString *libraryPath = [NSString stringWithUTF8String:argv[1]];
        NSString *out = [NSString stringWithUTF8String:argv[2]];
        NSMutableDictionary *m = [NSMutableDictionary dictionary];
        m[@"api"] = @"Metal compute explicit level(0), constant int2 offsets";
        m[@"availability"] = @"not_executed";
        m[@"os_version_and_build"] = NSProcessInfo.processInfo.operatingSystemVersionString;
        @try {
            NSError *error = nil;
            NSData *json = [NSData dataWithContentsOfFile:[out stringByAppendingPathComponent:@"corpus.json"]];
            NSDictionary *corpus = json ? [NSJSONSerialization JSONObjectWithData:json options:0 error:&error] : nil;
            NSData *inputs = [NSData dataWithContentsOfFile:[out stringByAppendingPathComponent:@"inputs.bin"]];
            NSData *pixels = [NSData dataWithContentsOfFile:[out stringByAppendingPathComponent:@"image-data.bin"]];
            if (!corpus || !inputs || !pixels) return finish(m, out, @"inputs", error, 3);
            NSArray *imageCases = corpus[@"images"], *samplerCases = corpus[@"samplers"], *runs = corpus[@"runs"], *geometries = corpus[@"geometries"];
            NSUInteger count = [corpus[@"case_count"] unsignedIntegerValue];
            if (count == 0 || count > 8192 || runs.count != 144 || imageCases.count != 6 || samplerCases.count != 12 || geometries.count != 2
                || inputs.length != geometries.count * count * sizeof(Input)) return finish(m, out, @"input_layout", nil, 3);
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            if (!device) return finish(m, out, @"no_metal_device", nil, 2);
            m[@"device_name"] = device.name ?: @"unknown";
            m[@"unified_memory"] = @(device.hasUnifiedMemory);
            if (!device.hasUnifiedMemory) return finish(m, out, @"requires_unified_memory", nil, 2);
            id<MTLLibrary> library = [device newLibraryWithURL:[NSURL fileURLWithPath:libraryPath] error:&error];
            if (!library) return finish(m, out, @"load_metallib", error, 3);
            NSMutableArray<id<MTLComputePipelineState>> *pipelines = [NSMutableArray array];
            for (NSString *name in @[@"sample_offset_float", @"sample_offset_half"]) {
                id<MTLFunction> function = [library newFunctionWithName:name];
                if (!function) return finish(m, out, @"missing_entry", nil, 3);
                id<MTLComputePipelineState> pipeline = [device newComputePipelineStateWithFunction:function error:&error];
                if (!pipeline) return finish(m, out, @"compute_pipeline", error, 3);
                [pipelines addObject:pipeline];
            }
            m[@"pipeline_created"] = @YES;
            m[@"pipeline_count"] = @(pipelines.count);
            NSMutableArray<id<MTLTexture>> *images = [NSMutableArray array];
            NSMutableData *uploaded = [NSMutableData dataWithLength:pixels.length];
            NSMutableArray *imageMetadata = [NSMutableArray array];
            for (NSDictionary *c in imageCases) {
                NSUInteger width = [c[@"width"] unsignedIntegerValue], height = [c[@"height"] unsignedIntegerValue];
                NSUInteger offset = [c[@"data_offset"] unsignedIntegerValue], length = [c[@"data_bytes"] unsignedIntegerValue], pitch = [c[@"pitch"] unsignedIntegerValue];
                if (width == 0 || height == 0 || width > 16 || height > 16 || length != pitch * height || offset > pixels.length || length > pixels.length - offset)
                    return finish(m, out, @"image_layout", nil, 3);
                MTLPixelFormat format = (MTLPixelFormat)[c[@"pixel_format"] unsignedIntegerValue];
                if (format != MTLPixelFormatRGBA8Unorm && format != MTLPixelFormatBGRA8Unorm && format != MTLPixelFormatRGBA16Float)
                    return finish(m, out, @"image_format", nil, 3);
                MTLTextureDescriptor *d = [MTLTextureDescriptor texture2DDescriptorWithPixelFormat:format width:width height:height mipmapped:NO];
                d.storageMode = MTLStorageModeShared;
                d.usage = MTLTextureUsageShaderRead;
                d.sampleCount = 1;
                d.arrayLength = 1;
                id<MTLTexture> image = [device newTextureWithDescriptor:d];
                if (!image || image.textureType != MTLTextureType2D || image.mipmapLevelCount != 1 || image.sampleCount != 1 || image.arrayLength != 1)
                    return finish(m, out, @"texture_allocation", nil, 3);
                [image replaceRegion:MTLRegionMake2D(0, 0, width, height) mipmapLevel:0 withBytes:(const uint8_t *)pixels.bytes + offset bytesPerRow:pitch];
                [image getBytes:(uint8_t *)uploaded.mutableBytes + offset bytesPerRow:pitch fromRegion:MTLRegionMake2D(0, 0, width, height) mipmapLevel:0];
                [images addObject:image];
                [imageMetadata addObject:@{@"width": @(image.width), @"height": @(image.height), @"pixel_format": @(image.pixelFormat),
                    @"mip_levels": @(image.mipmapLevelCount), @"sample_count": @(image.sampleCount), @"array_length": @(image.arrayLength)}];
            }
            m[@"single_mip_images_created"] = @YES;
            m[@"images"] = imageMetadata;
            m[@"input_images_read_back_equal"] = @([uploaded isEqualToData:pixels]);
            if (![uploaded writeToFile:[out stringByAppendingPathComponent:@"uploaded-images.bin"] atomically:YES] || ![uploaded isEqualToData:pixels])
                return finish(m, out, @"image_upload_readback", nil, 3);
            NSMutableArray<id<MTLSamplerState>> *samplers = [NSMutableArray array];
            NSMutableArray *samplerMetadata = [NSMutableArray array];
            for (NSDictionary *c in samplerCases) {
                MTLSamplerDescriptor *d = [MTLSamplerDescriptor new];
                BOOL linear = [c[@"filter"] isEqualToString:@"linear"];
                d.minFilter = linear ? MTLSamplerMinMagFilterLinear : MTLSamplerMinMagFilterNearest;
                d.magFilter = d.minFilter;
                d.mipFilter = MTLSamplerMipFilterNotMipmapped;
                d.maxAnisotropy = 1;
                d.normalizedCoordinates = [c[@"normalized"] boolValue];
                NSString *address = c[@"address"];
                if ([address isEqualToString:@"edge"]) d.sAddressMode = MTLSamplerAddressModeClampToEdge;
                else if ([address isEqualToString:@"zero"]) d.sAddressMode = MTLSamplerAddressModeClampToZero;
                else if ([address isEqualToString:@"repeat"]) d.sAddressMode = MTLSamplerAddressModeRepeat;
                else if ([address isEqualToString:@"mirror"]) d.sAddressMode = MTLSamplerAddressModeMirrorRepeat;
                else return finish(m, out, @"sampler_address", nil, 3);
                d.tAddressMode = d.sAddressMode;
                d.rAddressMode = MTLSamplerAddressModeClampToEdge;
                d.compareFunction = MTLCompareFunctionNever;
                d.lodMinClamp = 0.0f;
                d.lodMaxClamp = 1000.0f;
                id<MTLSamplerState> state = [device newSamplerStateWithDescriptor:d];
                if (!state) return finish(m, out, @"sampler_allocation", nil, 3);
                [samplers addObject:state];
                [samplerMetadata addObject:@{@"min_filter": @(d.minFilter), @"mag_filter": @(d.magFilter), @"mip_filter": @(d.mipFilter),
                    @"s_address": @(d.sAddressMode), @"t_address": @(d.tAddressMode), @"normalized": @(d.normalizedCoordinates),
                    @"anisotropy": @(d.maxAnisotropy), @"compare": @(d.compareFunction), @"lod_min": @(d.lodMinClamp), @"lod_max": @(d.lodMaxClamp)}];
            }
            m[@"samplers_created"] = @YES;
            m[@"samplers"] = samplerMetadata;
            id<MTLCommandQueue> queue = [device newCommandQueue];
            id<MTLBuffer> input = [device newBufferWithBytes:inputs.bytes length:inputs.length options:MTLResourceStorageModeShared];
            const NSUInteger runStride = count * sizeof(Output);
            id<MTLBuffer> output = [device newBufferWithLength:runs.count * runStride options:MTLResourceStorageModeShared];
            if (!queue || !input || !output) return finish(m, out, @"buffer_allocation", nil, 3);
            memset(output.contents, 0xa5, output.length);
            id<MTLCommandBuffer> command = [queue commandBuffer];
            if (!command) return finish(m, out, @"command_buffer", nil, 3);
            NSUInteger encoded = 0;
            for (NSDictionary *c in runs) {
                NSUInteger run = [c[@"id"] unsignedIntegerValue], ii = [c[@"image"] unsignedIntegerValue], si = [c[@"sampler"] unsignedIntegerValue];
                NSUInteger precision = [c[@"return_precision"] unsignedIntegerValue];
                if (run != encoded || ii >= images.count || si >= samplers.count || (precision != 16 && precision != 32))
                    return finish(m, out, @"run_layout", nil, 3);
                NSDictionary *imageCase = imageCases[ii];
                NSUInteger gi = [imageCase[@"geometry"] unsignedIntegerValue];
                if (gi >= geometries.count) return finish(m, out, @"geometry_index", nil, 3);
                NSDictionary *geometry = geometries[gi];
                NSUInteger inputOffset = [geometry[@"input_offset"] unsignedIntegerValue];
                if (inputOffset > inputs.length || count * sizeof(Input) > inputs.length - inputOffset)
                    return finish(m, out, @"input_span", nil, 3);
                NSDictionary *samplerCase = samplerCases[si];
                Params params = {(uint32_t)count, [samplerCase[@"normalized"] boolValue] ? 1u : 0u, (uint32_t)run, 0};
                id<MTLComputePipelineState> pipeline = pipelines[precision == 16 ? 1 : 0];
                NSUInteger threads = MIN((NSUInteger)64, pipeline.maxTotalThreadsPerThreadgroup);
                if (threads == 0) return finish(m, out, @"thread_limit", nil, 3);
                id<MTLComputeCommandEncoder> encoder = [command computeCommandEncoder];
                if (!encoder) return finish(m, out, @"compute_encoder", nil, 3);
                [encoder setComputePipelineState:pipeline];
                [encoder setBuffer:input offset:inputOffset atIndex:0];
                [encoder setBuffer:output offset:run * runStride atIndex:1];
                [encoder setBytes:&params length:sizeof(params) atIndex:2];
                [encoder setTexture:images[ii] atIndex:0];
                [encoder setSamplerState:samplers[si] atIndex:0];
                [encoder dispatchThreads:MTLSizeMake(count, 1, 1) threadsPerThreadgroup:MTLSizeMake(threads, 1, 1)];
                [encoder endEncoding];
                encoded += 1;
            }
            [command commit];
            [command waitUntilCompleted];
            m[@"wait_until_completed_called"] = @YES;
            m[@"command_status"] = @(command.status);
            m[@"command_completed"] = @(command.status == MTLCommandBufferStatusCompleted);
            m[@"compute_dispatches"] = @(encoded);
            m[@"case_count_per_dispatch"] = @(count);
            m[@"input_buffer_bytes"] = @(input.length);
            m[@"readback_bytes"] = @(output.length);
            if (command.status != MTLCommandBufferStatusCompleted || command.error)
                return finish(m, out, @"command_completion", command.error, 3);
            if (!save(out, @"results.bin", output.contents, output.length)) return finish(m, out, @"readback_write", nil, 3);
            m[@"readback_copied_after_successful_wait"] = @YES;
            m[@"availability"] = @"available_and_executed";
            return finish(m, out, @"completed", nil, 0);
        } @catch (NSException *exception) {
            (void)exception;
            return finish(m, out, @"objective_c_exception", nil, 3);
        }
    }
}
