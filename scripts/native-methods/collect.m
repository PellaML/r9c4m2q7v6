#import <Foundation/Foundation.h>
#import <objc/runtime.h>
#include <CommonCrypto/CommonDigest.h>
#include <dlfcn.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <mach-o/dyld.h>
#include <mach-o/loader.h>
#include <ptrauth.h>

// Bounded introspection of the framework loaded into this process only.
// No target process, guest, account file, or service executable is opened.
static uintptr_t slide, textBase, codeLo, codeHi;
static NSMutableArray *starts, *queue, *windows, *references, *stubs;
static NSMutableDictionary *names;
static NSMutableSet *queued, *seenReferences, *seenStubs;
static NSUInteger exportedBytes;
static const NSUInteger byteLimit = 256 * 1024, windowLimit = 128;

static NSString *hx(uint64_t v) { return [NSString stringWithFormat:@"0x%llx", (unsigned long long)v]; }
static uintptr_t canonical(uintptr_t v) { return v & 0x0000ffffffffffffULL; }
static NSData *readBytes(uintptr_t p, NSUInteger n) {
    if (!p || !n || n > 2 * 1024 * 1024) return nil;
    NSMutableData *d = [NSMutableData dataWithLength:n];
    mach_vm_size_t got = 0;
    kern_return_t k = mach_vm_read_overwrite(mach_task_self(), p, n,
                                            (mach_vm_address_t)d.mutableBytes, &got);
    return k == KERN_SUCCESS && got == n ? d : nil;
}
static NSString *hexData(NSData *d) {
    if (!d) return @"";
    NSMutableString *s = [NSMutableString stringWithCapacity:d.length * 2];
    const unsigned char *p = d.bytes;
    for (NSUInteger i = 0; i < d.length; ++i) [s appendFormat:@"%02x", p[i]];
    return s;
}
static NSString *sha(NSData *d) {
    unsigned char digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256(d.bytes, (CC_LONG)d.length, digest);
    return hexData([NSData dataWithBytes:digest length:sizeof(digest)]);
}
static NSDictionary *symbol(uintptr_t p) {
    Dl_info d = {0};
    if (!dladdr((void *)p, &d)) return @{};
    NSMutableDictionary *r = [NSMutableDictionary dictionary];
    if (d.dli_fname) r[@"image"] = @(d.dli_fname);
    if (d.dli_sname) r[@"name"] = @(d.dli_sname);
    if (d.dli_saddr) r[@"symbol_address"] = hx((uintptr_t)d.dli_saddr);
    if (d.dli_fbase) r[@"image_base"] = hx((uintptr_t)d.dli_fbase);
    return r;
}
static NSString *boundedString(uintptr_t p) {
    NSDictionary *s = symbol(p);
    NSString *image = s[@"image"];
    if (!([image hasPrefix:@"/System/Library/"] || [image hasPrefix:@"/usr/lib/"])) return nil;
    NSData *d = readBytes(p, 192);
    if (!d) return nil;
    const unsigned char *b = d.bytes;
    NSUInteger n = 0;
    while (n < d.length && b[n]) {
        if (b[n] < 0x20 || b[n] > 0x7e) return nil;
        ++n;
    }
    if (!n || n == d.length) return nil;
    return [[NSString alloc] initWithBytes:b length:n encoding:NSUTF8StringEncoding];
}
static void enqueue(uintptr_t p, NSString *reason, NSUInteger depth) {
    if (p < codeLo || p >= codeHi || depth > 2 || queue.count >= windowLimit) return;
    if (![starts containsObject:@(p)] && starts.count) return;
    if ([queued containsObject:@(p)]) return;
    [queued addObject:@(p)];
    [queue addObject:@{@"address":@(p), @"reason":reason, @"depth":@(depth)}];
}
static void reference(uintptr_t pc, uintptr_t p, NSUInteger depth) {
    if (references.count >= 1536 || [seenReferences containsObject:@(p)]) return;
    [seenReferences addObject:@(p)];
    NSMutableDictionary *r = [NSMutableDictionary dictionaryWithDictionary:
        @{@"instruction":hx(pc - slide), @"runtime_address":hx(p), @"symbol":symbol(p)}];
    if (p >= codeLo && p < codeHi) {
        r[@"vmaddr"] = hx(p - slide);
        enqueue(p, @"code-address materialization", depth + 1);
    }
    NSData *d = readBytes(p, 32);
    if (d) {
        r[@"bytes_hex"] = hexData(d);
        uint64_t v = 0; memcpy(&v, d.bytes, 8);
        r[@"word64"] = hx(v);
        uintptr_t q = canonical(v);
        r[@"pointer_symbol"] = symbol(q);
        NSString *s = boundedString(p), *t = boundedString(q);
        if (s) r[@"ascii"] = s;
        if (t) r[@"pointed_ascii"] = t;
        if (q >= codeLo && q < codeHi) enqueue(q, @"code pointer", depth + 1);
    }
    [references addObject:r];
}
static int64_t signExtend(uint64_t v, unsigned bits) {
    uint64_t m = 1ULL << (bits - 1); return (int64_t)((v ^ m) - m);
}
static void scanReferences(NSData *data, uintptr_t base, NSUInteger depth) {
    const uint32_t *words = data.bytes;
    NSUInteger n = data.length / 4;
    for (NSUInteger i = 0; i < n; ++i) {
        uint32_t w = words[i]; uintptr_t pc = base + i * 4;
        if ((w & 0x9f000000U) == 0x90000000U) { // ADRP + immediate ADD/load
            int64_t imm = signExtend(((w >> 29) & 3) | (((uint64_t)(w >> 5) & 0x7ffff) << 2), 21) * 4096;
            uintptr_t page = (pc & ~4095ULL) + imm;
            unsigned reg = w & 31;
            for (NSUInteger j = i + 1; j < n && j <= i + 6; ++j) {
                uint32_t x = words[j];
                if (((x >> 5) & 31) != reg) continue;
                if ((x & 0xff000000U) == 0x91000000U) {
                    uintptr_t p = page + (((x >> 10) & 4095) << ((x & (1U << 22)) ? 12 : 0));
                    reference(base + j * 4, p, depth);
                } else if ((x & 0xffc00000U) == 0xf9400000U) {
                    reference(base + j * 4, page + (((x >> 10) & 4095) * 8), depth);
                } else if ((x & 0xffc00000U) == 0xb9400000U) {
                    reference(base + j * 4, page + (((x >> 10) & 4095) * 4), depth);
                }
            }
        } else if ((w & 0x9f000000U) == 0x10000000U) { // ADR
            int64_t imm = signExtend(((w >> 29) & 3) | (((uint64_t)(w >> 5) & 0x7ffff) << 2), 21);
            reference(pc, pc + imm, depth);
        }
    }
}
static void collectWindow(NSDictionary *item) {
    uintptr_t p = [item[@"address"] unsignedLongLongValue];
    NSUInteger depth = [item[@"depth"] unsignedIntegerValue];
    uintptr_t end = codeHi; BOOL exact = NO;
    for (NSNumber *v in starts) {
        uintptr_t a = v.unsignedLongLongValue;
        if (a == p) exact = YES;
        if (a > p) { end = a; break; }
    }
    NSUInteger length = MIN((NSUInteger)(end - p), exact ? 16384 : 2048);
    length = MIN(length, byteLimit - exportedBytes);
    if (!length) return;
    NSData *d = readBytes(p, length);
    if (!d) return;
    exportedBytes += length;
    NSMutableArray *branches = [NSMutableArray array];
    const uint32_t *w = d.bytes;
    for (NSUInteger i = 0; i < length / 4; ++i) {
        if ((w[i] & 0x7c000000U) != 0x14000000U) continue;
        uintptr_t pc = p + i * 4;
        uintptr_t target = pc + signExtend(w[i] & 0x03ffffffU, 26) * 4;
        NSMutableDictionary *b = [NSMutableDictionary dictionaryWithDictionary:
            @{@"at":hx(pc - slide), @"target_runtime":hx(target), @"symbol":symbol(target)}];
        if (target >= codeLo && target < codeHi) {
            b[@"target"] = hx(target - slide);
            if (names[@(target)]) b[@"methods"] = names[@(target)];
            if (target < p || target >= end) enqueue(target, @"direct branch", depth + 1);
        } else if (stubs.count < 256 && ![seenStubs containsObject:@(target)]) {
            [seenStubs addObject:@(target)];
            NSData *stub = readBytes(target, 32);
            if (stub) {
                [stubs addObject:@{@"runtime_address":hx(target), @"vmaddr_relative_to_target_slide":hx(target - slide),
                    @"symbol":symbol(target), @"bytes_hex":hexData(stub), @"sha256":sha(stub)}];
                scanReferences(stub, target, depth);
            }
        }
        [branches addObject:b];
    }
    scanReferences(d, p, depth);
    NSMutableDictionary *r = [NSMutableDictionary dictionaryWithDictionary:
        @{@"vmaddr":hx(p - slide), @"runtime_address":hx(p), @"length":@(length),
          @"function_end":hx(end - slide), @"exact_function_start":@(exact),
          @"truncated":@(p + length != end), @"reason":item[@"reason"], @"depth":@(depth),
          @"sha256":sha(d), @"bytes_hex":hexData(d), @"branches":branches, @"symbol":symbol(p)}];
    if (names[@(p)]) r[@"methods"] = names[@(p)];
    [windows addObject:r];
}
static BOOL wanted(NSString *className, NSString *selector) {
    NSString *s = selector.lowercaseString;
    if (([s containsString:@"synchroniz"] || [s containsString:@"discard"]) &&
        ([s containsString:@"resource"] || [s containsString:@"child"])) return YES;
    if ([s containsString:@"feature"] || [s isEqualToString:@"capabilities"]) return YES;
    if ([className isEqualToString:@"_PGDevice"] &&
        ([selector isEqualToString:@"getUserTaskID:"] || [selector isEqualToString:@"getTaskID:"])) return YES;
    if ([className isEqualToString:@"PGFIFO"] &&
        [@[@"signalStampValue:", @"faultAtStampValue:", @"pushStamp:cmdID:"] containsObject:selector]) return YES;
    return NO;
}
int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc != 2) return 2;
        const char *path = "/System/Library/Frameworks/ParavirtualizedGraphics.framework/ParavirtualizedGraphics";
        if (!dlopen(path, RTLD_NOW | RTLD_LOCAL)) return 3;
        const struct mach_header_64 *h = NULL; const char *image = NULL;
        for (uint32_t i = 0; i < _dyld_image_count(); ++i) {
            const char *s = _dyld_get_image_name(i);
            if (s && strstr(s, "/ParavirtualizedGraphics.framework/") && !strstr(s, "/XPCServices/")) {
                h = (const struct mach_header_64 *)_dyld_get_image_header(i);
                slide = _dyld_get_image_vmaddr_slide(i); image = s; break;
            }
        }
        if (!h || h->magic != MH_MAGIC_64) return 4;
        NSMutableArray *segments = [NSMutableArray array], *sections = [NSMutableArray array];
        const struct segment_command_64 *link = NULL;
        const struct linkedit_data_command *fs = NULL;
        NSString *uuid = @"";
        const uint8_t *lc = (const uint8_t *)(h + 1);
        for (uint32_t i = 0; i < h->ncmds; ++i) {
            const struct load_command *c = (const struct load_command *)lc;
            if (c->cmdsize < 8 || lc + c->cmdsize > (const uint8_t *)(h + 1) + h->sizeofcmds) return 5;
            if (c->cmd == LC_SEGMENT_64) {
                const struct segment_command_64 *g = (const struct segment_command_64 *)c;
                NSString *name = [[NSString alloc] initWithBytes:g->segname length:strnlen(g->segname, 16) encoding:NSUTF8StringEncoding];
                NSMutableDictionary *entry = [NSMutableDictionary dictionaryWithDictionary:
                    @{@"name":name, @"vmaddr":hx(g->vmaddr), @"vmsize":@(g->vmsize), @"fileoff":@(g->fileoff), @"filesize":@(g->filesize)}];
                if ([name isEqualToString:@"__TEXT"]) {
                    textBase = g->vmaddr + slide;
                    NSData *bytes = readBytes(textBase, g->filesize);
                    if (bytes) entry[@"live_segment_sha256"] = sha(bytes);
                }
                if ([name isEqualToString:@"__LINKEDIT"]) link = g;
                [segments addObject:entry];
                const struct section_64 *s = (const struct section_64 *)(g + 1);
                for (uint32_t j = 0; j < g->nsects; ++j) {
                    NSString *sn = [[NSString alloc] initWithBytes:s[j].sectname length:strnlen(s[j].sectname, 16) encoding:NSUTF8StringEncoding];
                    [sections addObject:@{@"segment":name, @"name":sn, @"vmaddr":hx(s[j].addr), @"size":@(s[j].size), @"fileoff":@(s[j].offset)}];
                    if ([name isEqualToString:@"__TEXT"] && [sn isEqualToString:@"__text"]) {
                        codeLo = s[j].addr + slide; codeHi = codeLo + s[j].size;
                    }
                }
            } else if (c->cmd == LC_FUNCTION_STARTS) fs = (const struct linkedit_data_command *)c;
            else if (c->cmd == LC_UUID) uuid = hexData([NSData dataWithBytes:((const struct uuid_command *)c)->uuid length:16]);
            lc += c->cmdsize;
        }
        if (!codeLo || !codeHi) return 6;
        starts = [NSMutableArray array]; queue = [NSMutableArray array]; windows = [NSMutableArray array];
        references = [NSMutableArray array]; stubs = [NSMutableArray array]; names = [NSMutableDictionary dictionary];
        queued = [NSMutableSet set]; seenReferences = [NSMutableSet set]; seenStubs = [NSMutableSet set];
        if (fs && link) {
            NSData *d = readBytes(slide + link->vmaddr - link->fileoff + fs->dataoff, fs->datasize);
            if (d) {
                const uint8_t *b = d.bytes; NSUInteger i = 0; uintptr_t addr = textBase;
                while (i < d.length) {
                    uint64_t v = 0; unsigned shift = 0; uint8_t part;
                    do { if (i >= d.length || shift > 63) return 7; part = b[i++]; v |= (uint64_t)(part & 127) << shift; shift += 7; } while (part & 128);
                    if (!v) break; addr += v;
                    if (addr >= codeLo && addr < codeHi) [starts addObject:@(addr)];
                }
            }
        }
        NSMutableArray *methods = [NSMutableArray array], *ivars = [NSMutableArray array];
        unsigned classCount = 0; Class *classes = objc_copyClassList(&classCount);
        for (unsigned i = 0; i < classCount; ++i) {
            Class cls = classes[i]; const char *im = class_getImageName(cls);
            if (!im || strcmp(im, image)) continue;
            NSString *cn = @(class_getName(cls));
            for (unsigned kind = 0; kind < 2; ++kind) {
                Class c = kind ? object_getClass(cls) : cls;
                unsigned count = 0; Method *ms = class_copyMethodList(c, &count);
                for (unsigned j = 0; j < count; ++j) {
                    NSString *selector = @(sel_getName(method_getName(ms[j])));
                    uintptr_t p = (uintptr_t)ptrauth_strip(method_getImplementation(ms[j]), ptrauth_key_function_pointer);
                    NSString *label = [NSString stringWithFormat:@"%c[%@ %@]", kind ? '+' : '-', cn, selector];
                    if (!names[@(p)]) names[@(p)] = [NSMutableArray array];
                    [names[@(p)] addObject:label];
                    if (wanted(cn, selector)) {
                        [methods addObject:@{@"class":cn, @"selector":selector, @"kind":kind ? @"class" : @"instance",
                            @"types":@(method_getTypeEncoding(ms[j])), @"vmaddr":hx(p - slide), @"runtime_address":hx(p)}];
                        enqueue(p, label, 0);
                    }
                }
                free(ms);
            }
            if ([cn isEqualToString:@"PGFIFO"] || [cn isEqualToString:@"_PGDevice"] ||
                [cn isEqualToString:@"PGRemoteTask"] || [cn.lowercaseString containsString:@"feature"]) {
                unsigned count = 0; Ivar *vs = class_copyIvarList(cls, &count);
                for (unsigned j = 0; j < count; ++j) {
                    [ivars addObject:@{@"class":cn, @"name":@(ivar_getName(vs[j])), @"type":@(ivar_getTypeEncoding(vs[j])), @"offset":@(ivar_getOffset(vs[j]))}];
                }
                free(vs);
            }
        }
        free(classes);
        for (NSUInteger i = 0; i < queue.count && exportedBytes < byteLimit; ++i) collectWindow(queue[i]);
        NSMutableDictionary *result = [NSMutableDictionary dictionaryWithDictionary:
            @{@"schema":@"bounded-native-methods-v1", @"image":@(image), @"uuid":uuid,
              @"cputype":@(h->cputype), @"cpusubtype":@(h->cpusubtype), @"slide":hx(slide),
              @"runtime_header":hx((uintptr_t)h), @"segments":segments, @"sections":sections,
              @"function_starts_count":@(starts.count), @"methods":methods, @"ivars":ivars,
              @"windows":windows, @"references":references, @"external_branch_heads":stubs,
              @"exported_code_bytes":@(exportedBytes), @"limits":@{@"code_bytes":@(byteLimit), @"windows":@(windowLimit), @"dependency_depth":@2},
              @"scope":@"Own-process read-only runtime introspection; selected framework methods and bounded dependencies; no service or guest execution"}];
        NSError *error = nil;
        NSData *json = [NSJSONSerialization dataWithJSONObject:result options:NSJSONWritingPrettyPrinted | NSJSONWritingSortedKeys error:&error];
        if (!json || ![json writeToFile:@(argv[1]) options:NSDataWritingAtomic error:&error]) return 8;
        return methods.count && windows.count ? 0 : 9;
    }
}
