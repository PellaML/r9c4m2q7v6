// Public Metal API only. Newly authored synthetic values; no external inputs.
// Observes native MTLPixelFormat.rgb10a2Unorm stores: render-pass clear and
// fragment-shader output (float4 and half4 return types), via blit readback.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ c: Bool, _ m: String) throws { if !c { throw Failure(message: m) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func bytes<T>(_ v: [T]) -> Data { v.withUnsafeBytes { Data($0) } }

// ---------- synthetic fragment inputs (category, float4) ----------
var cats: [UInt32] = []
var vals: [SIMD4<Float>] = []
func add(_ c: UInt32, _ v: SIMD4<Float>) { cats.append(c); vals.append(v) }
func s4(_ x: Float) -> SIMD4<Float> { SIMD4(x, x, x, x) }
// 0: every exact 10-bit code k/1023 in rgb; alpha cycles exact 2-bit codes
for k in 0...1023 { let v = Float(k) / 1023; add(0, SIMD4(v, v, v, Float(k % 4) / 3)) }
// 1: 10-bit midpoints (k+0.5)/1023 nearest float and +-1,+-2 ulp, all channels
for k in 0..<1023 {
    let t = Float((Double(k) + 0.5) / 1023)
    for x in [t.nextDown.nextDown, t.nextDown, t, t.nextUp, t.nextUp.nextUp] { add(1, s4(x)) }
}
// 2: dense sweep i/1024 all channels (2-bit alpha thresholds + 10-bit grid)
for i in 0...1024 { add(2, s4(Float(i) / 1024)) }
// 3: 2-bit midpoints (k+0.5)/3 with +-1..4 ulp, all channels
for k in 0..<3 {
    var x = Float((Double(k) + 0.5) / 3)
    for _ in 0..<4 { x = x.nextDown }
    for _ in 0..<9 { add(3, s4(x)); x = x.nextUp }
}
// 4: channel order / independence
let order: [SIMD4<Float>] = [
    SIMD4(1, 0, 0, 0), SIMD4(0, 1, 0, 0), SIMD4(0, 0, 1, 0), SIMD4(0, 0, 0, 1),
    SIMD4(0, 0, 0, 0), SIMD4(1, 1, 1, 1), SIMD4(1.0 / 1023, 2.0 / 1023, 4.0 / 1023, 1.0 / 3),
    SIMD4(0.25, 0.5, 0.75, 2.0 / 3), SIMD4(512.0 / 1023, 0, 1, 0), SIMD4(0, 1, 0, 1)]
for v in order { add(4, v) }
// 5: clamp and special finite/non-finite scalars, all channels
let specials: [Float] = [-1e30, -2, -1, -0.5, -1e-7, -Float.leastNonzeroMagnitude, -0.0, 0,
    Float.leastNonzeroMagnitude, Float.leastNormalMagnitude, 1e-7, Float(1).nextDown, 1, Float(1).nextUp,
    1.0001, 1.5, 2, 3, 1023, 1024, 1e30, Float.greatestFiniteMagnitude, -Float.greatestFiniteMagnitude,
    Float.infinity, -Float.infinity, Float.nan, -Float.nan]
for x in specials { add(5, s4(x)) }
// 6: mixed-sign / out-of-range per channel combinations
for v in [SIMD4<Float>(-1, 0.5, 2, 0.5), SIMD4(2, -1, 0.5, -1), SIMD4(0.5, 2, -1, 2), SIMD4(Float.nan, 1, 0, Float.infinity)] { add(6, v) }
// 7: typical finite values, independent per channel (LCG in [0,1))
var lcg: UInt32 = 0x9e37_79b9
func next() -> Float { lcg = lcg &* 1664525 &+ 1013904223; return Float(lcg >> 8) / 16_777_216 }
for _ in 0..<2048 { add(7, SIMD4(next(), next(), next(), next())) }
let W = 64
let n = vals.count
let H = (n + W - 1) / W
while vals.count < W * H { cats.append(255); vals.append(s4(0)) } // padding, excluded from analysis

// ---------- clear colors (double) ----------
var clears: [SIMD4<Double>] = []
for k in [0, 1, 2, 255, 511, 512, 1021, 1022] {
    let t = (Double(k) + 0.5) / 1023
    let f = Float(t)
    for x in [Double(f.nextDown), t.nextDown, t, t.nextUp, Double(f.nextUp)] { clears.append(SIMD4(x, x, x, x)) }
}
for k in 0..<3 {
    let t = (Double(k) + 0.5) / 3
    let f = Float(t)
    for x in [Double(f.nextDown), t.nextDown, t, t.nextUp, Double(f.nextUp)] { clears.append(SIMD4(x, x, x, x)) }
}
for x in [-1.0, -1e-9, -0.0, 0, 1e-9, 1.0 / 1023, 0.5, 1.0.nextDown, 1, 1.0.nextUp, 1.5, 2, 1e30] { clears.append(SIMD4(x, x, x, x)) }
for v in order { clears.append(SIMD4<Double>(Double(v.x), Double(v.y), Double(v.z), Double(v.w))) }
clears.append(SIMD4(-1, 0.5, 2, 0.5)); clears.append(SIMD4(2, -1, 0.5, -1))
clears.append(SIMD4(Double.nan, 1, 0, Double.infinity))
for _ in 0..<32 { clears.append(SIMD4(Double(next()), Double(next()), Double(next()), Double(next()))) }

func run() throws -> [String: Any] {
    try need(CommandLine.arguments.count == 3, "usage: probe probe.metal outdir")
    let out = URL(fileURLWithPath: CommandLine.arguments[2])
    let source = try String(contentsOfFile: CommandLine.arguments[1], encoding: .utf8)
    guard let device = MTLCreateSystemDefaultDevice() else {
        throw Failure(message: "UNAVAILABLE: MTLCreateSystemDefaultDevice returned nil")
    }
    guard let queue = device.makeCommandQueue() else { throw Failure(message: "no command queue") }
    let lib = try device.makeLibrary(source: source, options: MTLCompileOptions())
    func pso(_ frag: String) throws -> MTLRenderPipelineState {
        let d = MTLRenderPipelineDescriptor()
        d.vertexFunction = lib.makeFunction(name: "tri")
        d.fragmentFunction = lib.makeFunction(name: frag)
        try need(d.vertexFunction != nil && d.fragmentFunction != nil, "missing function \(frag)")
        d.colorAttachments[0].pixelFormat = .rgb10a2Unorm
        d.colorAttachments[0].isBlendingEnabled = false
        d.colorAttachments[0].writeMask = .all
        return try device.makeRenderPipelineState(descriptor: d)
    }
    let pFloat = try pso("store_float"), pHalf = try pso("store_half")
    func target(_ w: Int, _ h: Int) throws -> MTLTexture {
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgb10a2Unorm, width: w, height: h, mipmapped: false)
        td.storageMode = .private; td.usage = [.renderTarget]
        guard let t = device.makeTexture(descriptor: td) else { throw Failure(message: "texture") }
        return t
    }
    func buffer(_ len: Int, _ fill: UInt8) throws -> MTLBuffer {
        guard let b = device.makeBuffer(length: len, options: .storageModeShared) else { throw Failure(message: "buffer") }
        memset(b.contents(), Int32(fill), len); return b
    }
    let input = try buffer(vals.count * 16, 0)
    _ = vals.withUnsafeBytes { memcpy(input.contents(), $0.baseAddress!, $0.count) }
    var width = UInt32(W)
    let tFloat = try target(W, H), tHalf = try target(W, H)
    let rbFloat = try buffer(W * H * 4, 0xa5), rbHalf = try buffer(W * H * 4, 0xa5)
    let tClear = try clears.map { _ in try target(1, 1) }
    let rbClear = try buffer(clears.count * 4, 0xa5)
    guard let cb = queue.makeCommandBuffer() else { throw Failure(message: "command buffer") }
    for (t, p) in [(tFloat, pFloat), (tHalf, pHalf)] {
        let rp = MTLRenderPassDescriptor()
        rp.colorAttachments[0].texture = t
        rp.colorAttachments[0].loadAction = .clear
        rp.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        rp.colorAttachments[0].storeAction = .store
        guard let e = cb.makeRenderCommandEncoder(descriptor: rp) else { throw Failure(message: "render encoder") }
        e.setRenderPipelineState(p)
        e.setFragmentBuffer(input, offset: 0, index: 0)
        e.setFragmentBytes(&width, length: 4, index: 1)
        e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
        e.endEncoding()
    }
    for (i, c) in clears.enumerated() {
        let rp = MTLRenderPassDescriptor()
        rp.colorAttachments[0].texture = tClear[i]
        rp.colorAttachments[0].loadAction = .clear
        rp.colorAttachments[0].clearColor = MTLClearColor(red: c.x, green: c.y, blue: c.z, alpha: c.w)
        rp.colorAttachments[0].storeAction = .store
        guard let e = cb.makeRenderCommandEncoder(descriptor: rp) else { throw Failure(message: "clear encoder") }
        e.endEncoding()
    }
    guard let blit = cb.makeBlitCommandEncoder() else { throw Failure(message: "blit") }
    for (t, b) in [(tFloat, rbFloat), (tHalf, rbHalf)] {
        blit.copy(from: t, sourceSlice: 0, sourceLevel: 0, sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
                  sourceSize: MTLSize(width: W, height: H, depth: 1), to: b, destinationOffset: 0,
                  destinationBytesPerRow: W * 4, destinationBytesPerImage: W * H * 4)
    }
    for i in 0..<clears.count {
        blit.copy(from: tClear[i], sourceSlice: 0, sourceLevel: 0, sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
                  sourceSize: MTLSize(width: 1, height: 1, depth: 1), to: rbClear, destinationOffset: i * 4,
                  destinationBytesPerRow: 4, destinationBytesPerImage: 4)
    }
    blit.endEncoding()
    cb.commit()
    cb.waitUntilCompleted()
    try need(cb.status == .completed && cb.error == nil, "command buffer status \(cb.status.rawValue) error \(String(describing: cb.error))")
    func read(_ b: MTLBuffer, _ count: Int) -> [UInt32] { Array(UnsafeBufferPointer(start: b.contents().bindMemory(to: UInt32.self, capacity: count), count: count)) }
    let oFloat = read(rbFloat, W * H), oHalf = read(rbHalf, W * H), oClear = read(rbClear, clears.count)
    // ---------- models (analysis only; raw files are the oracle) ----------
    func clampD(_ x: Double) -> Double { x.isNaN ? 0 : min(max(x, 0), 1) }
    func exactRNE(_ x: Double, _ m: Double) -> UInt32 { UInt32((clampD(x) * m).rounded(.toNearestOrEven)) }
    func floatRint(_ x: Float, _ m: Float) -> UInt32 { let c: Float = x.isNaN ? 0 : min(max(x, 0), 1); return UInt32((c * m).rounded(.toNearestOrEven)) }
    func floatFloorHalf(_ x: Float, _ m: Float) -> UInt32 { let c: Float = x.isNaN ? 0 : min(max(x, 0), 1); return UInt32((c * m + 0.5).rounded(.down)) }
    func trunc(_ x: Double, _ m: Double) -> UInt32 { UInt32((clampD(x) * m).rounded(.down)) }
    func dec(_ p: UInt32) -> [UInt32] { [p & 1023, (p >> 10) & 1023, (p >> 20) & 1023, p >> 30] }
    func tally(_ name: String, _ inputs: [SIMD4<Double>], _ asFloat: [SIMD4<Float>], _ out: [UInt32], _ cat: [UInt32]) -> [String: Any] {
        var models: [String: [Int]] = ["exact_rne": [0, 0], "float_rint": [0, 0], "float_floor_half": [0, 0], "trunc": [0, 0]]
        var examples: [[String: Any]] = []
        var rgbUnequalScalar = 0, used = 0
        for i in 0..<inputs.count where cat[i] != 255 {
            used += 1
            let got = dec(out[i])
            let d = inputs[i], f = asFloat[i]
            let ds = [d.x, d.y, d.z, d.w], fs = [f.x, f.y, f.z, f.w]
            if (cat[i] == 1 || cat[i] == 2 || cat[i] == 3 || cat[i] == 5) && !(got[0] == got[1] && got[1] == got[2]) { rgbUnequalScalar += 1 }
            for (key, fn) in [("exact_rne", { (c: Int) -> UInt32 in exactRNE(ds[c], c == 3 ? 3 : 1023) }),
                              ("float_rint", { (c: Int) -> UInt32 in floatRint(fs[c], c == 3 ? 3 : 1023) }),
                              ("float_floor_half", { (c: Int) -> UInt32 in floatFloorHalf(fs[c], c == 3 ? 3 : 1023) }),
                              ("trunc", { (c: Int) -> UInt32 in trunc(ds[c], c == 3 ? 3 : 1023) })] {
                for c in 0..<4 where fn(c) != got[c] {
                    models[key]![c == 3 ? 1 : 0] += 1
                    if key == "exact_rne" && examples.count < 64 {
                        examples.append(["i": i, "cat": cat[i], "ch": c, "in_f64": "\(ds[c])", "in_f32_bits": String(format: "0x%08x", fs[c].bitPattern),
                                         "native": got[c], "exact_rne": fn(c), "raw": String(format: "0x%08x", out[i])])
                    }
                }
            }
        }
        var r: [String: Any] = [:]
        r["path"] = name; r["pixels"] = used; r["rgb_unequal_in_scalar_categories"] = rgbUnequalScalar
        r["mismatches_rgb_alpha"] = models; r["exact_rne_examples"] = examples
        return r
    }
    let dIn = vals.map { SIMD4<Double>(Double($0.x), Double($0.y), Double($0.z), Double($0.w)) }
    let hF = vals.map { v -> SIMD4<Float> in SIMD4(Float(Float16(v.x)), Float(Float16(v.y)), Float(Float16(v.z)), Float(Float16(v.w))) }
    let hD = hF.map { SIMD4<Double>(Double($0.x), Double($0.y), Double($0.z), Double($0.w)) }
    let cF = clears.map { SIMD4<Float>(Float($0.x), Float($0.y), Float($0.z), Float($0.w)) }
    let cCat = [UInt32](repeating: 0, count: clears.count)
    var res: [String: Any] = [:]
    res["fragment_float4"] = tally("fragment_float4", dIn, vals, oFloat, cats)
    res["fragment_half4_model_on_f32_input"] = tally("fragment_half4", dIn, vals, oHalf, cats)
    res["fragment_half4_model_on_rne_half_input"] = tally("fragment_half4_rne_half", hD, hF, oHalf, cats)
    res["clear_model_on_f64"] = tally("clear", clears, cF, oClear, cCat)
    var floatEqHalf = 0
    for i in 0..<n where oFloat[i] == oHalf[i] { floatEqHalf += 1 }
    res["fragment_float4_equals_half4_pixels"] = floatEqHalf
    var files: [String: String] = [:]
    for (name, d) in [("inputs-f32x4.bin", bytes(vals)), ("categories-u32.bin", bytes(cats)), ("clears-f64x4.bin", bytes(clears)),
                      ("out-fragment-float4-u32.bin", bytes(oFloat)), ("out-fragment-half4-u32.bin", bytes(oHalf)),
                      ("out-clear-u32.bin", bytes(oClear))] {
        try d.write(to: out.appendingPathComponent(name)); files[name] = sha(d)
    }
    var dev: [String: Any] = [:]
    dev["name"] = device.name
    dev["registryID"] = device.registryID
    dev["hasUnifiedMemory"] = device.hasUnifiedMemory
    dev["isLowPower"] = device.isLowPower
    dev["isHeadless"] = device.isHeadless
    if #available(macOS 14.0, *) { dev["architecture"] = device.architecture.name }
    dev["supportsApple7"] = device.supportsFamily(.apple7); dev["supportsMac2"] = device.supportsFamily(.mac2)
    res["device"] = dev
    res["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    res["command"] = ["status": cb.status.rawValue, "completed": cb.status == .completed, "error": NSNull()] as [String: Any]
    res["layout"] = ["width": W, "height": H, "pixels_used": n, "clears": clears.count, "bit_layout_assumed_for_models": "R[0:10] G[10:20] B[20:30] A[30:32]"] as [String: Any]
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