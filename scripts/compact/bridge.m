#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <objc/runtime.h>
#include <dispatch/dispatch.h>
#include <dlfcn.h>
#include <stdint.h>
#include <string.h>
#include <stdio.h>
#include <mach-o/loader.h>
#include <CommonCrypto/CommonDigest.h>

static NSString *hash_bytes(const void *bytes, size_t size) {
    unsigned char digest[CC_SHA256_DIGEST_LENGTH];
    char hex[CC_SHA256_DIGEST_LENGTH * 2 + 1];
    CC_SHA256(bytes,(CC_LONG)size,digest);
    for (size_t i=0;i<sizeof(digest);i++) snprintf(hex+2*i,3,"%02x",digest[i]);
    return @(hex);
}

static void image_identity(NSMutableDictionary *facts,const void *base) {
    const struct mach_header_64 *header=base;
    if (!header || header->magic!=MH_MAGIC_64 || header->sizeofcmds>65536 || header->ncmds>4096) return;
    const uint8_t *at=(const uint8_t *)(header+1), *end=at+header->sizeofcmds;
    for (uint32_t i=0;i<header->ncmds;i++) {
        if ((size_t)(end-at)<sizeof(struct load_command)) return;
        const struct load_command *load=(const struct load_command *)at;
        if (load->cmdsize<sizeof(*load) || load->cmdsize>(size_t)(end-at)) return;
        if (load->cmd==LC_UUID && load->cmdsize>=sizeof(struct uuid_command)) {
            const struct uuid_command *uuid=(const struct uuid_command *)at;
            facts[@"reader_image_uuid"]=[[[NSUUID alloc] initWithUUIDBytes:uuid->uuid] UUIDString];
        }
        if (load->cmd==LC_SEGMENT_64 && load->cmdsize>=sizeof(struct segment_command_64)) {
            const struct segment_command_64 *segment=(const struct segment_command_64 *)at;
            if (!strncmp(segment->segname,"__TEXT",16) && segment->filesize<=segment->vmsize && segment->filesize<=8*1024*1024) {
                facts[@"reader_TEXT_bytes"]=@(segment->filesize);
                facts[@"reader_TEXT_sha256"]=hash_bytes(base,(size_t)segment->filesize);
                if (segment->nsects>(load->cmdsize-sizeof(*segment))/sizeof(struct section_64)) return;
                const struct section_64 *section=(const struct section_64 *)(segment+1);
                for (uint32_t j=0;j<segment->nsects;j++) {
                    if (!strncmp(section[j].sectname,"__text",16) && section[j].addr>=segment->vmaddr &&
                        section[j].size<=segment->filesize && section[j].addr-segment->vmaddr<=segment->filesize-section[j].size) {
                        const uint8_t *code=(const uint8_t *)base+(section[j].addr-segment->vmaddr);
                        facts[@"reader_text_bytes"]=@(section[j].size);
                        facts[@"reader_text_sha256"]=hash_bytes(code,(size_t)section[j].size);
                    }
                }
            }
        }
        at+=load->cmdsize;
    }
}

@protocol CompactDevice
- (MTLComputePipelineDescriptor *)newComputePipelineDescriptorWithSerializedData:(dispatch_data_t)data deserializationContext:(id)context;
@end
@interface MTLComputePipelineDescriptor (CompactProbe)
- (NSInteger)textureWriteRoundingMode;
@end

int compact_probe(const uint8_t *bytes, size_t length, int64_t *rounding,
                  uint64_t *threads, char *json, size_t capacity) {
    @autoreleasepool {
        NSMutableDictionary *facts = [@{@"os":NSProcessInfo.processInfo.operatingSystemVersionString,
            @"payload_bytes":@(length), @"descriptor_observed":@NO,
            @"pipeline_created":@NO, @"gpu_execution_claimed":@NO} mutableCopy];
        int code = 2;
        @try {
            id<MTLDevice> device = MTLCreateSystemDefaultDevice();
            if (!device) { facts[@"status"] = @"no_device"; goto done; }
            facts[@"device"] = device.name;
            facts[@"device_class"] = NSStringFromClass([(NSObject *)device class]);
            SEL selector = @selector(newComputePipelineDescriptorWithSerializedData:deserializationContext:);
            if (![device respondsToSelector:selector]) { facts[@"status"] = @"reader_absent"; goto done; }
            NSMethodSignature *signature = [(NSObject *)device methodSignatureForSelector:selector];
            Method method = class_getInstanceMethod([(NSObject *)device class], selector);
            facts[@"reader_encoding"] = method ? @(method_getTypeEncoding(method)) : @"missing";
            if (!signature || signature.numberOfArguments != 4 || signature.methodReturnType[0] != '@' ||
                [signature getArgumentTypeAtIndex:2][0] != '@' || [signature getArgumentTypeAtIndex:3][0] != '@') {
                facts[@"status"] = @"reader_abi_unqualified"; goto done;
            }
            Dl_info origin;
            if (method && dladdr((const void *)method_getImplementation(method), &origin)) {
                facts[@"reader_image"] = origin.dli_fname ? @(origin.dli_fname) : @"unknown";
                facts[@"reader_symbol"] = origin.dli_sname ? @(origin.dli_sname) : @"unknown";
                facts[@"reader_image_offset"] = @((uintptr_t)method_getImplementation(method) - (uintptr_t)origin.dli_fbase);
                image_identity(facts,origin.dli_fbase);
            }
            if (!bytes || length == 0 || length > 64) { facts[@"status"] = @"input_bound"; goto done; }
            dispatch_data_t data = dispatch_data_create(bytes, length, dispatch_get_global_queue(QOS_CLASS_DEFAULT,0), DISPATCH_DATA_DESTRUCTOR_DEFAULT);
            MTLComputePipelineDescriptor *descriptor = [(id<CompactDevice>)device newComputePipelineDescriptorWithSerializedData:data deserializationContext:nil];
            if (!descriptor) { facts[@"status"] = @"reader_returned_nil"; goto done; }
            facts[@"descriptor_class"] = NSStringFromClass(descriptor.class);
            SEL getter = @selector(textureWriteRoundingMode);
            NSMethodSignature *get = [descriptor methodSignatureForSelector:getter];
            if (!get || get.numberOfArguments != 2 || strcmp(get.methodReturnType,@encode(NSInteger))) {
                facts[@"status"] = @"getter_abi_unqualified"; goto done;
            }
            *rounding = (int64_t)[descriptor textureWriteRoundingMode];
            *threads = (uint64_t)descriptor.maxTotalThreadsPerThreadgroup;
            facts[@"rounding_getter"] = @(*rounding);
            facts[@"max_threads_getter"] = @(*threads);
            facts[@"compute_function_is_nil"] = @(descriptor.computeFunction == nil);
            facts[@"descriptor_observed"] = @YES;
            facts[@"status"] = @"native_descriptor_observed";
            code = 0;
        } @catch (NSException *exception) {
            facts[@"status"] = @"exception";
            facts[@"exception_name"] = exception.name;
        }
done:;
        NSData *encoded = [NSJSONSerialization dataWithJSONObject:facts options:NSJSONWritingSortedKeys error:NULL];
        if (!encoded || encoded.length >= capacity) return 70;
        memcpy(json,encoded.bytes,encoded.length);
        json[encoded.length] = 0;
        return code;
    }
}
