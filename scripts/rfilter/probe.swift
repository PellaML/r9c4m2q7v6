// Public Metal API only. Synthetic data generated here; no external inputs.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ c: Bool, _ m: String) throws { if !c { throw Failure(message: m) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func bytes<T>(_ v: [T]) -> Data { v.withUnsafeBytes { Data($0) } }
func h(_ v: UInt16) -> String { String(format: "0x%04x", v) }

let W = 8, H = 8
// ---- synthetic texels (binary16 bit patterns), row-major ----
var lcg: UInt32 = 0x1234_5678
func next() -> UInt32 { lcg = lcg &* 1664525 &+ 1013904223; return lcg }
var red = [UInt16](repeating: 0, count: W * H)
let row0: [UInt16] = [0x3c00, 0x3c01, 0x3c00, 0x3c03, 0xbc00, 0xbc01, 0x0000, 0x8000] // ties, signs, zeros
let row1: [UInt16] = [0x3800, 0x3801, 0x4000, 0x4001, 0x7bff, 0x7bfe, 0x0001, 0x0003] // max finite, subnormals
for x in 0..<W { red[x] = row0[x]; red[W + x] = row1[x] }
for x in 0..<W { red[2 * W + x] = Float16(Float(3.5 - Double(x)) * 0.125).bitPattern } // signed ramp crossing 0
for y in 3..<H {
    for x in 0..<W {
        let r = next()
        let e = Int((r >> 8) % 17) - 8 // 2^-8 .. 2^8
        let m = Float((r >> 12) & 0x3ff) / 1024
        let v = Float(1 + m) * Float(pow(2.0, Double(e))) * ((r & 1) == 1 ? -1 : 1)
        red[y * W + x] = Float16(v).bitPattern
    }
}
// ---- synthetic normalized UVs ----
var uv: [SIMD2<Float>] = []
for r in 0..<H { // x sweep at each row center, texel space -0.5 .. 8.5 step 1/64
    let v = (Float(r) + 0.5) / Float(H)
    for k in 0...576 { uv.append(SIMD2((Float(k) / 64 - 0.5) / Float(W), v)) }
}
for j in 0...72 { for k in 0...72 { // 2D grid, texel space step 1/8
    uv.append(SIMD2((Float(k) / 8 - 0.5) / Float(W), (Float(j) / 8 - 0.5) / Float(H)))
} }
for r in [0, 1, 2] { // exact weight ties (k+0.5)/256 between texel columns 0|1 and 2|3, +-1 ulp
    let v = (Float(r) + 0.5) / Float(H)
    for base in [0, 2] { for k in 0..<256 {
        let u = (Float(base) + 0.5 + (Float(k) + 0.5) / 256) / Float(W)
        uv.append(SIMD2(u, v)); uv.append(SIMD2(u.nextUp, v)); uv.append(SIMD2(u.nextDown, v))
    } }
}
for u: Float in [-1, -0.0, 0, 1e-9, 0.5 / 8, 1 - 1e-7, 1, Float(1).nextUp, 2] {
    for v: Float in [-1, 0, 0.5 / 8, 0.5, 1, 2] { uv.append(SIMD2(u, v)) }
}
let n = uv.count

func run() throws -> [String: Any] {
    try need(CommandLine.arguments.count == 3, "usage: probe probe.metal outdir")
    let out = URL(fileURLWithPath: CommandLine.arguments[2])
    let source = try String(contentsOfFile: CommandLine.arguments[1], encoding: .utf8)
    guard let device = MTLCreateSystemDefaultDevice() else {
        throw Failure(message: "UNAVAILABLE: MTLCreateSystemDefaultDevice returned nil")
    }
    guard let queue = device.makeCommandQueue() else { throw Failure(message: "no command queue") }
    let opts = MTLCompileOptions()
    let lib = try device.makeLibrary(source: source, options: opts)
    guard let fn = lib.makeFunction(name: "observe") else { throw Failure(message: "no function") }
    let pso = try device.makeComputePipelineState(function: fn)
    func texture(_ f: MTLPixelFormat, _ channels: Int) throws -> (MTLTexture, Data) {
        var px = [UInt16]()
        for v in red { px.append(v); if channels >= 2 { px.append(0) }; if channels == 4 { px.append(0); px.append(0x3c00) } }
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: f, width: W, height: H, mipmapped: false)
        td.storageMode = .shared; td.usage = [.shaderRead]
        guard let t = device.makeTexture(descriptor: td) else { throw Failure(message: "texture \(f.rawValue)") }
        px.withUnsafeBytes { t.replace(region: MTLRegionMake2D(0, 0, W, H), mipmapLevel: 0,
                                       withBytes: $0.baseAddress!, bytesPerRow: W * channels * 2) }
        return (t, bytes(px))
    }
    let (t1, d1) = try texture(.r16Float, 1)
    let (t2, d2) = try texture(.rg16Float, 2)
    let (t4, d4) = try texture(.rgba16Float, 4)
    func buf(_ d: Data) throws -> MTLBuffer {
        try d.withUnsafeBytes { (raw: UnsafeRawBufferPointer) throws -> MTLBuffer in guard let b = device.makeBuffer(bytes: raw.baseAddress!, length: raw.count,
                                                                  options: .storageModeShared) else { throw Failure(message: "buffer") }; return b }
    }
    let ub = try buf(bytes(uv))
    let poison = bytes([UInt16](repeating: 0xdead, count: n * 4))
    let o1 = try buf(poison), o2 = try buf(poison), o4 = try buf(poison)
    var count = UInt32(n)
    guard let cb = queue.makeCommandBuffer(), let enc = cb.makeComputeCommandEncoder() else { throw Failure(message: "encoder") }
    enc.setComputePipelineState(pso)
    enc.setTexture(t1, index: 0); enc.setTexture(t2, index: 1); enc.setTexture(t4, index: 2)
    enc.setBuffer(ub, offset: 0, index: 0); enc.setBuffer(o1, offset: 0, index: 1)
    enc.setBuffer(o2, offset: 0, index: 2); enc.setBuffer(o4, offset: 0, index: 3)
    enc.setBytes(&count, length: 4, index: 4)
    enc.dispatchThreadgroups(MTLSize(width: (n + 63) / 64, height: 1, depth: 1), threadsPerThreadgroup: MTLSize(width: 64, height: 1, depth: 1))
    enc.endEncoding()
    cb.commit()
    cb.waitUntilCompleted()
    try need(cb.status == .completed && cb.error == nil, "command buffer status \(cb.status.rawValue) error \(String(describing: cb.error))")
    func read(_ b: MTLBuffer) -> [UInt16] { Array(UnsafeBufferPointer(start: b.contents().bindMemory(to: UInt16.self, capacity: n * 4), count: n * 4)) }
    let a1 = read(o1), a2 = read(o2), a4 = read(o4)
    var files: [String: String] = [:]
    for (name, d) in [("texels-r16.bin", d1), ("texels-rg16.bin", d2), ("texels-rgba16.bin", d4), ("uv-f32.bin", bytes(uv)),
                      ("out-r16.bin", bytes(a1)), ("out-rg16.bin", bytes(a2)), ("out-rgba16.bin", bytes(a4))] {
        try d.write(to: out.appendingPathComponent(name)); files[name] = sha(d)
    }
    var rMis = 0, rgMis = 0, constBad1 = 0, constBad2 = 0, constBad4 = 0, poisoned = 0
    var examples: [[String: Any]] = []
    for i in 0..<n {
        let p1 = Array(a1[i*4..<i*4+4]), p2 = Array(a2[i*4..<i*4+4]), p4 = Array(a4[i*4..<i*4+4])
        if (p1 + p2 + p4).contains(0xdead) { poisoned += 1 }
        if p1[0] != p4[0] { rMis += 1; if examples.count < 32 { var e: [String: Any] = [:]; e["i"] = i; e["u"] = uv[i].x; e["v"] = uv[i].y; e["r16"] = p1.map(h); e["rgba16"] = p4.map(h); examples.append(e) } }
        if p2[0] != p4[0] { rgMis += 1 }
        if p1[1] != 0 || p1[2] != 0 || p1[3] != 0x3c00 { constBad1 += 1 }
        if p2[1] != 0 || p2[2] != 0 || p2[3] != 0x3c00 { constBad2 += 1 }
        if p4[1] != 0 || p4[2] != 0 || p4[3] != 0x3c00 { constBad4 += 1 }
    }
    var dev: [String: Any] = [:]
    dev["name"] = device.name
    dev["registryID"] = device.registryID
    dev["hasUnifiedMemory"] = device.hasUnifiedMemory
    dev["isLowPower"] = device.isLowPower
    dev["isRemovable"] = device.isRemovable
    dev["isHeadless"] = device.isHeadless
    if #available(macOS 14.0, *) { dev["architecture"] = device.architecture.name }
    dev["supportsApple7"] = device.supportsFamily(.apple7); dev["supportsMac2"] = device.supportsFamily(.mac2)
    var cmd: [String: Any] = [:]
    cmd["status"] = cb.status.rawValue
    cmd["completed"] = cb.status == .completed
    cmd["error"] = NSNull()
    cmd["gpuStartTime"] = cb.gpuStartTime
    cmd["gpuEndTime"] = cb.gpuEndTime
    var res: [String: Any] = [:]
    let same = rMis == 0 && constBad1 == 0 && poisoned == 0
    res["status"] = same ? "R16F_EQUALS_RGBA16F_RED" : "DIFFERENCE_OBSERVED"
    res["device"] = dev
    res["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    res["command"] = cmd
    res["sampler"] = "normalized, clamp_to_edge, linear min/mag, mip none, level(0), compute kernel"
    res["samples"] = n
    res["texels"] = W * H
    res["poisoned"] = poisoned
    res["r16_vs_rgba16_red_mismatches"] = rMis
    res["rg16_vs_rgba16_red_mismatches"] = rgMis
    res["r16_gba_not_0_0_1"] = constBad1
    res["rg16_gba_not_0_0_1"] = constBad2
    res["rgba16_gba_not_0_0_1"] = constBad4
    res["examples"] = examples
    res["files_sha256"] = files
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
