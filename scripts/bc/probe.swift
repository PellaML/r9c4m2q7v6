// Public Metal API only. Newly authored synthetic values; no external inputs.
// (a) rgb10a2Unorm premultiplied source-over blend over a nonzero initial
//     destination (written by a first non-blended pass), float4 and half4 returns.
// (b) float2 clamp variants under fast / safe / safe+precise runtime compile modes.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ c: Bool, _ m: String) throws { if !c { throw Failure(message: m) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func bytes<T>(_ v: [T]) -> Data { v.withUnsafeBytes { Data($0) } }
var lcg: UInt32 = 0x2545_f491
func rnd() -> Float { lcg = lcg &* 1664525 &+ 1013904223; return Float(lcg >> 8) / 16_777_216 }

// ---------- (a) blend cases ----------
var bcat: [UInt32] = [], dst: [SIMD4<Float>] = [], src: [SIMD4<Float>] = []
func addB(_ c: UInt32, _ d: SIMD4<Float>, _ s: SIMD4<Float>) { bcat.append(c); dst.append(d); src.append(s) }
let codes = [0, 1, 2, 255, 256, 511, 512, 767, 1021, 1022, 1023]
let alphas: [Float] = [0, 1e-7, 1.0 / 3, 0.5, 2.0 / 3, Float(1).nextDown, 1]
let cols: [Float] = [0, 0.25, 0.5, 1]
for dc in codes { for da in 0..<4 { for sa in alphas { for sc in cols {
    let d = SIMD4<Float>(Float(dc) / 1023, Float(1023 - dc) / 1023, Float((dc * 7) % 1024) / 1023, Float(da) / 3)
    addB(0, d, SIMD4(sc, sc * 0.5, 1 - sc, sa))
} } } }
for sc: Float in [-1, -0.0, 1.5, 2, 1e30] { for sa: Float in [-0.5, -0.0, 0, 0.5, 1.5, 2] {
    for d in [SIMD4<Float>(0.5, 0.25, 0.75, 2.0 / 3), SIMD4(1, 1, 1, 1), SIMD4(0, 0, 0, 0)] { addB(1, d, SIMD4(sc, sc, sc, sa)) }
} }
for _ in 0..<4096 { addB(3, SIMD4(rnd(), rnd(), rnd(), rnd()), SIMD4(rnd(), rnd(), rnd(), rnd())) }
for _ in 0..<1024 {
    let s = SIMD4<Float>(rnd(), rnd(), rnd(), rnd()) * 1.5 - 0.25
    addB(4, SIMD4(rnd(), rnd(), rnd(), rnd()), s)
}
let W = 64
let nb = bcat.count
let H = (nb + W - 1) / W
while bcat.count < W * H { addB(255, SIMD4(0, 0, 0, 0), SIMD4(0, 0, 0, 0)) }

// ---------- (b) clamp inputs ----------
let V: [Float] = [-Float.greatestFiniteMagnitude, -2, -1, -0.5, -Float.leastNormalMagnitude, -Float.leastNonzeroMagnitude,
                  -0.0, 0, Float.leastNonzeroMagnitude, Float.leastNormalMagnitude, 0.5, Float(1).nextDown, 1, Float(1).nextUp,
                  2, Float.greatestFiniteMagnitude]
var cx: [SIMD2<Float>] = [], clo: [SIMD2<Float>] = [], chi: [SIMD2<Float>] = []
for a in 0..<16 { for b in 0..<16 { for c in 0..<16 {
    cx.append(SIMD2(V[a], V[(a + 5) % 16])); clo.append(SIMD2(V[b], V[c])); chi.append(SIMD2(V[c], V[b]))
} } }
let nc = cx.count
func run() throws -> [String: Any] {
    try need(CommandLine.arguments.count == 3, "usage: probe scriptdir outdir")
    let dir = URL(fileURLWithPath: CommandLine.arguments[1])
    let out = URL(fileURLWithPath: CommandLine.arguments[2])
    guard let device = MTLCreateSystemDefaultDevice() else {
        throw Failure(message: "UNAVAILABLE: MTLCreateSystemDefaultDevice returned nil")
    }
    guard let queue = device.makeCommandQueue() else { throw Failure(message: "no command queue") }
    var res: [String: Any] = [:]
    var files: [String: String] = [:]
    func save(_ name: String, _ d: Data) throws { try d.write(to: out.appendingPathComponent(name)); files[name] = sha(d) }
    func buf(_ d: Data) throws -> MTLBuffer {
        guard let b = device.makeBuffer(length: max(d.count, 16), options: .storageModeShared) else { throw Failure(message: "buffer") }
        d.withUnsafeBytes { _ = memcpy(b.contents(), $0.baseAddress!, d.count) }; return b
    }
    func words<T>(_ b: MTLBuffer, _ count: Int, _ t: T.Type) -> [T] { Array(UnsafeBufferPointer(start: b.contents().bindMemory(to: T.self, capacity: count), count: count)) }

    // ---------- (a) blend ----------
    let blib = try device.makeLibrary(source: try String(contentsOf: dir.appendingPathComponent("blend.metal"), encoding: .utf8), options: MTLCompileOptions())
    func pso(_ frag: String, _ blend: Bool) throws -> MTLRenderPipelineState {
        let d = MTLRenderPipelineDescriptor()
        d.vertexFunction = blib.makeFunction(name: "tri"); d.fragmentFunction = blib.makeFunction(name: frag)
        try need(d.vertexFunction != nil && d.fragmentFunction != nil, "missing \(frag)")
        let c = d.colorAttachments[0]!
        c.pixelFormat = .rgb10a2Unorm; c.writeMask = .all; c.isBlendingEnabled = blend
        if blend {
            c.sourceRGBBlendFactor = .one; c.destinationRGBBlendFactor = .oneMinusSourceAlpha; c.rgbBlendOperation = .add
            c.sourceAlphaBlendFactor = .one; c.destinationAlphaBlendFactor = .oneMinusSourceAlpha; c.alphaBlendOperation = .add
        }
        return try device.makeRenderPipelineState(descriptor: d)
    }
    let pStore = try pso("store_float", false), pBF = try pso("store_float", true), pBH = try pso("store_half", true)
    let factors = [MTLBlendFactor.one.rawValue, MTLBlendFactor.oneMinusSourceAlpha.rawValue, MTLBlendOperation.add.rawValue,
                   MTLBlendFactor.one.rawValue, MTLBlendFactor.oneMinusSourceAlpha.rawValue, MTLBlendOperation.add.rawValue]
    res["blend_factors_raw_srcRGB_dstRGB_opRGB_srcA_dstA_opA"] = factors
    func target() throws -> MTLTexture {
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgb10a2Unorm, width: W, height: H, mipmapped: false)
        td.storageMode = .private; td.usage = [.renderTarget]
        guard let t = device.makeTexture(descriptor: td) else { throw Failure(message: "texture") }
        return t
    }
    let tF = try target(), tH = try target()
    let dBuf = try buf(bytes(dst)), sBuf = try buf(bytes(src))
    let n4 = W * H * 4
    let rbDF = try buf(Data(repeating: 0xa5, count: n4)), rbDH = try buf(Data(repeating: 0xa5, count: n4))
    let rbF = try buf(Data(repeating: 0xa5, count: n4)), rbH = try buf(Data(repeating: 0xa5, count: n4))
    var width = UInt32(W)
    guard let cb = queue.makeCommandBuffer() else { throw Failure(message: "command buffer") }
    func pass(_ t: MTLTexture, _ p: MTLRenderPipelineState, _ values: MTLBuffer, _ load: Bool) throws {
        let rp = MTLRenderPassDescriptor()
        rp.colorAttachments[0].texture = t
        rp.colorAttachments[0].loadAction = load ? .load : .clear
        rp.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        rp.colorAttachments[0].storeAction = .store
        guard let e = cb.makeRenderCommandEncoder(descriptor: rp) else { throw Failure(message: "render encoder") }
        e.setRenderPipelineState(p); e.setFragmentBuffer(values, offset: 0, index: 0); e.setFragmentBytes(&width, length: 4, index: 1)
        e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3); e.endEncoding()
    }
    func copy(_ pairs: [(MTLTexture, MTLBuffer)]) throws {
        guard let b = cb.makeBlitCommandEncoder() else { throw Failure(message: "blit") }
        for (t, d) in pairs {
            b.copy(from: t, sourceSlice: 0, sourceLevel: 0, sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0), sourceSize: MTLSize(width: W, height: H, depth: 1),
                   to: d, destinationOffset: 0, destinationBytesPerRow: W * 4, destinationBytesPerImage: n4)
        }
        b.endEncoding()
    }
    try pass(tF, pStore, dBuf, false); try pass(tH, pStore, dBuf, false)
    try copy([(tF, rbDF), (tH, rbDH)])
    try pass(tF, pBF, sBuf, true); try pass(tH, pBH, sBuf, true)
    try copy([(tF, rbF), (tH, rbH)])
    cb.commit(); cb.waitUntilCompleted()
    try need(cb.status == .completed && cb.error == nil, "blend command buffer status \(cb.status.rawValue) error \(String(describing: cb.error))")
    let oDF = words(rbDF, W * H, UInt32.self), oDH = words(rbDH, W * H, UInt32.self)
    let oF = words(rbF, W * H, UInt32.self), oH = words(rbH, W * H, UInt32.self)
    try save("blend-dest-in-f32x4.bin", bytes(dst)); try save("blend-src-in-f32x4.bin", bytes(src)); try save("blend-categories-u32.bin", bytes(bcat))
    try save("blend-dest-stored-float-u32.bin", bytes(oDF)); try save("blend-dest-stored-half-target-u32.bin", bytes(oDH))
    try save("blend-out-float4-u32.bin", bytes(oF)); try save("blend-out-half4-u32.bin", bytes(oH))
    func dec(_ p: UInt32) -> [UInt32] { [p & 1023, (p >> 10) & 1023, (p >> 20) & 1023, p >> 30] }
    func q(_ x: Double, _ m: Double) -> UInt32 { x.isNaN ? 0 : UInt32((min(max(x, 0), 1) * m).rounded(.toNearestOrEven)) }
    func c01(_ x: Double) -> Double { x.isNaN ? 0 : min(max(x, 0), 1) }
    // model(srcClamp, f32Arithmetic, halfSource)
    func blendTally(_ out: [UInt32], _ dest: [UInt32], half: Bool) -> [String: Any] {
        var mis: [String: [Int]] = [:]
        var examples: [[String: Any]] = []
        let names = ["f64_srcclamp", "f64_noclamp", "f32_srcclamp", "f32fma_srcclamp"]
        for nm in names { mis[nm] = [0, 0] }
        for i in 0..<nb {
            let g = dec(out[i]), dq = dec(dest[i])
            let sv = src[i]
            let s4 = half ? [Float(Float16(sv.x)), Float(Float16(sv.y)), Float(Float16(sv.z)), Float(Float16(sv.w))] : [sv.x, sv.y, sv.z, sv.w]
            for nm in names {
                for c in 0..<4 {
                    let m: Double = c == 3 ? 3 : 1023
                    let d = Double(dq[c]) / m
                    let r: Double
                    switch nm {
                    case "f64_srcclamp": r = c01(Double(s4[c])) + d * (1 - c01(Double(s4[3])))
                    case "f64_noclamp": r = Double(s4[c]) + d * (1 - Double(s4[3]))
                    case "f32_srcclamp":
                        let sf = Float(c01(Double(s4[c]))), af = Float(c01(Double(s4[3]))), df = Float(dq[c]) / Float(m)
                        r = Double(sf + df * (1 - af))
                    default:
                        let sf = Float(c01(Double(s4[c]))), af = Float(c01(Double(s4[3]))), df = Float(dq[c]) / Float(m)
                        r = Double(sf.addingProduct(df, 1 - af))
                    }
                    if q(r, m) != g[c] {
                        mis[nm]![c == 3 ? 1 : 0] += 1
                        if nm == "f64_srcclamp" && examples.count < 48 {
                            examples.append(["i": i, "cat": bcat[i], "ch": c, "src": "\(s4[c])", "src_a": "\(s4[3])", "dest_code": dq[c],
                                             "native": g[c], "model": q(r, m), "model_real": "\(r * m)"])
                        }
                    }
                }
            }
        }
        return ["mismatches_rgb_alpha": mis, "f64_srcclamp_examples": examples]
    }
    var destExact = 0
    for i in 0..<nb {
        let d = dst[i], g = dec(oDF[i])
        if g == [q(Double(d.x), 1023), q(Double(d.y), 1023), q(Double(d.z), 1023), q(Double(d.w), 3)] { destExact += 1 }
    }
    var blend: [String: Any] = [:]
    blend["cases"] = nb
    blend["dest_stored_matches_exact_rne"] = destExact
    blend["dest_float_equals_half_target"] = oDF == oDH
    blend["float4"] = blendTally(oF, oDF, half: false)
    blend["half4_model_on_rne_half_source"] = blendTally(oH, oDH, half: true)
    var fe = 0
    for i in 0..<nb where oF[i] == oH[i] { fe += 1 }
    blend["float4_equals_half4_pixels"] = fe
    res["blend"] = blend
    // ---------- (b) clamp ----------
    let csrc = try String(contentsOf: dir.appendingPathComponent("clamp.metal"), encoding: .utf8)
    let xB = try buf(bytes(cx)), loB = try buf(bytes(clo)), hiB = try buf(bytes(chi))
    var count = UInt32(nc)
    let variants = ["clamp", "fast::clamp", "precise::clamp", "min(max(x,lo),hi)", "max(min(x,hi),lo)"]
    let modes = ["fast", "safe", "safe+precise_functions"]
    var outputs: [String: [UInt32]] = [:]
    var compile: [String: String] = [:]
    var all = Data()
    for (mi, mode) in modes.enumerated() {
        for (v, vname) in variants.enumerated() {
            let key = "\(mode)|\(vname)"
            let o = MTLCompileOptions()
            o.preprocessorMacros = ["VARIANT": NSNumber(value: v)]
            if #available(macOS 15.0, *) {
                o.mathMode = mi == 0 ? .fast : .safe
                if mi == 2 { o.mathFloatingPointFunctions = .precise }
            } else {
                o.fastMathEnabled = mi == 0
            }
            var got = [UInt32](repeating: 0xffff_ffff, count: nc * 2)
            do {
                let lib = try device.makeLibrary(source: csrc, options: o)
                guard let fn = lib.makeFunction(name: "observe") else { throw Failure(message: "no function") }
                let p = try device.makeComputePipelineState(function: fn)
                let ob = try buf(Data(repeating: 0xa5, count: nc * 8))
                guard let cb2 = queue.makeCommandBuffer(), let e = cb2.makeComputeCommandEncoder() else { throw Failure(message: "compute encoder") }
                e.setComputePipelineState(p)
                e.setBuffer(xB, offset: 0, index: 0); e.setBuffer(loB, offset: 0, index: 1); e.setBuffer(hiB, offset: 0, index: 2)
                e.setBuffer(ob, offset: 0, index: 3); e.setBytes(&count, length: 4, index: 4)
                e.dispatchThreadgroups(MTLSize(width: (nc + 63) / 64, height: 1, depth: 1), threadsPerThreadgroup: MTLSize(width: 64, height: 1, depth: 1))
                e.endEncoding(); cb2.commit(); cb2.waitUntilCompleted()
                try need(cb2.status == .completed && cb2.error == nil, "clamp command buffer \(cb2.status.rawValue)")
                got = words(ob, nc * 2, UInt32.self)
                outputs[key] = got
                compile[key] = "ok"
            } catch {
                compile[key] = "COMPILE_OR_RUN_UNAVAILABLE_NOT_GPU_SEMANTICS: \(error)".prefix(400).description
            }
            all.append(bytes(got))
        }
    }
    try save("clamp-x-f32x2.bin", bytes(cx)); try save("clamp-lo-f32x2.bin", bytes(clo)); try save("clamp-hi-f32x2.bin", bytes(chi))
    try save("clamp-out-mode-variant-f32x2.bin", all)
    let ref = outputs["safe|min(max(x,lo),hi)"]
    var cmp: [String: Any] = [:]
    for (key, got) in outputs {
        var vsMinMax = [0, 0], vsMaxMin = [0, 0], signedZeroOnly = 0
        let alt = outputs[String(key.split(separator: "|")[0]) + "|max(min(x,hi),lo)"]
        let mm = outputs[String(key.split(separator: "|")[0]) + "|min(max(x,lo),hi)"]
        for j in 0..<(nc * 2) {
            let lane = j % 2, i = j / 2
            let lo = lane == 0 ? clo[i].x : clo[i].y, hi = lane == 0 ? chi[i].x : chi[i].y
            let ordered = lo <= hi ? 0 : 1
            if let mm = mm, mm[j] != got[j] {
                vsMinMax[ordered] += 1
                if Float(bitPattern: mm[j]) == Float(bitPattern: got[j]) { signedZeroOnly += 1 }
            }
            if let alt = alt, alt[j] != got[j] { vsMaxMin[ordered] += 1 }
        }
        var refDiff = 0
        if let ref = ref { for j in 0..<(nc * 2) where ref[j] != got[j] { refDiff += 1 } }
        cmp[key] = ["vs_same_mode_minmax_ordered_reversed": vsMinMax, "vs_same_mode_maxmin_ordered_reversed": vsMaxMin,
                    "signed_zero_only_diffs_vs_minmax": signedZeroOnly, "vs_safe_minmax_total": refDiff] as [String: Any]
    }
    res["clamp"] = ["lanes": nc * 2, "compile": compile, "comparisons": cmp,
                    "file_order": "modes [fast, safe, safe+precise_functions] x variants [clamp, fast::clamp, precise::clamp, minmax, maxmin], each nc*2 f32 bits; 0xffffffff fill = unavailable"] as [String: Any]

    var dev: [String: Any] = [:]
    dev["name"] = device.name
    dev["registryID"] = device.registryID
    if #available(macOS 14.0, *) { dev["architecture"] = device.architecture.name }
    res["device"] = dev
    res["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    res["files_sha256"] = files
    res["status"] = "OBSERVED"
    return res
}
let outDir = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "."
var result: [String: Any]
var code: Int32 = 0
do { result = try run() } catch let e as Failure {
    result = ["status": e.message.hasPrefix("UNAVAILABLE") ? "UNAVAILABLE_NOT_PASS" : "FAILED_NOT_PASS", "error": e.message]; code = 3
} catch { result = ["status": "FAILED_NOT_PASS", "error": "\(error)"]; code = 3 }
let json = try! JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
try! json.write(to: URL(fileURLWithPath: outDir).appendingPathComponent("summary.json"))
FileHandle.standardOutput.write(json)
exit(code)