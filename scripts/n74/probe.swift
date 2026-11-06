// Public Metal API only. Newly authored synthetic data; no external inputs.
// Priority cohort: RGBA32Float sampled with a "sampler74"-shaped state:
// nearest/nearest, notMipmapped (mip 0), UNNORMALIZED coordinates, clampToZero
// S/T/R, maxAnisotropy 1, compare never; versus a normalized clampToEdge
// nearest comparison sampler. Plus a guessed two-pass integral image (sequential
// f32 load/add/store), checked against host RTNE models and for finiteness.
// API/allocation/pipeline failures are recorded per section, never as numbers.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ c: Bool, _ m: String) throws { if !c { throw Failure(message: m) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func bytes<T>(_ v: [T]) -> Data { v.withUnsafeBytes { Data($0) } }
func h32(_ v: UInt32) -> String { String(format: "0x%08x", v) }
func u32(_ d: Data) -> [UInt32] { d.withUnsafeBytes { Array($0.bindMemory(to: UInt32.self)) } }
func cls32(_ v: UInt32) -> String {
    let e = (v >> 23) & 0xff, m = v & 0x7fffff
    if e == 0xff { return m == 0 ? "inf" : "nan" }
    if e == 0 { return m == 0 ? "zero" : "subnormal" }
    if v & 0x7fffffff == 0x7f7fffff { return "flt_max" }
    return "normal"
}

var rngState: UInt64 = 0x9E3779B97F4A7C15
func rnd() -> UInt64 { rngState ^= rngState << 13; rngState ^= rngState >> 7; rngState ^= rngState << 17; return rngState }

let W = 16, H = 16, N = W * H * 4
// A: arbitrary finite bits (same generator/seed as n32 main set): +-0, +-subnormal, +-FLT_MAX, random.
let specials: [UInt32] = [
    0x00000000, 0x80000000, 0x00000001, 0x80000001, 0x00000002, 0x80000002, 0x007fffff, 0x807fffff,
    0x00400000, 0x80400000, 0x00012345, 0x80012345, 0x00800000, 0x80800000, 0x00800001, 0x80800001,
    0x7f7fffff, 0xff7fffff, 0x7f7ffffe, 0xff7ffffe, 0x7f000000, 0xff000000, 0x3f800000, 0xbf800000,
    0x3f800001, 0x3f7fffff, 0x40490fdb, 0xc0490fdb, 0x33800000, 0x33000000, 0x33000001, 0x38800000,
    0x387fe000, 0x387fffff, 0x477fe000, 0x477fefff, 0x477ff000, 0x477ff001, 0xc77ff000, 0x3f801000,
    0x3f802000, 0x3f803000, 0x3f801001, 0x3dcccccd, 0xbdcccccd, 0x0000ffff, 0x00ff0000, 0x7effffff]
var dataA = specials
while dataA.count < N {
    let r = UInt32(truncatingIfNeeded: rnd() >> 11)
    let v = (dataA.count % 4 == 3) ? (r & 0x807fffff) : r
    if (v >> 23) & 0xff == 0xff { continue }
    dataA.append(v)
}
// B1: moderate arbitrary finite values, magnitudes 2^-8..2^13, both signs, full 23-bit mantissas (outside [0,1]).
var dataB1: [UInt32] = []
while dataB1.count < N {
    let r = rnd()
    let e = UInt32(119 + Int(r % 21)), m = UInt32(truncatingIfNeeded: r >> 8) & 0x7fffff, s = UInt32(truncatingIfNeeded: r >> 40) & 1
    dataB1.append((s << 31) | (e << 23) | m)
}
// B3: tiny values (subnormals and normals near FLT_MIN), both signs: probes add underflow/flush behavior.
var dataB3: [UInt32] = []
while dataB3.count < N {
    let r = rnd()
    let e = UInt32(r % 4), m = UInt32(truncatingIfNeeded: r >> 8) & 0x7fffff, s = UInt32(truncatingIfNeeded: r >> 40) & 1
    dataB3.append((s << 31) | (e << 23) | m)
}
// Pixel-unit coordinate axis: every texel edge k (0..16) and +-1 ulp, every center, out-of-range and signed zero.
var axis: [Float] = []
for k in 0...16 { let e = Float(k); axis += [e.nextDown, e, e.nextUp] }
for k in 0..<16 { axis.append(Float(k) + 0.5) }
axis += [-0.0, -1e-30, -0.5, -1, 16.5, 17, 1e6, -1e6]
var coordPx: [SIMD2<Float>] = []
for v in axis { for u in axis { coordPx.append(SIMD2(u, v)) } }
let coordNorm = coordPx.map { SIMD2($0.x / Float(W), $0.y / Float(H)) }   // exact power-of-two scaling
let NC = coordPx.count

func flushF(_ x: Float) -> Float { x.isSubnormal ? (x.sign == .minus ? -0.0 : 0.0) : x }
// host models of the two passes; ftz=true flushes inputs and results of each add (sign-preserving).
func integral(_ bits: [UInt32], ftz: Bool) -> (p1: [UInt32], p2: [UInt32]) {
    var p1 = [Float](repeating: 0, count: N), p2 = [Float](repeating: 0, count: N)
    for y in 0..<H { for c in 0..<4 {
        var acc: Float = 0
        for x in 0..<W {
            let v = Float(bitPattern: bits[(y * W + x) * 4 + c])
            acc = ftz ? flushF(flushF(acc) + flushF(v)) : acc + v
            p1[(y * W + x) * 4 + c] = acc
        }
    } }
    for x in 0..<W { for c in 0..<4 {
        var acc: Float = 0
        for y in 0..<H {
            let v = p1[(y * W + x) * 4 + c]
            acc = ftz ? flushF(flushF(acc) + flushF(v)) : acc + v
            p2[(y * W + x) * 4 + c] = acc
        }
    } }
    return (p1.map { $0.bitPattern }, p2.map { $0.bitPattern })
}
func texelIndex(_ c: Float, _ size: Int, zeroBorder: Bool) -> Int? {
    let t = c.rounded(.down)
    if zeroBorder { if !(t >= 0 && t < Float(size)) { return nil }; return Int(t) }
    if t.isNaN { return 0 }
    return Int(min(max(t, 0), Float(size - 1)))
}

func run() throws -> [String: Any] {
    try need(CommandLine.arguments.count == 3, "usage: probe srcdir outdir")
    let srcDir = URL(fileURLWithPath: CommandLine.arguments[1])
    let out = URL(fileURLWithPath: CommandLine.arguments[2])
    guard let device = MTLCreateSystemDefaultDevice() else { throw Failure(message: "UNAVAILABLE: MTLCreateSystemDefaultDevice returned nil") }
    guard let queue = device.makeCommandQueue() else { throw Failure(message: "UNAVAILABLE: no command queue") }
    var res: [String: Any] = [:]
    var dev: [String: Any] = [:]
    dev["name"] = device.name
    dev["registryID"] = device.registryID
    dev["hasUnifiedMemory"] = device.hasUnifiedMemory
    dev["supports32BitFloatFiltering"] = device.supports32BitFloatFiltering
    if #available(macOS 14.0, *) { dev["architecture"] = device.architecture.name }
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
    func mktex(_ mode: MTLStorageMode) throws -> MTLTexture {
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba32Float, width: W, height: H, mipmapped: false)
        td.storageMode = mode; td.usage = [.shaderRead, .shaderWrite]
        guard let t = device.makeTexture(descriptor: td) else { throw Failure(message: "ALLOC: rgba32Float storage \(mode.rawValue)") }
        try need(t.pixelFormat.rawValue == 125, "ALLOC: unexpected pixel format \(t.pixelFormat.rawValue)")
        let p = try mkbuf(W * H * 16)
        guard let cb = queue.makeCommandBuffer(), let bl = cb.makeBlitCommandEncoder() else { throw Failure(message: "ENCODER: blit") }
        bl.copy(from: p, sourceOffset: 0, sourceBytesPerRow: W * 16, sourceBytesPerImage: W * H * 16,
                sourceSize: MTLSize(width: W, height: H, depth: 1), to: t, destinationSlice: 0,
                destinationLevel: 0, destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0))
        bl.endEncoding(); try commit(cb, "poison blit")
        return t
    }

    // ---- samplers ----
    var samplerInfo: [String: Any] = [:]
    func mkSampler(_ name: String, normalized: Bool, mode: MTLSamplerAddressMode) -> MTLSamplerState? {
        let sd = MTLSamplerDescriptor()
        sd.minFilter = .nearest; sd.magFilter = .nearest; sd.mipFilter = .notMipmapped
        sd.normalizedCoordinates = normalized; sd.maxAnisotropy = 1
        sd.sAddressMode = mode; sd.tAddressMode = mode; sd.rAddressMode = mode
        sd.compareFunction = .never
        let s = device.makeSamplerState(descriptor: sd)
        samplerInfo[name] = ["allocated": s != nil, "min": sd.minFilter.rawValue, "mag": sd.magFilter.rawValue, "mip": sd.mipFilter.rawValue,
                             "normalized": sd.normalizedCoordinates, "s": sd.sAddressMode.rawValue, "t": sd.tAddressMode.rawValue,
                             "r": sd.rAddressMode.rawValue, "max_anisotropy": sd.maxAnisotropy, "compare": sd.compareFunction.rawValue,
                             "lod_min": sd.lodMinClamp, "lod_max_is_flt_max": sd.lodMaxClamp == Float.greatestFiniteMagnitude] as [String: Any]
        return s
    }
    let s74 = mkSampler("sampler74_px_clampToZero", normalized: false, mode: .clampToZero)
    let sEdge = mkSampler("normalized_clampToEdge", normalized: true, mode: .clampToEdge)
    res["samplers"] = samplerInfo

    // ---- libraries: base + two isolated pixel-sampling spellings, each default and safe math ----
    var libInfo: [String: Any] = [:]
    var libs: [String: MTLLibrary] = [:]
    for file in ["base", "px_nolod", "px_level0"] { for v in ["default", "safe"] {
        let key = "\(file)/\(v)"
        var info: [String: Any] = [:]
        let o = MTLCompileOptions()
        if v == "safe" {
            if #available(macOS 15.0, *) { o.mathMode = .safe } else { libInfo[key] = ["compile": "SKIPPED: mathMode unavailable"]; continue }
        }
        if #available(macOS 15.0, *) { info["mathMode"] = o.mathMode.rawValue }
        do {
            let src = try String(contentsOf: srcDir.appendingPathComponent("\(file).metal"), encoding: .utf8)
            libs[key] = try device.makeLibrary(source: src, options: o); info["compile"] = "ok"
        } catch { info["compile"] = "LIBRARY_ERROR: \(error)" }
        libInfo[key] = info
    } }
    res["libraries"] = libInfo
    var psos: [String: MTLComputePipelineState] = [:], pinfo: [String: String] = [:]
    let fnsByFile = ["base": ["store_f4", "read_f4", "sample_norm_level0", "integral_rows", "integral_cols_read"],
                     "px_nolod": ["sample_px", "integral_cols_px"], "px_level0": ["sample_px", "integral_cols_px"]]
    for (key, lib) in libs {
        let file = String(key.split(separator: "/")[0])
        for fn in fnsByFile[file] ?? [] {
            let pk = "\(key)/\(fn)"
            guard let f = lib.makeFunction(name: fn) else { pinfo[pk] = "MISSING_FUNCTION"; continue }
            do { psos[pk] = try device.makeComputePipelineState(function: f); pinfo[pk] = "ok" }
            catch { pinfo[pk] = "PIPELINE_ERROR: \(error)" }
        }
    }
    res["pipelines"] = pinfo
    func pso(_ k: String) throws -> MTLComputePipelineState {
        guard let p = psos[k] else { throw Failure(message: "PIPELINE_UNAVAILABLE: \(k) \(pinfo[k] ?? "library not compiled")") }
        return p
    }
    func dispatch(_ p: MTLComputePipelineState, _ tex: [MTLTexture], _ bufs: [Int: MTLBuffer], _ s: MTLSamplerState?,
                  grid: (Int, Int), dims: (Int, Int)) throws {
        guard let cb = queue.makeCommandBuffer(), let enc = cb.makeComputeCommandEncoder() else { throw Failure(message: "ENCODER: compute") }
        enc.setComputePipelineState(p)
        for (i, t) in tex.enumerated() { enc.setTexture(t, index: i) }
        for (i, b) in bufs { enc.setBuffer(b, offset: 0, index: i) }
        if let s { enc.setSamplerState(s, index: 0) }
        var d = SIMD2<UInt32>(UInt32(dims.0), UInt32(dims.1))
        enc.setBytes(&d, length: 8, index: 2)
        let th = grid.1 == 1 ? 1 : 8
        enc.dispatchThreadgroups(MTLSize(width: (grid.0 + 7) / 8, height: (grid.1 + th - 1) / th, depth: 1),
                                 threadsPerThreadgroup: MTLSize(width: 8, height: th, depth: 1))
        enc.endEncoding(); try commit(cb, "compute")
    }

    var blobs: [String: Data] = [:]
    var sectionsByTag: [String: [[String: Any]]] = [:]
    var store: [String: Data] = [:]
    let integrals: [(String, [UInt32])] = [("B1", dataB1), ("A", dataA), ("B3", dataB3)]
    for v in ["default", "safe"] { let modes: [(String, MTLStorageMode)] = [("shared", .shared), ("private", .private)]
    for (modeName, mode) in modes {
        let tag = "\(v)-\(modeName)"
        var blob = Data(), secs: [[String: Any]] = []
        func section(_ name: String, _ count: Int, _ body: () throws -> Data) {
            var d: Data, status = "ok"
            do { d = try body() } catch let e as Failure { status = e.message; d = Data() } catch { status = "ERROR: \(error)"; d = Data() }
            if d.count != 4 * count {
                if status == "ok" { status = "SIZE_MISMATCH \(d.count)" }
                d = Data(repeating: 0xa5, count: 4 * count)
            }
            secs.append(["name": name, "offset": blob.count, "bytes": d.count, "count_u32": count, "status": status, "sha256": sha(d)])
            blob.append(d); store["\(tag)/\(name)"] = d
        }
        func readF(_ t: MTLTexture) throws -> Data {
            let ob = try mkbuf(N * 4)
            try dispatch(try pso("base/\(v)/read_f4"), [t], [1: ob], nil, grid: (W, H), dims: (W, H))
            return Data(bytes: ob.contents(), count: N * 4)
        }
        func storeTex(_ bits: [UInt32]) throws -> MTLTexture {
            let t = try mktex(mode)
            try dispatch(try pso("base/\(v)/store_f4"), [t], [0: try bufFrom(bits)], nil, grid: (W, H), dims: (W, H))
            return t
        }
        func sample(_ k: String, _ t: MTLTexture, _ s: MTLSamplerState?, _ uv: [SIMD2<Float>]) throws -> Data {
            guard let s else { throw Failure(message: "ALLOC: sampler state nil") }
            let ob = try mkbuf(uv.count * 16)
            try dispatch(try pso(k), [t], [0: try bufFrom(uv), 1: ob], s, grid: (uv.count, 1), dims: (uv.count, 0))
            return Data(bytes: ob.contents(), count: uv.count * 16)
        }
        var tA: MTLTexture? = nil
        func texA() throws -> MTLTexture { guard let t = tA else { throw Failure(message: "DEPENDENCY: A texture unavailable") }; return t }
        section("A_store_read", N) { let t = try storeTex(dataA); tA = t; return try readF(t) }
        section("A_s74_px_nolod", NC * 4) { try sample("px_nolod/\(v)/sample_px", try texA(), s74, coordPx) }
        section("A_s74_px_level0", NC * 4) { try sample("px_level0/\(v)/sample_px", try texA(), s74, coordPx) }
        section("A_edge_norm_level0", NC * 4) { try sample("base/\(v)/sample_norm_level0", try texA(), sEdge, coordNorm) }
        for (dn, bits) in integrals {
            var tIn: MTLTexture? = nil, tP1: MTLTexture? = nil
            func dep(_ t: MTLTexture?) throws -> MTLTexture { guard let t else { throw Failure(message: "DEPENDENCY: integral texture unavailable") }; return t }
            section("\(dn)_input_store_read", N) { let t = try storeTex(bits); tIn = t; return try readF(t) }
            section("\(dn)_pass1_rows", N) {
                let t = try mktex(mode); tP1 = t
                try dispatch(try pso("base/\(v)/integral_rows"), [try dep(tIn), t], [:], nil, grid: (H, 1), dims: (W, H))
                return try readF(t)
            }
            section("\(dn)_pass2_cols_read", N) {
                let t = try mktex(mode)
                try dispatch(try pso("base/\(v)/integral_cols_read"), [try dep(tP1), t], [:], nil, grid: (W, 1), dims: (W, H))
                return try readF(t)
            }
            for sp in ["px_nolod", "px_level0"] {
                section("\(dn)_pass2_cols_s74_\(sp)", N) {
                    guard let s = s74 else { throw Failure(message: "ALLOC: sampler74 nil") }
                    let t = try mktex(mode)
                    try dispatch(try pso("\(sp)/\(v)/integral_cols_px"), [try dep(tP1), t], [:], s, grid: (W, 1), dims: (W, H))
                    return try readF(t)
                }
            }
        }
        blobs[tag] = blob; sectionsByTag[tag] = secs
    } }
    res["sections"] = sectionsByTag

    // ---------- analysis (raw output is the oracle) ----------
    func get(_ tag: String, _ name: String) -> (String, [UInt32]?) {
        let st = (sectionsByTag[tag]?.first(where: { ($0["name"] as? String) == name })?["status"] as? String) ?? "MISSING"
        guard st == "ok", let d = store["\(tag)/\(name)"] else { return (st, nil) }
        return (st, u32(d))
    }
    func compare(_ g: [UInt32], _ e: [UInt32], _ inputClass: (Int) -> String) -> [String: Any] {
        var mis = 0, poison = 0, byClass: [String: Int] = [:], ex: [[String: String]] = []
        for i in 0..<e.count where g[i] != e[i] {
            mis += 1; if g[i] == 0xa5a5a5a5 { poison += 1 }
            byClass[inputClass(i), default: 0] += 1
            if ex.count < 24 { ex.append(["i": "\(i)", "expected": h32(e[i]), "got": h32(g[i]), "class": inputClass(i)]) }
        }
        return ["mismatches": mis, "poisoned": poison, "mismatch_by_class": byClass, "examples": ex]
    }
    // expected sampling outputs
    func expectSample(zero: Bool) -> [UInt32] {
        var e: [UInt32] = []
        for c in coordPx {
            if let ix = texelIndex(c.x, W, zeroBorder: zero), let iy = texelIndex(c.y, H, zeroBorder: zero) {
                e += Array(dataA[(iy * W + ix) * 4..<(iy * W + ix) * 4 + 4])
            } else { e += [0, 0, 0, 0] }
        }
        return e
    }
    let e74 = expectSample(zero: true), eEdge = expectSample(zero: false)
    func coordClass(_ i: Int) -> String {
        let c = coordPx[i / 4]
        let inX = c.x.rounded(.down) >= 0 && c.x.rounded(.down) < Float(W), inY = c.y.rounded(.down) >= 0 && c.y.rounded(.down) < Float(H)
        let k = (inX && inY) ? "in_range" : "border"
        let lane = (inX && inY) ? cls32(e74[i]) : "zero_border"
        return "\(k)/\(lane)"
    }
    let ftzModels = integrals.map { ($0.0, integral($0.1, ftz: false), integral($0.1, ftz: true)) }
    var analysis: [String: Any] = [:]
    var priorityOK = true
    for tag in sectionsByTag.keys.sorted() {
        var a: [String: Any] = [:]
        let (st0, g0) = get(tag, "A_store_read")
        if let g0 { var r = compare(g0, dataA, { cls32(dataA[$0]) }); r["status"] = st0; a["A_store_read"] = r } else { a["A_store_read"] = ["status": st0] }
        if g0 == nil || (compare(g0!, dataA, { _ in "" })["mismatches"] as? Int) != 0 { priorityOK = false }
        var s74ok = false
        for (name, e) in [("A_s74_px_nolod", e74), ("A_s74_px_level0", e74), ("A_edge_norm_level0", eEdge)] {
            let (st, g) = get(tag, name)
            if let g {
                var r = compare(g, e, coordClass); r["status"] = st
                // border outputs observed, independent of the model
                var border: [String: Int] = [:]
                for i in 0..<NC { let c = coordPx[i]
                    let inR = c.x.rounded(.down) >= 0 && c.x.rounded(.down) < Float(W) && c.y.rounded(.down) >= 0 && c.y.rounded(.down) < Float(H)
                    if !inR { border[[g[i*4], g[i*4+1], g[i*4+2], g[i*4+3]].map { h32($0) }.joined(separator: ","), default: 0] += 1 } }
                r["border_outputs_rgba_bits_count"] = border
                if name == "A_edge_norm_level0" {
                    var vs74 = 0; for i in 0..<NC * 4 where g[i] != e74[i] { vs74 += 1 }
                    r["lanes_differing_from_clampToZero_model"] = vs74
                }
                a[name] = r
                if name != "A_edge_norm_level0" && (r["mismatches"] as? Int) == 0 { s74ok = true }
            } else { a[name] = ["status": st] }
        }
        if !s74ok { priorityOK = false }
        for (dn, rtne, ftz) in ftzModels {
            let bits = integrals.first(where: { $0.0 == dn })!.1
            let (sti, gi) = get(tag, "\(dn)_input_store_read")
            if let gi { var r = compare(gi, bits, { cls32(bits[$0]) }); r["status"] = sti; a["\(dn)_input_store_read"] = r } else { a["\(dn)_input_store_read"] = ["status": sti] }
            for (name, eR, eF) in [("\(dn)_pass1_rows", rtne.p1, ftz.p1), ("\(dn)_pass2_cols_read", rtne.p2, ftz.p2),
                                   ("\(dn)_pass2_cols_s74_px_nolod", rtne.p2, ftz.p2), ("\(dn)_pass2_cols_s74_px_level0", rtne.p2, ftz.p2)] {
                let (st, g) = get(tag, name)
                guard let g else { a[name] = ["status": st]; continue }
                var r: [String: Any] = ["status": st]
                r["vs_host_rtne_ieee"] = compare(g, eR, { cls32(eR[$0]) })
                r["vs_host_rtne_daz_ftz"] = compare(g, eF, { cls32(eF[$0]) })
                r["nonfinite_outputs"] = g.filter { let c = cls32($0); return c == "inf" || c == "nan" }.count
                r["nonfinite_expected_ieee"] = eR.filter { let c = cls32($0); return c == "inf" || c == "nan" }.count
                if name.hasPrefix("\(dn)_pass2_cols_s74"), let gr = get(tag, "\(dn)_pass2_cols_read").1 {
                    r["lanes_differing_from_cols_read"] = zip(g, gr).filter { $0 != $1 }.count
                }
                a[name] = r
                if dn == "B1" && (name.hasSuffix("rows") || name.hasSuffix("cols_read")) {
                    if ((r["vs_host_rtne_ieee"] as? [String: Any])?["mismatches"] as? Int) != 0 || (r["nonfinite_outputs"] as? Int) != 0 { priorityOK = false }
                }
            }
        }
        analysis[tag] = a
    }
    res["analysis"] = analysis
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
    var outs: [(String, Data)] = [("in-A-u32.bin", bytes(dataA)), ("in-B1-u32.bin", bytes(dataB1)), ("in-B3-u32.bin", bytes(dataB3)),
                                  ("coords-px-f32x2.bin", bytes(coordPx)), ("coords-norm-f32x2.bin", bytes(coordNorm))]
    for (dn, rtne, ftz) in ftzModels { outs.append(("model-\(dn)-p1-p2-ieee-then-ftz-u32.bin", bytes(rtne.p1 + rtne.p2 + ftz.p1 + ftz.p2))) }
    for tag in blobs.keys.sorted() { outs.append(("out-\(tag).bin", blobs[tag]!)) }
    for (name, d) in outs { try d.write(to: out.appendingPathComponent(name)); files[name] = sha(d) }
    res["files_sha256"] = files
    res["layout"] = "out-<math>-<storage>.bin = concatenated sections (sections[].offset/bytes), raw u32 LE; texel (x,y) lane c at (y*16+x)*4+c; sample sections: coord i lanes i*4..i*4+3, coords = axis x axis (u fastest), \(NC) coords"
    res["axis_px"] = axis.map { h32($0.bitPattern) }
    res["status"] = priorityOK ? "S74_AND_B1_INTEGRAL_MATCH_MODELS" : "MISMATCH_OR_SECTION_FAILURE_SEE_ANALYSIS"
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