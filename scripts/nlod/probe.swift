// Public Metal API only. Newly authored synthetic data; no external inputs.
// Compares two MTLSamplerState objects that differ ONLY in lodAverage, with
// nearest min/mag, NotMipmapped, normalized, clamp-to-edge, single-mip RGBA16F.
// lodAverage is set through KVC so that a missing Swift spelling cannot be
// mistaken for a GPU result; availability is recorded separately.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ c: Bool, _ m: String) throws { if !c { throw Failure(message: m) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func bytes<T>(_ v: [T]) -> Data { v.withUnsafeBytes { Data($0) } }

let TW = 8, TH = 8
// texel (x,y): r = x+1, g = y+1, b = 8x+y, a = 0.25 (all exact in binary16)
var texels: [UInt16] = []
for y in 0..<TH { for x in 0..<TW {
    texels += [Float16(Float(x + 1)).bitPattern, Float16(Float(y + 1)).bitPattern,
               Float16(Float(8 * x + y)).bitPattern, Float16(Float(0.25)).bitPattern]
} }
// boundary coordinate set (normalized): every texel edge k/8 and +-1,+-2 ulp,
// texel centers, and out-of-range / signed-zero values.
var axis: [Float] = []
for k in 0...8 {
    let e = Float(k) / 8
    axis += [e.nextDown.nextDown, e.nextDown, e, e.nextUp, e.nextUp.nextUp]
}
for k in 0..<8 { axis.append((Float(k) + 0.5) / 8) }
axis += [-1, -0.0, -1e-30, 1e-30, 2, 1e6, -1e6, Float(0.5).nextUp]
var uvs: [SIMD2<Float>] = []
for v in axis { for u in axis { uvs.append(SIMD2(u, v)) } }
let n = uvs.count
let W = 64
let H = (n + W - 1) / W
var padded = uvs
while padded.count < W * H { padded.append(SIMD2(0.5, 0.5)) }

func run() throws -> [String: Any] {
    try need(CommandLine.arguments.count == 3, "usage: probe probe.metal outdir")
    let out = URL(fileURLWithPath: CommandLine.arguments[2])
    let source = try String(contentsOfFile: CommandLine.arguments[1], encoding: .utf8)
    guard let device = MTLCreateSystemDefaultDevice() else {
        throw Failure(message: "UNAVAILABLE: MTLCreateSystemDefaultDevice returned nil")
    }
    guard let queue = device.makeCommandQueue() else { throw Failure(message: "no command queue") }
    let lib = try device.makeLibrary(source: source, options: MTLCompileOptions())
    let pd = MTLRenderPipelineDescriptor()
    pd.vertexFunction = lib.makeFunction(name: "tri")
    pd.fragmentFunction = lib.makeFunction(name: "observe")
    try need(pd.vertexFunction != nil && pd.fragmentFunction != nil, "missing function")
    pd.colorAttachments[0].pixelFormat = .rgba16Float
    let pso = try device.makeRenderPipelineState(descriptor: pd)

    var api: [String: Any] = [:]
    let setSel = NSSelectorFromString("setLodAverage:"), getSel = NSSelectorFromString("lodAverage")
    var states: [MTLSamplerState] = []
    var observed: [[String: Any]] = []
    for k in 0..<2 {
        let sd = MTLSamplerDescriptor()
        sd.minFilter = .nearest; sd.magFilter = .nearest; sd.mipFilter = .notMipmapped
        sd.normalizedCoordinates = true; sd.maxAnisotropy = 1
        sd.lodMinClamp = 0; sd.lodMaxClamp = Float.greatestFiniteMagnitude
        sd.sAddressMode = .clampToEdge; sd.tAddressMode = .clampToEdge; sd.rAddressMode = .clampToEdge
        sd.compareFunction = .never
        let canSet = sd.responds(to: setSel), canGet = sd.responds(to: getSel)
        api["responds_setLodAverage"] = canSet; api["responds_lodAverage"] = canGet
        guard canSet && canGet else {
            throw Failure(message: "API_UNAVAILABLE: MTLSamplerDescriptor does not respond to lodAverage selectors (not a GPU result)")
        }
        sd.setValue(k == 1, forKey: "lodAverage")
        let readBack = (sd.value(forKey: "lodAverage") as? Bool) ?? false
        try need(readBack == (k == 1), "lodAverage readback mismatch")
        try need(sd.minFilter == .nearest && sd.magFilter == .nearest && sd.mipFilter == .notMipmapped &&
                 sd.maxAnisotropy == 1 && sd.normalizedCoordinates && sd.compareFunction == .never, "descriptor mismatch")
        guard let ss = device.makeSamplerState(descriptor: sd) else { throw Failure(message: "sampler allocation \(k)") }
        states.append(ss)
        observed.append(["lod_average": readBack, "min": sd.minFilter.rawValue, "mag": sd.magFilter.rawValue,
                         "mip": sd.mipFilter.rawValue, "normalized": sd.normalizedCoordinates, "max_anisotropy": sd.maxAnisotropy,
                         "lod_min": sd.lodMinClamp, "lod_max": "FLT_MAX", "s": sd.sAddressMode.rawValue, "t": sd.tAddressMode.rawValue,
                         "r": sd.rAddressMode.rawValue, "compare": sd.compareFunction.rawValue])
    }
    let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba16Float, width: TW, height: TH, mipmapped: false)
    td.storageMode = .shared; td.usage = [.shaderRead]
    guard let tex = device.makeTexture(descriptor: td) else { throw Failure(message: "texture") }
    try need(tex.mipmapLevelCount == 1, "mip count")
    texels.withUnsafeBytes { tex.replace(region: MTLRegionMake2D(0, 0, TW, TH), mipmapLevel: 0, withBytes: $0.baseAddress!, bytesPerRow: TW * 8) }
    let rtd = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba16Float, width: W, height: H, mipmapped: false)
    rtd.storageMode = .private; rtd.usage = [.renderTarget]
    guard let rt = device.makeTexture(descriptor: rtd) else { throw Failure(message: "target") }
    guard let ub = device.makeBuffer(length: padded.count * 8, options: .storageModeShared),
          let ob = device.makeBuffer(length: padded.count * 10 * 8, options: .storageModeShared) else { throw Failure(message: "buffer") }
    _ = padded.withUnsafeBytes { memcpy(ub.contents(), $0.baseAddress!, $0.count) }
    memset(ob.contents(), 0xa5, ob.length)
    var dims = SIMD2<UInt32>(UInt32(W), UInt32(W * H))
    guard let cb = queue.makeCommandBuffer() else { throw Failure(message: "command buffer") }
    let rp = MTLRenderPassDescriptor()
    rp.colorAttachments[0].texture = rt; rp.colorAttachments[0].loadAction = .clear; rp.colorAttachments[0].storeAction = .dontCare
    guard let e = cb.makeRenderCommandEncoder(descriptor: rp) else { throw Failure(message: "encoder") }
    e.setRenderPipelineState(pso)
    e.setFragmentTexture(tex, index: 0)
    e.setFragmentSamplerState(states[0], index: 0); e.setFragmentSamplerState(states[1], index: 1)
    e.setFragmentBuffer(ub, offset: 0, index: 0); e.setFragmentBuffer(ob, offset: 0, index: 1)
    e.setFragmentBytes(&dims, length: 8, index: 2)
    e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
    e.endEncoding()
    cb.commit(); cb.waitUntilCompleted()
    try need(cb.status == .completed && cb.error == nil, "command buffer status \(cb.status.rawValue) error \(String(describing: cb.error))")
    let o = Array(UnsafeBufferPointer(start: ob.contents().bindMemory(to: UInt16.self, capacity: padded.count * 40), count: padded.count * 40))

    // ---------- analysis (raw output is the oracle) ----------
    func idx(_ c: Float, _ size: Int) -> Int { let t = (c * Float(size)).rounded(.down); return t.isNaN ? 0 : Int(min(max(t, 0), Float(size - 1))) }
    var poisoned = 0, avgDiff = 0, methodDiff = 0, modelMis = 0
    var avgDiffByMethod = [Int](repeating: 0, count: 5), modelMisByMethod = [Int](repeating: 0, count: 10)
    var examples: [[String: Any]] = []
    for i in 0..<n {
        let base = i * 40
        let px = idx(uvs[i].x, TW), py = idx(uvs[i].y, TH)
        let expect = Array(texels[(py * TW + px) * 4..<(py * TW + px) * 4 + 4])
        for s in 0..<2 { for m in 0..<5 {
            let at = base + (s * 5 + m) * 4
            let got = Array(o[at..<at + 4])
            if got.contains(0xa5a5) { poisoned += 1 }
            if got != expect {
                modelMis += 1; modelMisByMethod[s * 5 + m] += 1
                if examples.count < 48 {
                    examples.append(["i": i, "u": "\(uvs[i].x)", "v": "\(uvs[i].y)", "u_bits": String(format: "0x%08x", uvs[i].x.bitPattern),
                                     "v_bits": String(format: "0x%08x", uvs[i].y.bitPattern), "sampler_avg": s, "method": m,
                                     "got": got.map { String(format: "0x%04x", $0) }, "floor_model": expect.map { String(format: "0x%04x", $0) }])
                }
            }
            if m > 0 && got != Array(o[base + s * 20..<base + s * 20 + 4]) { methodDiff += 1 }
        } }
        for m in 0..<5 where Array(o[base + m * 4..<base + m * 4 + 4]) != Array(o[base + 20 + m * 4..<base + 20 + m * 4 + 4]) {
            avgDiff += 1; avgDiffByMethod[m] += 1
        }
    }
    var files: [String: String] = [:]
    for (name, d) in [("texels-rgba16f.bin", bytes(texels)), ("uv-f32x2.bin", bytes(uvs)),
                      ("out-half4-coord-sampler-method.bin", bytes(Array(o[0..<n * 40])))] {
        try d.write(to: out.appendingPathComponent(name)); files[name] = sha(d)
    }
    var dev: [String: Any] = [:]
    dev["name"] = device.name
    dev["registryID"] = device.registryID
    dev["hasUnifiedMemory"] = device.hasUnifiedMemory
    if #available(macOS 14.0, *) { dev["architecture"] = device.architecture.name }
    var res: [String: Any] = [:]
    res["status"] = (avgDiff == 0 && poisoned == 0) ? "LODAVERAGE_FALSE_EQUALS_TRUE" : "DIFFERENCE_OBSERVED"
    res["api"] = api
    res["sampler_descriptors_observed"] = observed
    res["device"] = dev
    res["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    res["command"] = ["status": cb.status.rawValue, "completed": cb.status == .completed, "error": NSNull()] as [String: Any]
    res["coords"] = n
    res["methods"] = "0 implicit-derivative (arbitrary per-pixel coords), 1 level(0), 2 gradient 0.001, 3 gradient 4.0, 4 bias(4)"
    res["layout"] = "out[(coord*2+lodAverage)*5+method] = half4 bits"
    res["poisoned"] = poisoned
    res["avg_false_vs_true_mismatch_coord_methods"] = avgDiff
    res["avg_mismatch_by_method"] = avgDiffByMethod
    res["method_vs_implicit_mismatches"] = methodDiff
    res["floor_clamp_nearest_model_mismatches"] = modelMis
    res["model_mismatch_by_sampler_method"] = modelMisByMethod
    res["model_examples"] = examples
    res["files_sha256"] = files
    return res
}
let outDir = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "."
var result: [String: Any]
var code: Int32 = 0
do { result = try run() } catch let e as Failure {
    let kind = e.message.hasPrefix("UNAVAILABLE") ? "UNAVAILABLE_NOT_PASS" : (e.message.hasPrefix("API_UNAVAILABLE") ? "API_UNAVAILABLE_NOT_GPU_RESULT" : "FAILED_NOT_PASS")
    result = ["status": kind, "error": e.message]; code = 3
} catch { result = ["status": "FAILED_NOT_PASS", "error": "\(error)"]; code = 3 }
let json = try! JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
try! json.write(to: URL(fileURLWithPath: outDir).appendingPathComponent("summary.json"))
FileHandle.standardOutput.write(json)
exit(code)