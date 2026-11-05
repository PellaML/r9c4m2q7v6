#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#include <stdint.h>
#include <string.h>
#include <sys/sysctl.h>

// Adapted public harness idioms only: native setup and completed byte copies.
typedef struct { uint32_t abi,seed,mode,coverage,profile,repeat,variant,pixels; } Request;
typedef struct {
    uint32_t abi,status,completed,no_error,copied,guards;
    uint32_t seed,mode,coverage,profile,repeat,variant,width,height;
    uint64_t device;
} Evidence;
_Static_assert(sizeof(Request)==32 && sizeof(Evidence)==64,"C/Rust ABI");
_Static_assert(MTLPixelFormatRGBA16Float==115 && MTLPixelFormatRG16Float==65,"formats");
_Static_assert(MTLCommandBufferStatusCompleted==4,"completion");
enum { W=16,H=16,ROW=256,GUARD=256,TRACE_BYTES=256*128 };

static int finish(NSMutableDictionary *m,NSString *status,NSError *e,int code,char *json,size_t cap) {
    m[@"status"]=status;
    m[@"error"]=e?@{@"domain":e.domain,@"code":@(e.code),
        @"message":[e.localizedDescription substringToIndex:MIN((NSUInteger)2048,e.localizedDescription.length)]}:(id)[NSNull null];
    NSData *data=[NSJSONSerialization dataWithJSONObject:m options:NSJSONWritingSortedKeys error:NULL];
    if(!data || data.length>=cap)return 70;
    memcpy(json,data.bytes,data.length);json[data.length]=0;return code;
}
static NSDictionary *compiler(MTLCompileOptions *o) {
    NSMutableDictionary *m=[@{@"fast_math":@(o.fastMathEnabled),@"language_version":@(o.languageVersion),
        @"optimization_level":@(o.optimizationLevel),@"preserve_invariance":@(o.preserveInvariance)} mutableCopy];
    if(@available(macOS 15.0,*)){m[@"math_mode"]=@(o.mathMode);m[@"floating_point_functions"]=@(o.mathFloatingPointFunctions);}
    return m;
}
static id<MTLBuffer> guarded(id<MTLDevice> d,NSUInteger bytes) {
    id<MTLBuffer> b=[d newBufferWithLength:bytes+2*GUARD options:MTLResourceStorageModeShared];
    if(b)memset(b.contents,0xa5,b.length);return b;
}
static BOOL intact(id<MTLBuffer> b,NSUInteger bytes,NSUInteger row_payload) {
    const uint8_t *p=b.contents;
    for(NSUInteger i=0;i<b.length;i++){
        BOOL edge=i<GUARD || i>=GUARD+bytes;
        BOOL padding=!edge && row_payload && (i-GUARD)%ROW>=row_payload;
        if((edge||padding)&&p[i]!=0xa5)return NO;
    }return YES;
}
static void read_texture(id<MTLBlitCommandEncoder> b,id<MTLTexture> t,id<MTLBuffer> out) {
    [b copyFromTexture:t sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0)
        sourceSize:MTLSizeMake(W,H,1) toBuffer:out destinationOffset:GUARD
        destinationBytesPerRow:ROW destinationBytesPerImage:H*ROW];
}
static id<MTLRenderPipelineState> pipeline(id<MTLDevice> d,id<MTLLibrary> l,NSString *entry,NSMutableDictionary *meta,NSError **error) {
    MTLRenderPipelineDescriptor *p=[MTLRenderPipelineDescriptor new];
    p.vertexFunction=[l newFunctionWithName:@"vertex_plane"];
    p.fragmentFunction=[l newFunctionWithName:entry];
    p.rasterSampleCount=1;p.alphaToCoverageEnabled=NO;p.alphaToOneEnabled=NO;
    p.depthAttachmentPixelFormat=MTLPixelFormatInvalid;p.stencilAttachmentPixelFormat=MTLPixelFormatInvalid;
    for(NSUInteger i=0;i<2;i++){
        p.colorAttachments[i].pixelFormat=i==0?MTLPixelFormatRGBA16Float:MTLPixelFormatRG16Float;
        p.colorAttachments[i].blendingEnabled=NO;p.colorAttachments[i].writeMask=MTLColorWriteMaskAll;
    }
    meta[@"color0_format"]=@(p.colorAttachments[0].pixelFormat);meta[@"color1_format"]=@(p.colorAttachments[1].pixelFormat);
    meta[@"color0_blending"]=@(p.colorAttachments[0].blendingEnabled);meta[@"color1_blending"]=@(p.colorAttachments[1].blendingEnabled);
    meta[@"color0_write_mask"]=@(p.colorAttachments[0].writeMask);meta[@"color1_write_mask"]=@(p.colorAttachments[1].writeMask);
    meta[@"samples"]=@(p.rasterSampleCount);meta[@"entry"]=entry;
    if(!p.vertexFunction||!p.fragmentFunction)return nil;
    return [d newRenderPipelineStateWithDescriptor:p error:error];
}
static void draw(id<MTLRenderCommandEncoder> e,id<MTLRenderPipelineState> p,const float *points,
                 const uint32_t *params,id<MTLBuffer> trace,MTLScissorRect scissor) {
    [e setRenderPipelineState:p];[e setViewport:(MTLViewport){0,0,W,H,0,1}];[e setScissorRect:scissor];
    [e setVertexBytes:points length:6*sizeof(float) atIndex:0];
    [e setVertexBytes:params length:8*sizeof(uint32_t) atIndex:1];
    [e setFragmentBytes:params length:8*sizeof(uint32_t) atIndex:1];
    [e setFragmentBuffer:trace offset:GUARD atIndex:0];
    [e drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
}
int quad_fetch_native(const Request *r,const char *source,const uint16_t *initial0,const uint16_t *initial1,
    const float *vertices,const uint32_t *scissor,uint16_t *before0,uint16_t *before1,uint16_t *after0,uint16_t *after1,
    uint32_t *trace,Evidence *e,char *json,size_t cap) {
    if(!e||!json||!cap)return 64;
    memset(e,0,sizeof(*e));e->abi=1;
    @autoreleasepool {
        NSMutableDictionary *m=[@{@"schema":@1,@"backend":@"native_metal",@"completed":@NO,@"copied":@NO,
            @"os_version_and_build":NSProcessInfo.processInfo.operatingSystemVersionString} mutableCopy];
        @try {
            if(!r||r->abi!=1||r->seed>2||r->mode>4||r->coverage>3||r->profile>1||r->repeat>1||r->variant>3||r->pixels!=W*H||
                !source||!initial0||!initial1||!vertices||!scissor||!before0||!before1||!after0||!after1||!trace)
                return finish(m,@"invalid_request",nil,64,json,cap);
            e->seed=r->seed;e->mode=r->mode;e->coverage=r->coverage;e->profile=r->profile;
            e->repeat=r->repeat;e->variant=r->variant;e->width=W;e->height=H;
            m[@"request"]=@{@"seed":@(r->seed),@"mode":@(r->mode),@"coverage":@(r->coverage),@"profile":@(r->profile),
                @"repeat":@(r->repeat),@"variant":@(r->variant),@"width":@(W),@"height":@(H)};
            char build[128]={0};size_t n=sizeof(build);
            if(sysctlbyname("kern.osversion",build,&n,NULL,0)!=0)return finish(m,@"os_build_unavailable",nil,23,json,cap);
            m[@"os_build"]=@(build);
            id<MTLDevice> device=MTLCreateSystemDefaultDevice();
            if(!device)return finish(m,@"no_metal_device_no_fallback",nil,23,json,cap);
            e->device=device.registryID;
            m[@"device"]=@{@"name":device.name?:@"unknown",@"registry_id":@(device.registryID),@"unified_memory":@(device.hasUnifiedMemory),
                @"mac2":@([device supportsFamily:MTLGPUFamilyMac2]),@"apple7":@([device supportsFamily:MTLGPUFamilyApple7]),@"metal3":@([device supportsFamily:MTLGPUFamilyMetal3])};
            MTLCompileOptions *o=[MTLCompileOptions new];m[@"default_compiler"]=compiler(o);
            if(r->profile==1)o.fastMathEnabled=NO;
            o.preprocessorMacros=@{@"FAULT":@(r->variant)};
            m[@"effective_compiler"]=compiler(o);m[@"texture_rounding_override"]=@"none";
            NSError *error=nil;
            id<MTLLibrary> library=[device newLibraryWithSource:@(source) options:o error:&error];
            if(!library||error)return finish(m,@"shader_compile_failed",error,24,json,cap);
            NSArray *names=@[@"fetch_only",@"fetch_independent",@"fetch_half_derivative",@"fetch_float_derivative",@"analytic_derivative"];
            NSMutableDictionary *pm=[NSMutableDictionary dictionary];
            id<MTLRenderPipelineState> consumer=pipeline(device,library,names[r->mode],pm,&error);
            if(!consumer||error)return finish(m,@"consumer_pipeline_failed",error,25,json,cap);
            id<MTLRenderPipelineState> seed=nil;
            if(r->seed==2){NSMutableDictionary *sm=[NSMutableDictionary dictionary];seed=pipeline(device,library,@"seed_plane",sm,&error);m[@"seed_pipeline"]=sm;if(!seed||error)return finish(m,@"seed_pipeline_failed",error,25,json,cap);}
            pm[@"actual_framebuffer_fetch_input"]=@(r->mode!=4);pm[@"fetch_type"]=@"half2 color(1)";m[@"pipeline"]=pm;
            NSMutableArray *points=[NSMutableArray array];for(NSUInteger i=0;i<12;i++)[points addObject:@(vertices[i])];m[@"vertex_points"]=points;
            NSMutableArray<id<MTLTexture>> *textures=[NSMutableArray array];
            NSMutableArray<id<MTLBuffer>> *uploads=[NSMutableArray array],*pre=[NSMutableArray array],*post=[NSMutableArray array];
            for(NSUInteger slot=0;slot<2;slot++){
                NSUInteger channels=slot==0?4:2;
                MTLTextureDescriptor *td=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:(slot==0?MTLPixelFormatRGBA16Float:MTLPixelFormatRG16Float) width:W height:H mipmapped:NO];
                td.storageMode=MTLStorageModePrivate;td.usage=MTLTextureUsageRenderTarget;
                id<MTLTexture> t=[device newTextureWithDescriptor:td];
                id<MTLBuffer> u=guarded(device,H*ROW),p=guarded(device,H*ROW),q=guarded(device,H*ROW);
                if(!t||!u||!p||!q)return finish(m,@"texture_or_buffer_allocation_failed",nil,26,json,cap);
                if(t.pixelFormat!=td.pixelFormat||t.width!=W||t.height!=H||t.sampleCount!=1||t.mipmapLevelCount!=1||t.arrayLength!=1||
                    t.usage!=MTLTextureUsageRenderTarget||t.storageMode!=MTLStorageModePrivate||t.textureType!=MTLTextureType2D||t.framebufferOnly)
                    return finish(m,@"texture_property_failure",nil,26,json,cap);
                const uint16_t *input=slot==0?initial0:initial1;
                for(NSUInteger y=0;y<H;y++)memcpy((uint8_t*)u.contents+GUARD+y*ROW,input+y*W*channels,W*channels*2);
                [textures addObject:t];[uploads addObject:u];[pre addObject:p];[post addObject:q];
            }
            id<MTLBuffer> tb=guarded(device,TRACE_BYTES);id<MTLCommandQueue> queue=[device newCommandQueue];
            if(!tb||!queue)return finish(m,@"trace_or_queue_unavailable",nil,26,json,cap);
            id<MTLCommandBuffer> cb=[queue commandBuffer];if(!cb)return finish(m,@"command_unavailable",nil,26,json,cap);
            id<MTLBlitCommandEncoder> upload=[cb blitCommandEncoder];if(!upload)return finish(m,@"upload_encoder_failed",nil,26,json,cap);
            for(NSUInteger i=0;i<2;i++)[upload copyFromBuffer:uploads[i] sourceOffset:GUARD sourceBytesPerRow:ROW sourceBytesPerImage:H*ROW
                sourceSize:MTLSizeMake(W,H,1) toTexture:textures[i] destinationSlice:0 destinationLevel:0 destinationOrigin:MTLOriginMake(0,0,0)];
            [upload endEncoding];
            uint32_t params[8]={W,H,r->mode,r->seed,0,r->variant,0,0};
            MTLRenderPassDescriptor *first=[MTLRenderPassDescriptor renderPassDescriptor];
            for(NSUInteger i=0;i<2;i++){
                first.colorAttachments[i].texture=textures[i];first.colorAttachments[i].loadAction=r->seed==0?MTLLoadActionLoad:MTLLoadActionClear;
                first.colorAttachments[i].storeAction=MTLStoreActionStore;
            }
            first.colorAttachments[0].clearColor=MTLClearColorMake(-101,-102,-103,-104);
            first.colorAttachments[1].clearColor=r->seed==1?MTLClearColorMake(7,-5,0,1):MTLClearColorMake(-11,13,0,1);
            id<MTLRenderCommandEncoder> s=[cb renderCommandEncoderWithDescriptor:first];if(!s)return finish(m,@"seed_encoder_failed",nil,26,json,cap);
            if(r->seed==2)draw(s,seed,vertices,params,tb,(MTLScissorRect){0,0,W,H});
            [s endEncoding];
            id<MTLBlitCommandEncoder> capture=[cb blitCommandEncoder];if(!capture)return finish(m,@"seed_capture_failed",nil,26,json,cap);
            for(NSUInteger i=0;i<2;i++)read_texture(capture,textures[i],pre[i]);[capture endEncoding];
            MTLRenderPassDescriptor *second=[MTLRenderPassDescriptor renderPassDescriptor];
            for(NSUInteger i=0;i<2;i++){
                second.colorAttachments[i].texture=textures[i];second.colorAttachments[i].loadAction=MTLLoadActionLoad;second.colorAttachments[i].storeAction=MTLStoreActionStore;
            }
            MTLScissorRect rect={scissor[0],scissor[1],scissor[2],scissor[3]};
            if(r->variant==3){rect.x++;rect.y++;}
            if(rect.x+rect.width>W||rect.y+rect.height>H)return finish(m,@"scissor_out_of_bounds",nil,64,json,cap);
            m[@"scissor_requested"]=@[@(scissor[0]),@(scissor[1]),@(scissor[2]),@(scissor[3])];
            m[@"scissor_actual"]=@[@(rect.x),@(rect.y),@(rect.width),@(rect.height)];
            m[@"passes"]=@{@"count":@2,@"seed_load":@(first.colorAttachments[1].loadAction),@"seed_store":@(MTLStoreActionStore),
                @"consumer_load":@(MTLLoadActionLoad),@"consumer_store":@(MTLStoreActionStore),@"seed_capture_before_consumer":@YES,
                @"seed_draw":@(r->seed==2),@"consumer_draw_count":@1};
            id<MTLRenderCommandEncoder> c=[cb renderCommandEncoderWithDescriptor:second];if(!c)return finish(m,@"consumer_encoder_failed",nil,26,json,cap);
            draw(c,consumer,vertices+6,params,tb,rect);[c endEncoding];
            id<MTLBlitCommandEncoder> read=[cb blitCommandEncoder];if(!read)return finish(m,@"final_readback_failed",nil,26,json,cap);
            for(NSUInteger i=0;i<2;i++)read_texture(read,textures[i],post[i]);[read endEncoding];
            [cb commit];[cb waitUntilCompleted];e->status=(uint32_t)cb.status;e->completed=cb.status==MTLCommandBufferStatusCompleted;e->no_error=cb.error==nil;
            m[@"command_status"]=@(cb.status);m[@"completed"]=@(e->completed!=0);m[@"wait_until_completed_called"]=@YES;
            if(!e->completed||!e->no_error)return finish(m,@"command_failed",cb.error,27,json,cap);
            if(!intact(tb,TRACE_BYTES,0))return finish(m,@"trace_guard_changed",nil,28,json,cap);
            for(NSUInteger slot=0;slot<2;slot++){
                NSUInteger channels=slot==0?4:2;
                if(!intact(uploads[slot],H*ROW,W*channels*2)||!intact(pre[slot],H*ROW,W*channels*2)||!intact(post[slot],H*ROW,W*channels*2))
                    return finish(m,@"texture_buffer_guard_changed",nil,28,json,cap);
                uint16_t *b=slot==0?before0:before1,*a=slot==0?after0:after1;
                for(NSUInteger y=0;y<H;y++){
                    memcpy(b+y*W*channels,(uint8_t*)pre[slot].contents+GUARD+y*ROW,W*channels*2);
                    memcpy(a+y*W*channels,(uint8_t*)post[slot].contents+GUARD+y*ROW,W*channels*2);
                }
            }
            memcpy(trace,(uint8_t*)tb.contents+GUARD,TRACE_BYTES);e->guards=1;e->copied=1;
            m[@"guards_intact"]=@YES;m[@"copied"]=@YES;
            return finish(m,@"gpu_readback_observed",nil,0,json,cap);
        }@catch(NSException *exception){m[@"exception_name"]=exception.name?:@"unknown";return finish(m,@"objective_c_exception",nil,29,json,cap);}
    }
}
