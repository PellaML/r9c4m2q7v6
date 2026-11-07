// Newly authored synthetic probe. Public Metal API only; all texels and
// coordinates are generated below. Observes rgb10a2Unorm (format 90) sampling.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ c: Bool, _ m: String) throws { if !c { throw Failure(message: m) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func bytes<T>(_ v: [T]) -> Data { v.withUnsafeBytes { Data($0) } }
func hx32(_ v: UInt32) -> String { String(format: "0x%08x", v) }
func hx16(_ v: UInt16) -> String { String(format: "0x%04x", v) }

let W = 16, H = 16
let PARENT = 16384, OFFSET = 12288, PITCH = 256
func pack(_ c: [Int]) -> UInt32 { UInt32(c[0]) | UInt32(c[1]) << 10 | UInt32(c[2]) << 20 | UInt32(c[3]) << 30 }
var state: UInt64 = 0x9e37_79b9_7f4a_7c15
func rnd() -> UInt32 { state = state &* 6364136223846793005 &+ 1442695040888963407; return UInt32(truncatingIfNeeded: state >> 32) }

// ---- synthetic texels ----
let row0: [[Int]] = [[0, 1023, 512, 0], [1023, 0, 511, 3], [0, 1023, 1, 1], [1023, 0, 1022, 2],
                     [1, 2, 3, 0], [1022, 1021, 1020, 3], [511, 512, 513, 1], [512, 511, 510, 2],
                     [0, 0, 0, 0], [1023, 1023, 1023, 3], [341, 682, 1023, 1], [682, 341, 0, 2],
                     [100, 900, 300, 3], [923, 123, 723, 0], [1, 1022, 0, 2], [1022, 1, 1023, 1]]
var texels = [UInt32](repeating: 0, count: W * H)
for x in 0..<W {
    texels[x] = pack(row0[x])
    let c = row0[x]
    texels[W + x] = pack([1023 - c[0], 1023 - c[1], 1023 - c[2], 3 - c[3]])
}
for i in (2 * W)..<(W * H) { texels[i] = rnd() }

// ---- synthetic normalized coordinates (texel space t maps to t/16 exactly) ----
var uv: [SIMD2<Float>] = []
var sets: [[String: Any]] = []
func mark(_ name: String, _ start: Int) { sets.append(["name": name, "start": start, "count": uv.count - start]) }
var st = uv.count
for k in 0...4096 { uv.append(SIMD2((0.5 + Float(k) / 4096) / 16, 0.5 / 16)) }      // x 0|1 on row 0
for k in 0...4096 { uv.append(SIMD2(0.5 / 16, (0.5 + Float(k) / 4096) / 16)) }      // y 0|1 at col 0
for k in 0...4096 { uv.append(SIMD2(8.5 / 16, (0.5 + Float(k) / 4096) / 16)) }      // y 0|1 at col 8
mark("fine_1_4096", st); st = uv.count
for k in 0..<1024 {
    let f = (Float(k) + 0.5) / 1024
    let ux = (0.5 + f) / 16, vy = (0.5 + f) / 16
    for u in [ux, ux.nextDown, ux.nextUp] { uv.append(SIMD2(u, 0.5 / 16)) }
    for v in [vy, vy.nextDown, vy.nextUp] { uv.append(SIMD2(0.5 / 16, v)) }
}
mark("ties_1_2048_pm1ulp", st); st = uv.count
for j in 0...144 { for k in 0...144 { uv.append(SIMD2((Float(k) / 8 - 1) / 16, (Float(j) / 8 - 1) / 16)) } }
mark("grid_1_8", st); st = uv.count
var specials: [Float] = [-0.0, 0, -1e-30, 1e-30, -1, 2, 1e6, -1e6, 1e30, -1e30]
for k in 0...16 { let u = Float(k) / 16; specials += [u, u.nextDown, u.nextUp] }
for k in 0..<16 { let u = (Float(k) + 0.5) / 16; specials += [u, u.nextDown, u.nextUp] }
for v in specials { for u in specials { uv.append(SIMD2(u, v)) } }
mark("edges_centers_pm1ulp", st); st = uv.count
for _ in 0..<8192 {
    let a = Float(rnd() >> 8) / 16777216, b = Float(rnd() >> 8) / 16777216
    uv.append(SIMD2(a * 1.25 - 0.125, b * 1.25 - 0.125))
}
for _ in 0..<8192 { // arbitrary binary32 mantissas in [0,1)
    let a = Float(bitPattern: 0x3f80_0000 | (rnd() >> 9)) - 1, b = Float(bitPattern: 0x3f80_0000 | (rnd() >> 9)) - 1
    uv.append(SIMD2(a, b))
}
mark("random", st)
let n = uv.count

struct Variant { let name: String; let tex: MTLTexture; let info: [String: Any] }

func run() throws -> [String: Any] {
    try need(CommandLine.arguments.count == 3, "usage: probe probe.metal outdir")
    let out = URL(fileURLWithPath: CommandLine.arguments[2])
    let source = try String(contentsOfFile: CommandLine.arguments[1], encoding: .utf8)
    guard let device = MTLCreateSystemDefaultDevice() else { throw Failure(message: "UNAVAILABLE: no device") }
    guard let queue = device.makeCommandQueue() else { throw Failure(message: "no command queue") }
    let lib = try device.makeLibrary(source: source, options: MTLCompileOptions())
    func cps(_ name: String) throws -> MTLComputePipelineState {
        guard let f = lib.makeFunction(name: name) else { throw Failure(message: "no function \(name)") }
        return try device.makeComputePipelineState(function: f)
    }
    func rps(_ name: String) throws -> MTLRenderPipelineState {
        let pd = MTLRenderPipelineDescriptor()
        pd.vertexFunction = lib.makeFunction(name: "tri"); pd.fragmentFunction = lib.makeFunction(name: name)
        try need(pd.vertexFunction != nil && pd.fragmentFunction != nil, "missing \(name)")
        pd.colorAttachments[0].pixelFormat = .rgba8Unorm
        return try device.makeRenderPipelineState(descriptor: pd)
    }
    let kFloat = try cps("k_float"), kHalf = try cps("k_half"), kCxF = try cps("k_cx_float"), kCxH = try cps("k_cx_half")
    let kRead = try cps("k_read"), fFloat = try rps("f_float"), fHalf = try rps("f_half")
    var log: [String] = []

    // ---- samplers: nearest/linear x lodAverage false/true ----
    let setSel = NSSelectorFromString("setLodAverage:"), getSel = NSSelectorFromString("lodAverage")
    var samplers: [(String, MTLSamplerState)] = []
    var samplerInfo: [[String: Any]] = []
    for linear in [false, true] { for avg in [false, true] {
        let sd = MTLSamplerDescriptor()
        sd.minFilter = linear ? .linear : .nearest; sd.magFilter = sd.minFilter; sd.mipFilter = .notMipmapped
        sd.normalizedCoordinates = true; sd.maxAnisotropy = 1
        sd.lodMinClamp = 0; sd.lodMaxClamp = Float.greatestFiniteMagnitude
        sd.sAddressMode = .clampToEdge; sd.tAddressMode = .clampToEdge; sd.rAddressMode = .clampToEdge
        sd.compareFunction = .never
        try need(sd.responds(to: setSel) && sd.responds(to: getSel), "API_UNAVAILABLE: lodAverage selectors")
        sd.setValue(avg, forKey: "lodAverage")
        let back = (sd.value(forKey: "lodAverage") as? Bool) ?? !avg
        try need(back == avg, "lodAverage readback")
        guard let ss = device.makeSamplerState(descriptor: sd) else { throw Failure(message: "sampler") }
        let name = (linear ? "linear" : "nearest") + (avg ? "-avg1" : "-avg0")
        samplers.append((name, ss))
        samplerInfo.append(["name": name, "min": sd.minFilter.rawValue, "mag": sd.magFilter.rawValue, "mip": sd.mipFilter.rawValue,
                            "normalized": sd.normalizedCoordinates, "max_anisotropy": sd.maxAnisotropy, "lod_min": sd.lodMinClamp,
                            "lod_max_bits": hx32(sd.lodMaxClamp.bitPattern), "s": sd.sAddressMode.rawValue,
                            "t": sd.tAddressMode.rawValue, "r": sd.rAddressMode.rawValue, "compare": sd.compareFunction.rawValue,
                            "lod_average": back, "support_argument_buffers": sd.supportArgumentBuffers])
    } }

    // ---- textures ----
    let usage: MTLTextureUsage = [.shaderRead, .shaderWrite, .renderTarget]
    var parentBytes = [UInt8](repeating: 0xa5, count: PARENT)
    for y in 0..<H { for x in 0..<W {
        let w = texels[y * W + x], o = OFFSET + y * PITCH + x * 4
        for b in 0..<4 { parentBytes[o + b] = UInt8((w >> (8 * UInt32(b))) & 0xff) }
    } }
    guard let staging = device.makeBuffer(bytes: parentBytes, length: PARENT, options: .storageModeShared) else {
        throw Failure(message: "staging") }
    func waitBlit(_ body: (MTLBlitCommandEncoder) -> Void) throws {
        guard let cb = queue.makeCommandBuffer(), let e = cb.makeBlitCommandEncoder() else { throw Failure(message: "blit") }
        body(e); e.endEncoding(); cb.commit(); cb.waitUntilCompleted()
        try need(cb.status == .completed && cb.error == nil, "blit status \(cb.status.rawValue) \(String(describing: cb.error))")
    }
    func desc(_ m: MTLStorageMode) -> MTLTextureDescriptor {
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgb10a2Unorm, width: W, height: H, mipmapped: false)
        d.usage = usage; d.storageMode = m; return d
    }
    func info(_ t: MTLTexture, _ kind: String) -> [String: Any] {
        var i: [String: Any] = ["kind": kind, "pixelFormat": t.pixelFormat.rawValue, "width": t.width, "height": t.height,
                                "mipmapLevelCount": t.mipmapLevelCount, "storageMode": t.storageMode.rawValue,
                                "usage": t.usage.rawValue, "isBufferView": t.buffer != nil]
        if t.buffer != nil { i["bufferOffset"] = t.bufferOffset; i["bufferBytesPerRow"] = t.bufferBytesPerRow
            i["parentLength"] = t.buffer!.length; i["parentStorageMode"] = t.buffer!.storageMode.rawValue }
        return i
    }
    var variants: [Variant] = []
    var textureErrors: [String: String] = [:]
    let packed = bytes(texels)
    let stdModes: [(String, MTLStorageMode)] = [("std-shared", .shared), ("std-private", .private), ("std-managed", .managed)]
    for (name, mode) in stdModes {
        guard let t = device.makeTexture(descriptor: desc(mode)) else { textureErrors[name] = "makeTexture nil"; continue }
        if mode == .private {
            do { try waitBlit { $0.copy(from: staging, sourceOffset: OFFSET, sourceBytesPerRow: PITCH, sourceBytesPerImage: PITCH * H,
                                     sourceSize: MTLSize(width: W, height: H, depth: 1), to: t, destinationSlice: 0,
                                     destinationLevel: 0, destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0)) } }
            catch { textureErrors[name] = "\(error)"; continue }
        } else {
            packed.withUnsafeBytes { t.replace(region: MTLRegionMake2D(0, 0, W, H), mipmapLevel: 0, withBytes: $0.baseAddress!, bytesPerRow: W * 4) }
        }
        variants.append(Variant(name: name, tex: t, info: info(t, name)))
    }
    let align = device.minimumLinearTextureAlignment(for: .rgb10a2Unorm)
    let viewModes: [(String, MTLResourceOptions, MTLStorageMode)] = [("view-shared", .storageModeShared, .shared),
                                                                     ("view-private", .storageModePrivate, .private)]
    for (name, opt, mode) in viewModes {
        guard let parent = device.makeBuffer(length: PARENT, options: opt) else { textureErrors[name] = "buffer nil"; continue }
        if mode == .private {
            do { try waitBlit { $0.copy(from: staging, sourceOffset: 0, to: parent, destinationOffset: 0, size: PARENT) } }
            catch { textureErrors[name] = "\(error)"; continue }
        } else { memcpy(parent.contents(), staging.contents(), PARENT) }
        guard OFFSET % align == 0 && PITCH % align == 0 else { textureErrors[name] = "alignment \(align)"; continue }
        guard let t = parent.makeTexture(descriptor: desc(mode), offset: OFFSET, bytesPerRow: PITCH) else {
            textureErrors[name] = "makeTexture(offset:) nil"; continue }
        variants.append(Variant(name: name, tex: t, info: info(t, name)))
    }
    try need(!variants.isEmpty && variants[0].name == "std-shared", "reference texture unavailable: \(textureErrors)")

    // ---- common buffers ----
    guard let ub = device.makeBuffer(bytes: uv, length: n * 8, options: .storageModeShared) else { throw Failure(message: "uv") }
    var count = UInt32(n)
    let GW = 256, GH = (n + GW - 1) / GW
    let rtd = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Unorm, width: GW, height: GH, mipmapped: false)
    rtd.storageMode = .private; rtd.usage = [.renderTarget]
    guard let rt = device.makeTexture(descriptor: rtd) else { throw Failure(message: "render target") }
    func outBuf(_ len: Int) throws -> MTLBuffer {
        guard let b = device.makeBuffer(length: len, options: .storageModeShared) else { throw Failure(message: "out buffer") }
        memset(b.contents(), 0xde, len); return b
    }
    func data(_ b: MTLBuffer, _ len: Int) -> Data { Data(bytes: b.contents(), count: len) }
    func finish(_ cb: MTLCommandBuffer) throws {
        cb.commit(); cb.waitUntilCompleted()
        try need(cb.status == .completed && cb.error == nil, "status \(cb.status.rawValue) \(String(describing: cb.error))")
    }
    func compute(_ p: MTLComputePipelineState, _ t: MTLTexture, _ s: MTLSamplerState?, _ elem: Int) throws -> (Data, Data) {
        let a = try outBuf(n * elem), b = try outBuf(n * elem)
        guard let cb = queue.makeCommandBuffer(), let e = cb.makeComputeCommandEncoder() else { throw Failure(message: "enc") }
        e.setComputePipelineState(p); e.setTexture(t, index: 0)
        if let s = s { e.setSamplerState(s, index: 0) }
        e.setBytes(&count, length: 4, index: 0); e.setBuffer(ub, offset: 0, index: 1)
        e.setBuffer(a, offset: 0, index: 2); e.setBuffer(b, offset: 0, index: 3)
        e.dispatchThreadgroups(MTLSize(width: (n + 63) / 64, height: 1, depth: 1), threadsPerThreadgroup: MTLSize(width: 64, height: 1, depth: 1))
        e.endEncoding(); try finish(cb)
        return (data(a, n * elem), data(b, n * elem))
    }
    func fragment(_ p: MTLRenderPipelineState, _ t: MTLTexture, _ s: MTLSamplerState, _ elem: Int) throws -> Data {
        let o = try outBuf(n * elem)
        var gw = UInt32(GW)
        guard let cb = queue.makeCommandBuffer() else { throw Failure(message: "cb") }
        let rp = MTLRenderPassDescriptor()
        rp.colorAttachments[0].texture = rt; rp.colorAttachments[0].loadAction = .dontCare; rp.colorAttachments[0].storeAction = .dontCare
        guard let e = cb.makeRenderCommandEncoder(descriptor: rp) else { throw Failure(message: "render enc") }
        e.setRenderPipelineState(p); e.setFragmentTexture(t, index: 0); e.setFragmentSamplerState(s, index: 0)
        e.setFragmentBytes(&count, length: 4, index: 0); e.setFragmentBuffer(ub, offset: 0, index: 1)
        e.setFragmentBuffer(o, offset: 0, index: 2); e.setFragmentBytes(&gw, length: 4, index: 3)
        e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
        e.endEncoding(); try finish(cb)
        return data(o, n * elem)
    }

    // ---- outputs ----
    var files: [String: String] = [:]
    func save(_ name: String, _ d: Data) throws { try d.write(to: out.appendingPathComponent(name)); files[name] = sha(d) }
    try save("texels-u32.bin", packed); try save("uv-f32x2.bin", bytes(uv)); try save("parent-u8.bin", Data(parentBytes))
    var refs: [String: Data] = [:]
    var observations: [[String: Any]] = []
    var diffFiles = 0
    func record(_ tex: String, _ smp: String, _ path: String, _ type: String, _ d: Data) throws {
        let elem = type == "float" ? 16 : 8, filt = smp.hasPrefix("linear") ? "linear" : "nearest"
        let key = "\(filt)-\(type)"
        var o: [String: Any] = ["texture": tex, "sampler": smp, "path": path, "type": type, "sha256": sha(d)]
        var poison = 0
        d.withUnsafeBytes { (p: UnsafeRawBufferPointer) in
            for i in 0..<n { if p[i * elem..<(i + 1) * elem].allSatisfy({ $0 == 0xde }) { poison += 1 } }
        }
        o["poisoned"] = poison
        if refs[key] == nil {
            refs[key] = d; o["role"] = "reference"; try save("ref-\(key).bin", d)
        } else {
            let r = refs[key]!
            var diffs = 0, lanes = 0
            var ex: [[String: Any]] = []
            r.withUnsafeBytes { (rp: UnsafeRawBufferPointer) in d.withUnsafeBytes { (dp: UnsafeRawBufferPointer) in
                for i in 0..<n {
                    let a = rp[i * elem..<(i + 1) * elem], b = dp[i * elem..<(i + 1) * elem]
                    if !a.elementsEqual(b) {
                        diffs += 1
                        for c in 0..<4 { let s = c * elem / 4; if !rp[i * elem + s..<i * elem + s + elem / 4].elementsEqual(dp[i * elem + s..<i * elem + s + elem / 4]) { lanes += 1 } }
                        if ex.count < 8 {
                            func words(_ q: UnsafeRawBufferPointer) -> [String] {
                                (0..<4).map { c in elem == 16 ? hx32(q.loadUnaligned(fromByteOffset: i * 16 + c * 4, as: UInt32.self))
                                                              : hx16(q.loadUnaligned(fromByteOffset: i * 8 + c * 2, as: UInt16.self)) } }
                            ex.append(["i": i, "u_bits": hx32(uv[i].x.bitPattern), "v_bits": hx32(uv[i].y.bitPattern),
                                       "ref": words(rp), "got": words(dp)])
                        }
                    }
                }
            } }
            o["diff_coords_vs_ref"] = diffs; o["diff_lanes_vs_ref"] = lanes; o["examples"] = ex
            if diffs > 0 && diffFiles < 16 {
                diffFiles += 1; let f = "diff\(diffFiles)-\(tex)-\(smp)-\(path)-\(type).bin"; try save(f, d); o["raw_file"] = f
            }
        }
        observations.append(o)
    }
    var readRef: Data? = nil
    var readObs: [[String: Any]] = []
    for v in variants {
        do {
            let of = try outBuf(W * H * 16), oh = try outBuf(W * H * 8)
            guard let cb = queue.makeCommandBuffer(), let e = cb.makeComputeCommandEncoder() else { throw Failure(message: "enc") }
            e.setComputePipelineState(kRead); e.setTexture(v.tex, index: 0); e.setTexture(v.tex, index: 1)
            e.setBuffer(of, offset: 0, index: 0); e.setBuffer(oh, offset: 0, index: 1)
            e.dispatchThreadgroups(MTLSize(width: 1, height: 1, depth: 1), threadsPerThreadgroup: MTLSize(width: 16, height: 16, depth: 1))
            e.endEncoding(); try finish(cb)
            let both = data(of, W * H * 16) + data(oh, W * H * 8)
            if readRef == nil { readRef = both; try save("read-float-u32x4.bin", data(of, W * H * 16)); try save("read-half-u16x4.bin", data(oh, W * H * 8)) }
            readObs.append(["texture": v.name, "sha256": sha(both), "equals_reference": both == readRef!])
        } catch { readObs.append(["texture": v.name, "error": "\(error)"]) }
        for (sname, s) in samplers {
            do {
                let (fl, fi) = try compute(kFloat, v.tex, s, 16)
                try record(v.name, sname, "kernel-level0", "float", fl); try record(v.name, sname, "kernel-implicit", "float", fi)
                let (hl, hi) = try compute(kHalf, v.tex, s, 8)
                try record(v.name, sname, "kernel-level0", "half", hl); try record(v.name, sname, "kernel-implicit", "half", hi)
                try record(v.name, sname, "fragment-implicit", "float", try fragment(fFloat, v.tex, s, 16))
                try record(v.name, sname, "fragment-implicit", "half", try fragment(fHalf, v.tex, s, 8))
            } catch { log.append("\(v.name) \(sname): \(error)") }
        }
        do {
            let (fl, fn) = try compute(kCxF, v.tex, nil, 16)
            try record(v.name, "linear-constexpr", "kernel-level0", "float", fl); try record(v.name, "nearest-constexpr", "kernel-level0", "float", fn)
            let (hl, hn) = try compute(kCxH, v.tex, nil, 8)
            try record(v.name, "linear-constexpr", "kernel-level0", "half", hl); try record(v.name, "nearest-constexpr", "kernel-level0", "half", hn)
        } catch { log.append("\(v.name) constexpr: \(error)") }
    }

    var dev: [String: Any] = ["name": device.name, "registryID": device.registryID, "hasUnifiedMemory": device.hasUnifiedMemory,
                              "isLowPower": device.isLowPower, "isHeadless": device.isHeadless,
                              "supportsMac2": device.supportsFamily(.mac2), "supportsApple7": device.supportsFamily(.apple7),
                              "supportsMetal3": device.supportsFamily(.metal3),
                              "minimumLinearTextureAlignment_rgb10a2": align]
    if #available(macOS 14.0, *) { dev["architecture"] = device.architecture.name }
    let totalDiffs = observations.compactMap { $0["diff_coords_vs_ref"] as? Int }.reduce(0, +)
    let totalPoison = observations.compactMap { $0["poisoned"] as? Int }.reduce(0, +)
    let expected = variants.count * (samplers.count * 6 + 4)
    var res: [String: Any] = [:]
    res["status"] = (log.isEmpty && textureErrors.isEmpty && observations.count == expected)
        ? (totalDiffs == 0 && totalPoison == 0 ? "ALL_VARIANTS_EQUAL_REFERENCE" : "DIFFERENCE_OBSERVED") : "PARTIAL_NOT_PASS"
    res["device"] = dev; res["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    res["coordinates"] = n; res["coordinate_sets"] = sets; res["samplers"] = samplerInfo
    res["textures"] = variants.map { $0.info }; res["texture_errors"] = textureErrors
    res["direct_read"] = readObs; res["observations"] = observations; res["observation_count"] = observations.count
    res["expected_observations"] = expected; res["errors"] = log; res["files_sha256"] = files
    res["reference"] = "std-shared texture, first sampler of each filter (lodAverage=false), kernel-level0; key filter-type"
    res["layout"] = "float outputs: n x uint32[4] raw binary32 RGBA; half outputs: n x uint16[4] raw binary16 RGBA; uv: n x float32[2] normalized"
    return res
}
let outDir = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "."
var result: [String: Any]
var code: Int32 = 0
do { result = try run() } catch let e as Failure {
    result = ["status": e.message.hasPrefix("UNAVAILABLE") ? "UNAVAILABLE_NOT_PASS" : "FAILED_NOT_PASS", "error": e.message]; code = 3
} catch { result = ["status": "FAILED_NOT_PASS", "error": "\(error)"]; code = 3 }
if (result["status"] as? String) == "PARTIAL_NOT_PASS" { code = 4 }
let json = try! JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
try! json.write(to: URL(fileURLWithPath: outDir).appendingPathComponent("summary.json"))
FileHandle.standardOutput.write(json.prefix(4000))
exit(code)