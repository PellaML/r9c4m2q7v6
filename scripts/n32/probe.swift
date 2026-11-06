// Public Metal API only. Newly authored synthetic data; no external inputs.
// RGBA32Float (16 B/pixel): compute write(float4) + direct read<float>, CPU/blit
// readback, CPU/blit upload + read, nearest (and, if supported, linear) sampling,
// and diagnostic half4 store/read conversions. NaN/Inf live in a separate
// diagnostic texture and never decide the finite-preservation status.
// API/allocation/pipeline failures are recorded per section, never as numbers.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ c: Bool, _ m: String) throws { if !c { throw Failure(message: m) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func bytes<T>(_ v: [T]) -> Data { v.withUnsafeBytes { Data($0) } }
func h32(_ v: UInt32) -> String { String(format: "0x%08x", v) }
func h16(_ v: UInt16) -> String { String(format: "0x%04x", v) }
func u32(_ d: Data) -> [UInt32] { d.withUnsafeBytes { Array($0.bindMemory(to: UInt32.self)) } }
func u16(_ d: Data) -> [UInt16] { d.withUnsafeBytes { Array($0.bindMemory(to: UInt16.self)) } }
func cls32(_ v: UInt32) -> String {
    let e = (v >> 23) & 0xff, m = v & 0x7fffff
    if e == 0xff { return m == 0 ? "inf" : "nan" }
    if e == 0 { return m == 0 ? "zero" : "subnormal" }
    if v & 0x7fffffff == 0x7f7fffff { return "flt_max" }
    return "normal"
}
func cls16(_ v: UInt16) -> String {
    let e = (v >> 10) & 0x1f, m = v & 0x3ff
    if e == 0x1f { return m == 0 ? "inf" : "nan" }
    if e == 0 { return m == 0 ? "zero" : "subnormal" }
    return "normal"
}

var rngState: UInt64 = 0x9E3779B97F4A7C15
func rnd() -> UInt64 { rngState ^= rngState << 13; rngState ^= rngState >> 7; rngState ^= rngState << 17; return rngState }

let W = 16, H = 16, N = W * H * 4      // main finite texture: 1024 words
let DW = 4, DH = 4, DN = DW * DH * 4   // diagnostic NaN/Inf texture: 64 words
let LW = 4, LH = 4                     // exact dyadic linear-filter texture

let specials: [UInt32] = [
    0x00000000, 0x80000000, 0x00000001, 0x80000001, 0x00000002, 0x80000002, 0x007fffff, 0x807fffff,
    0x00400000, 0x80400000, 0x00012345, 0x80012345, 0x00800000, 0x80800000, 0x00800001, 0x80800001,
    0x7f7fffff, 0xff7fffff, 0x7f7ffffe, 0xff7ffffe, 0x7f000000, 0xff000000, 0x3f800000, 0xbf800000,
    0x3f800001, 0x3f7fffff, 0x40490fdb, 0xc0490fdb, 0x33800000, 0x33000000, 0x33000001, 0x38800000,
    0x387fe000, 0x387fffff, 0x477fe000, 0x477fefff, 0x477ff000, 0x477ff001, 0xc77ff000, 0x3f801000,
    0x3f802000, 0x3f803000, 0x3f801001, 0x3dcccccd, 0xbdcccccd, 0x0000ffff, 0x00ff0000, 0x7effffff]
var mainBits = specials
while mainBits.count < N {
    let r = UInt32(truncatingIfNeeded: rnd() >> 11)
    let v = (mainBits.count % 4 == 3) ? (r & 0x807fffff) : r   // every 4th lane: random subnormal/zero
    if (v >> 23) & 0xff == 0xff { continue }                     // finite only
    mainBits.append(v)
}
var diagBits: [UInt32] = [
    0x7f800000, 0xff800000, 0x7fc00000, 0xffc00000, 0x7f800001, 0xff800001, 0x7fbfffff, 0xffbfffff,
    0x7fffffff, 0xffffffff, 0x7fc00001, 0x7fd23456, 0x7fa00000, 0xffa5a5a5, 0x7f812345, 0xffe00000,
    0x7fc00000, 0x3f800000, 0x7f800000, 0x00000001, 0x3f800000, 0x7f800001, 0x80000000, 0xff800000]
while diagBits.count < DN {
    var v = UInt32(truncatingIfNeeded: rnd() >> 11) | 0x7f800000
    if v & 0x007fffff == 0 { v |= 1 }
    diagBits.append(v)
}
var halfBits: [UInt16] = [
    0x0000, 0x8000, 0x0001, 0x8001, 0x03ff, 0x83ff, 0x0200, 0x0400, 0x8400, 0x7bff, 0xfbff, 0x3c00,
    0xbc00, 0x3c01, 0x3bff, 0x7c00, 0xfc00, 0x7e00, 0xfe00, 0x7c01, 0xfc01, 0x7dff, 0x7fff, 0xffff]
while halfBits.count < N { halfBits.append(UInt16(truncatingIfNeeded: rnd() >> 24)) }
var linF: [Float] = []
for y in 0..<LH { for x in 0..<LW { linF += [Float(8 * x), Float(8 * y), Float(8 * x * y), Float(64 - 8 * x)] } }
var linUV: [SIMD2<Float>] = [], linExp: [Float] = []
for y in 0..<LH { for x in 0..<(LW - 1) { for f: Float in [0.25, 0.5, 0.75] {
    linUV.append(SIMD2((Float(x) + 0.5 + f) / Float(LW), (Float(y) + 0.5) / Float(LH)))
    for c in 0..<4 { let a = linF[(y * LW + x) * 4 + c], b = linF[(y * LW + x + 1) * 4 + c]; linExp.append(a + (b - a) * f) }
} } }
var uvMain: [SIMD2<Float>] = [], uvDiag: [SIMD2<Float>] = []
for y in 0..<H { for x in 0..<W { uvMain.append(SIMD2((Float(x) + 0.5) / Float(W), (Float(y) + 0.5) / Float(H))) } }
for y in 0..<DH { for x in 0..<DW { uvDiag.append(SIMD2((Float(x) + 0.5) / Float(DW), (Float(y) + 0.5) / Float(DH))) } }

func rtz16(_ x: Float) -> UInt16 {
    if x.isNaN { return 0x7e00 }
    var h = Float16(x)
    if h.isInfinite && x.isFinite { h = x > 0 ? Float16.greatestFiniteMagnitude : -Float16.greatestFiniteMagnitude }
    if abs(Float(h)) > abs(x) { h = x > 0 ? h.nextDown : h.nextUp }
    return h.bitPattern
}

func run() throws -> [String: Any] {
    try need(CommandLine.arguments.count == 3, "usage: probe probe.metal outdir")
    let out = URL(fileURLWithPath: CommandLine.arguments[2])
    let source = try String(contentsOfFile: CommandLine.arguments[1], encoding: .utf8)
    guard let device = MTLCreateSystemDefaultDevice() else { throw Failure(message: "UNAVAILABLE: MTLCreateSystemDefaultDevice returned nil") }
    guard let queue = device.makeCommandQueue() else { throw Failure(message: "UNAVAILABLE: no command queue") }
    var res: [String: Any] = [:]
    var dev: [String: Any] = [:]
    dev["name"] = device.name
    dev["registryID"] = device.registryID
    dev["hasUnifiedMemory"] = device.hasUnifiedMemory
    dev["supports32BitFloatFiltering"] = device.supports32BitFloatFiltering
    dev["readWriteTextureSupport"] = device.readWriteTextureSupport.rawValue
    if #available(macOS 14.0, *) {
        dev["architecture"] = device.architecture.name
        var fam: [String: Bool] = [:]
        let fams: [(String, MTLGPUFamily)] = [("apple6", .apple6), ("apple7", .apple7), ("apple8", .apple8), ("apple9", .apple9),
                                              ("mac2", .mac2), ("metal3", .metal3)]
        for (n, f) in fams { fam[n] = device.supportsFamily(f) }
        dev["families"] = fam
    }
    res["device"] = dev
    res["os"] = ProcessInfo.processInfo.operatingSystemVersionString

    func mkbuf(_ len: Int) throws -> MTLBuffer {
        guard let b = device.makeBuffer(length: len, options: .storageModeShared) else { throw Failure(message: "ALLOC: buffer \(len)") }
        memset(b.contents(), 0xa5, len); return b
    }
    func bufFrom<T>(_ a: [T]) throws -> MTLBuffer {
        let mb = a.withUnsafeBytes { device.makeBuffer(bytes: $0.baseAddress!, length: $0.count, options: .storageModeShared) }
        guard let b = mb else { throw Failure(message: "ALLOC: input buffer") }
        return b
    }
    func commit(_ cb: MTLCommandBuffer, _ what: String) throws {
        cb.commit(); cb.waitUntilCompleted()
        try need(cb.status == .completed && cb.error == nil, "COMMAND: \(what) status \(cb.status.rawValue) error \(String(describing: cb.error))")
    }
    func blitToTex(_ b: MTLBuffer, _ t: MTLTexture) throws {
        guard let cb = queue.makeCommandBuffer(), let bl = cb.makeBlitCommandEncoder() else { throw Failure(message: "ENCODER: blit") }
        bl.copy(from: b, sourceOffset: 0, sourceBytesPerRow: t.width * 16, sourceBytesPerImage: t.width * t.height * 16,
                sourceSize: MTLSize(width: t.width, height: t.height, depth: 1), to: t, destinationSlice: 0,
                destinationLevel: 0, destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0))
        bl.endEncoding(); try commit(cb, "blit buffer->texture")
    }
    func blitFromTex(_ t: MTLTexture, _ b: MTLBuffer) throws {
        guard let cb = queue.makeCommandBuffer(), let bl = cb.makeBlitCommandEncoder() else { throw Failure(message: "ENCODER: blit") }
        bl.copy(from: t, sourceSlice: 0, sourceLevel: 0, sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
                sourceSize: MTLSize(width: t.width, height: t.height, depth: 1), to: b, destinationOffset: 0,
                destinationBytesPerRow: t.width * 16, destinationBytesPerImage: t.width * t.height * 16)
        bl.endEncoding(); try commit(cb, "blit texture->buffer")
    }
    func mktex(_ w: Int, _ h: Int, _ mode: MTLStorageMode) throws -> MTLTexture {
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba32Float, width: w, height: h, mipmapped: false)
        td.storageMode = mode; td.usage = [.shaderRead, .shaderWrite]
        guard let t = device.makeTexture(descriptor: td) else { throw Failure(message: "ALLOC: rgba32Float \(w)x\(h) storage \(mode.rawValue)") }
        try need(t.pixelFormat == .rgba32Float && t.pixelFormat.rawValue == 125, "ALLOC: unexpected pixel format \(t.pixelFormat.rawValue)")
        try blitToTex(try mkbuf(w * h * 16), t)   // poison 0xa5a5a5a5
        return t
    }
    func upload<T>(_ t: MTLTexture, _ a: [T]) throws {
        if t.storageMode == .shared {
            a.withUnsafeBytes { t.replace(region: MTLRegionMake2D(0, 0, t.width, t.height), mipmapLevel: 0,
                                          withBytes: $0.baseAddress!, bytesPerRow: t.width * 16) }
        } else { try blitToTex(try bufFrom(a), t) }
    }
    func readback(_ t: MTLTexture) throws -> Data {
        let n = t.width * t.height * 16
        if t.storageMode == .shared {
            var d = Data(repeating: 0xa5, count: n)
            d.withUnsafeMutableBytes { t.getBytes($0.baseAddress!, bytesPerRow: t.width * 16,
                                                 from: MTLRegionMake2D(0, 0, t.width, t.height), mipmapLevel: 0) }
            return d
        }
        let b = try mkbuf(n); try blitFromTex(t, b); return Data(bytes: b.contents(), count: n)
    }
    func sampler(_ f: MTLSamplerMinMagFilter) -> MTLSamplerState? {
        let sd = MTLSamplerDescriptor()
        sd.minFilter = f; sd.magFilter = f; sd.mipFilter = .notMipmapped
        sd.normalizedCoordinates = true; sd.maxAnisotropy = 1
        sd.sAddressMode = .clampToEdge; sd.tAddressMode = .clampToEdge; sd.rAddressMode = .clampToEdge
        return device.makeSamplerState(descriptor: sd)
    }
    let nearest = sampler(.nearest), linear = sampler(.linear)
    res["sampler_alloc"] = ["nearest": nearest != nil, "linear": linear != nil]

    var libInfo: [String: Any] = [:]
    var libs: [(String, MTLLibrary)] = []
    for v in ["default", "safe"] {
        let o = MTLCompileOptions()
        var info: [String: Any] = [:]
        if v == "safe" {
            if #available(macOS 15.0, *) { o.mathMode = .safe } else { info["compile"] = "SKIPPED: mathMode unavailable"; libInfo[v] = info; continue }
        }
        if #available(macOS 15.0, *) { info["mathMode"] = o.mathMode.rawValue }
        do { libs.append((v, try device.makeLibrary(source: source, options: o))); info["compile"] = "ok" }
        catch { info["compile"] = "LIBRARY_ERROR: \(error)" }
        libInfo[v] = info
    }
    res["libraries"] = libInfo
    try need(!libs.isEmpty, "LIBRARY: no library compiled")

    var blobs: [String: Data] = [:]
    var sectionsByTag: [String: [[String: Any]]] = [:]
    var store: [String: Data] = [:]
    var pipelineInfo: [String: [String: String]] = [:]
    let modes: [(String, MTLStorageMode)] = [("shared", .shared), ("private", .private)]
    for (libName, lib) in libs {
        var psos: [String: MTLComputePipelineState] = [:], pinfo: [String: String] = [:]
        for name in ["store_f4", "read_f4", "store_h4", "read_h4", "sample_f4"] {
            guard let f = lib.makeFunction(name: name) else { pinfo[name] = "MISSING_FUNCTION"; continue }
            do { psos[name] = try device.makeComputePipelineState(function: f); pinfo[name] = "ok" }
            catch { pinfo[name] = "PIPELINE_ERROR: \(error)" }
        }
        pipelineInfo[libName] = pinfo
        func pso(_ n: String) throws -> MTLComputePipelineState {
            guard let p = psos[n] else { throw Failure(message: "PIPELINE_UNAVAILABLE: \(n) \(pinfo[n] ?? "")") }
            return p
        }
        func dispatch(_ p: MTLComputePipelineState, _ t: MTLTexture, _ bufs: [Int: MTLBuffer], _ s: MTLSamplerState?, _ w: Int, _ h: Int) throws {
            guard let cb = queue.makeCommandBuffer(), let enc = cb.makeComputeCommandEncoder() else { throw Failure(message: "ENCODER: compute") }
            enc.setComputePipelineState(p)
            enc.setTexture(t, index: 0)
            for (i, b) in bufs { enc.setBuffer(b, offset: 0, index: i) }
            if let s { enc.setSamplerState(s, index: 0) }
            var dims = SIMD2<UInt32>(UInt32(w), UInt32(h))
            enc.setBytes(&dims, length: 8, index: 2)
            let tg = MTLSize(width: 8, height: h == 1 ? 1 : 8, depth: 1)
            enc.dispatchThreadgroups(MTLSize(width: (w + 7) / 8, height: (h + tg.height - 1) / tg.height, depth: 1), threadsPerThreadgroup: tg)
            enc.endEncoding(); try commit(cb, "compute \(p.label ?? "")")
        }
        func readF(_ t: MTLTexture) throws -> Data {
            let n = t.width * t.height * 16, ob = try mkbuf(n)
            try dispatch(try pso("read_f4"), t, [1: ob], nil, t.width, t.height); return Data(bytes: ob.contents(), count: n)
        }
        func readHalf(_ t: MTLTexture) throws -> Data {
            let n = t.width * t.height * 8, ob = try mkbuf(n)
            try dispatch(try pso("read_h4"), t, [1: ob], nil, t.width, t.height); return Data(bytes: ob.contents(), count: n)
        }
        func sample(_ t: MTLTexture, _ s: MTLSamplerState?, _ uv: [SIMD2<Float>]) throws -> Data {
            guard let s else { throw Failure(message: "ALLOC: sampler state nil") }
            let ob = try mkbuf(uv.count * 16)
            try dispatch(try pso("sample_f4"), t, [0: try bufFrom(uv), 1: ob], s, uv.count, 1)
            return Data(bytes: ob.contents(), count: uv.count * 16)
        }
        for (modeName, mode) in modes {
            let tag = "\(libName)-\(modeName)"
            var blob = Data(), secs: [[String: Any]] = []
            func section(_ name: String, _ elemBytes: Int, _ count: Int, _ body: () throws -> Data) {
                var d: Data, status = "ok"
                do { d = try body() } catch let e as Failure { status = e.message; d = Data() } catch { status = "ERROR: \(error)"; d = Data() }
                if d.count != elemBytes * count {
                    if status == "ok" { status = "SIZE_MISMATCH \(d.count)" }
                    d = Data(repeating: 0xa5, count: elemBytes * count)
                }
                secs.append(["name": name, "offset": blob.count, "bytes": d.count, "elem_bytes": elemBytes, "count": count, "status": status, "sha256": sha(d)])
                blob.append(d); store["\(tag)/\(name)"] = d
            }
            var tMain: MTLTexture? = nil, tDiag: MTLTexture? = nil
            func tex(_ t: MTLTexture?) throws -> MTLTexture { guard let t else { throw Failure(message: "DEPENDENCY: store texture unavailable") }; return t }
            section("f32_store_read_main", 4, N) {
                let t = try mktex(W, H, mode); tMain = t
                try dispatch(try pso("store_f4"), t, [0: try bufFrom(mainBits)], nil, W, H); return try readF(t)
            }
            section("f32_store_read_diag", 4, DN) {
                let t = try mktex(DW, DH, mode); tDiag = t
                try dispatch(try pso("store_f4"), t, [0: try bufFrom(diagBits)], nil, DW, DH); return try readF(t)
            }
            section("f32_store_readback_main", 4, N) { try readback(try tex(tMain)) }
            section("f32_store_readback_diag", 4, DN) { try readback(try tex(tDiag)) }
            section("upload_read_main", 4, N) { let t = try mktex(W, H, mode); try upload(t, mainBits); return try readF(t) }
            section("upload_read_diag", 4, DN) { let t = try mktex(DW, DH, mode); try upload(t, diagBits); return try readF(t) }
            section("nearest_centers_main", 4, N) { try sample(try tex(tMain), nearest, uvMain) }
            section("nearest_centers_diag", 4, DN) { try sample(try tex(tDiag), nearest, uvDiag) }
            section("linear_centers_main", 4, N) {
                guard device.supports32BitFloatFiltering else { throw Failure(message: "SKIPPED_UNSUPPORTED: supports32BitFloatFiltering=false") }
                return try sample(try tex(tMain), linear, uvMain)
            }
            section("linear_points_lin", 4, linExp.count) {
                guard device.supports32BitFloatFiltering else { throw Failure(message: "SKIPPED_UNSUPPORTED: supports32BitFloatFiltering=false") }
                let t = try mktex(LW, LH, mode); try upload(t, linF); return try sample(t, linear, linUV)
            }
            var tHalf: MTLTexture? = nil
            section("h16_store_f32_read", 4, N) {
                let t = try mktex(W, H, mode); tHalf = t
                try dispatch(try pso("store_h4"), t, [0: try bufFrom(halfBits)], nil, W, H); return try readF(t)
            }
            section("h16_store_h16_read", 2, N) { try readHalf(try tex(tHalf)) }
            section("f32_store_h16_read_main", 2, N) { try readHalf(try tex(tMain)) }
            section("f32_store_h16_read_diag", 2, DN) { try readHalf(try tex(tDiag)) }
            blobs[tag] = blob; sectionsByTag[tag] = secs
        }
    }
    res["pipelines"] = pipelineInfo
    res["sections"] = sectionsByTag

    // ---------- analysis (raw output is the oracle) ----------
    func statusOf(_ tag: String, _ name: String) -> String {
        (sectionsByTag[tag]?.first(where: { ($0["name"] as? String) == name })?["status"] as? String) ?? "MISSING"
    }
    func exact(_ tag: String, _ name: String, _ expect: [UInt32]) -> [String: Any] {
        let st = statusOf(tag, name)
        guard st == "ok", let d = store["\(tag)/\(name)"] else { return ["status": st] }
        let g = u32(d)
        var mis = 0, poison = 0, byClass: [String: Int] = [:], ex: [[String: String]] = []
        for i in 0..<expect.count where g[i] != expect[i] {
            mis += 1; if g[i] == 0xa5a5a5a5 { poison += 1 }
            byClass[cls32(expect[i]), default: 0] += 1
            if ex.count < 24 { ex.append(["i": "\(i)", "in": h32(expect[i]), "got": h32(g[i]), "in_class": cls32(expect[i]), "got_class": cls32(g[i])]) }
        }
        return ["status": st, "mismatches": mis, "poisoned": poison, "mismatch_by_input_class": byClass, "examples": ex]
    }
    func trans32(_ tag: String, _ name: String, _ input: [UInt32]) -> [String: Any] {
        let st = statusOf(tag, name)
        guard st == "ok", let d = store["\(tag)/\(name)"] else { return ["status": st] }
        let g = u32(d)
        var same = 0, seen = Set<String>(), ch: [[String]] = []
        for i in 0..<input.count {
            if g[i] == input[i] { same += 1; continue }
            let k = "\(input[i])>\(g[i])"
            if !seen.contains(k) && ch.count < 64 { seen.insert(k); ch.append([h32(input[i]), h32(g[i]), cls32(input[i]), cls32(g[i])]) }
        }
        return ["status": st, "identical": same, "changed": input.count - same, "changes_in_got_class": ch]
    }
    let classCount = Dictionary(grouping: mainBits, by: cls32).mapValues { $0.count }
    res["main_input_classes"] = classCount
    var analysis: [String: Any] = [:]
    var finiteOK = true
    for tag in sectionsByTag.keys.sorted() {
        var a: [String: Any] = [:]
        for name in ["f32_store_read_main", "f32_store_readback_main", "upload_read_main", "nearest_centers_main"] {
            let r = exact(tag, name, mainBits); a[name] = r
            if (r["status"] as? String) != "ok" || (r["mismatches"] as? Int) != 0 { finiteOK = false }
        }
        a["linear_centers_main"] = exact(tag, "linear_centers_main", mainBits)
        a["linear_points_lin"] = exact(tag, "linear_points_lin", linExp.map { $0.bitPattern })
        for name in ["f32_store_read_diag", "f32_store_readback_diag", "upload_read_diag", "nearest_centers_diag"] {
            a[name] = trans32(tag, name, diagBits)
        }
        // half4 store into RGBA32Float, read as float: expected exact widening (non-NaN); NaN reported raw.
        if statusOf(tag, "h16_store_f32_read") == "ok", let d = store["\(tag)/h16_store_f32_read"] {
            let g = u32(d); var mis = 0, nanIn = 0, nanOutNaN = 0, ex: [[String: String]] = [], nanEx: [[String]] = []
            for i in 0..<N {
                let e = Float(Float16(bitPattern: halfBits[i])).bitPattern
                if cls16(halfBits[i]) == "nan" {
                    nanIn += 1; if cls32(g[i]) == "nan" { nanOutNaN += 1 }
                    if nanEx.count < 32 { nanEx.append([h16(halfBits[i]), h32(g[i])]) }
                } else if g[i] != e {
                    mis += 1; if ex.count < 24 { ex.append(["in16": h16(halfBits[i]), "got": h32(g[i]), "exact_widen": h32(e)]) }
                }
            }
            a["h16_store_f32_read"] = ["status": "ok", "non_nan_mismatch_vs_exact_widen": mis, "examples": ex, "nan_inputs": nanIn, "nan_outputs_still_nan": nanOutNaN, "nan_examples_in16_got32": nanEx] as [String: Any]
        } else { a["h16_store_f32_read"] = ["status": statusOf(tag, "h16_store_f32_read")] }
        if statusOf(tag, "h16_store_h16_read") == "ok", let d = store["\(tag)/h16_store_h16_read"] {
            let g = u16(d); var mis = 0, nanChanged = 0, ex: [[String]] = []
            for i in 0..<N where g[i] != halfBits[i] {
                if cls16(halfBits[i]) == "nan" { nanChanged += 1 } else { mis += 1 }
                if ex.count < 32 { ex.append([h16(halfBits[i]), h16(g[i])]) }
            }
            a["h16_store_h16_read"] = ["status": "ok", "non_nan_changed": mis, "nan_changed": nanChanged, "examples_in_got": ex] as [String: Any]
        } else { a["h16_store_h16_read"] = ["status": statusOf(tag, "h16_store_h16_read")] }
        // float stored, read through texture2d<half>: compare to RTNE and RTZ host models (diagnostic only).
        if statusOf(tag, "f32_store_h16_read_main") == "ok", let d = store["\(tag)/f32_store_h16_read_main"] {
            let g = u16(d); var rtne = 0, rtz = 0, byCat: [String: [Int]] = [:], ex: [[String: String]] = []
            for i in 0..<N {
                let x = Float(bitPattern: mainBits[i]), en = Float16(x).bitPattern, ez = rtz16(x)
                let ax = abs(x)
                let cat = Float(Float16(x)) == x ? "exact" : (ax >= 65520 ? "overflow_ge65520" : (ax > 65504 ? "between65504_65520" : (ax < 6.103515625e-05 ? "below_half_min_normal" : "rounded_normal")))
                var c = byCat[cat] ?? [0, 0, 0]; c[0] += 1
                if g[i] != en { rtne += 1; c[1] += 1 }
                if g[i] != ez { rtz += 1; c[2] += 1 }
                byCat[cat] = c
                if g[i] != en && ex.count < 32 { ex.append(["in": h32(mainBits[i]), "got16": h16(g[i]), "rtne": h16(en), "rtz": h16(ez), "cat": cat]) }
            }
            a["f32_store_h16_read_main"] = ["status": "ok", "mismatch_vs_rtne": rtne, "mismatch_vs_rtz": rtz,
                                            "by_category_count_rtneMis_rtzMis": byCat, "examples_not_rtne": ex] as [String: Any]
        } else { a["f32_store_h16_read_main"] = ["status": statusOf(tag, "f32_store_h16_read_main")] }
        if statusOf(tag, "f32_store_h16_read_diag") == "ok", let d = store["\(tag)/f32_store_h16_read_diag"] {
            let g = u16(d)
            a["f32_store_h16_read_diag"] = ["status": "ok", "pairs_in32_got16": (0..<DN).map { [h32(diagBits[$0]), h16(g[$0])] }] as [String: Any]
        } else { a["f32_store_h16_read_diag"] = ["status": statusOf(tag, "f32_store_h16_read_diag")] }
        analysis[tag] = a
    }
    res["analysis"] = analysis
    // cross-variant byte identity per section
    var cross: [String: Any] = [:]
    if let first = sectionsByTag.keys.sorted().first, let secs = sectionsByTag[first] {
        for s in secs {
            let name = s["name"] as! String
            var hashes: [String: String] = [:]
            for tag in sectionsByTag.keys.sorted() { if let d = store["\(tag)/\(name)"] { hashes[tag] = sha(d) } }
            cross[name] = ["all_identical": Set(hashes.values).count == 1, "sha256": hashes] as [String: Any]
        }
    }
    res["cross_variant"] = cross

    var files: [String: String] = [:]
    var uvAll = uvMain; uvAll += uvDiag; uvAll += linUV
    var outs: [(String, Data)] = [("in-main-u32.bin", bytes(mainBits)), ("in-diag-u32.bin", bytes(diagBits)),
                                  ("in-half-u16.bin", bytes(halfBits)), ("in-lin-f32.bin", bytes(linF)),
                                  ("uv-main-diag-lin-f32x2.bin", bytes(uvAll))]
    for tag in blobs.keys.sorted() { outs.append(("out-\(tag).bin", blobs[tag]!)) }
    for (name, d) in outs { try d.write(to: out.appendingPathComponent(name)); files[name] = sha(d) }
    res["files_sha256"] = files
    res["layout"] = "out-<lib>-<storage>.bin = concatenated sections (see sections[].offset/bytes); u32/u16 little-endian raw bits; main texel (x,y) lanes at ((y*16+x)*4+c); uv file = 256 main centers, 16 diag centers, \(linUV.count) linear points"
    res["dims"] = ["main": [W, H], "diag": [DW, DH], "lin": [LW, LH]]
    res["status"] = finiteOK ? "FINITE_BITS_PRESERVED_ALL_REQUIRED_PATHS" : "FINITE_MISMATCH_OR_SECTION_FAILURE_SEE_ANALYSIS"
    return res
}
let outDir = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "."
var result: [String: Any]
var code: Int32 = 0
do { result = try run() } catch let e as Failure {
    let kind = e.message.hasPrefix("UNAVAILABLE") ? "UNAVAILABLE_NOT_PASS" : "FAILED_NOT_PASS"
    result = ["status": kind, "error": e.message]; code = 3
} catch { result = ["status": "FAILED_NOT_PASS", "error": "\(error)"]; code = 3 }
let json = try! JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
try! json.write(to: URL(fileURLWithPath: outDir).appendingPathComponent("summary.json"))
FileHandle.standardOutput.write(json)
exit(code)